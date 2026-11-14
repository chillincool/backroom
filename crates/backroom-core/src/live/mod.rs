//! A long-lived session with soju: what a chat app runs on.
//!
//! [`start`] connects a control connection (which lists and watches the
//! bouncer's networks) plus one connection bound to each network, keeps them
//! connected, and turns IRC traffic into [`Event`]s. The front end sends
//! [`Command`]s through the returned [`SessionHandle`].
//!
//! ```text
//!  front end ──Command──▶ coordinator ──ToConn──▶ connection task (control)
//!            ◀──Event────            ◀─FromConn─  connection task (network 1)
//!                                                 connection task (network …)
//! ```
//!
//! All state lives in the coordinator task; connection tasks only do I/O,
//! reconnecting with backoff. A rejected login stops reconnecting instead of
//! retrying forever.

mod connection;
mod coordinator;
mod members;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::sync::mpsc;

use crate::auth::CredentialProvider;
use crate::model::{BufferKind, ChannelListing, ChatMessage, Member, NetworkId, NetworkInfo};
use crate::transport::Connector;

#[derive(Clone)]
pub struct SessionConfig {
    pub connector: Arc<dyn Connector>,
    pub credentials: Arc<dyn CredentialProvider>,
    /// Nick to register with; soju replaces it with each network's own nick.
    pub nick: String,
    pub backoff: Backoff,
    /// Send a PING after this long without traffic; give up on the
    /// connection after twice as long.
    pub keepalive: Duration,
}

impl SessionConfig {
    pub fn new(
        connector: Arc<dyn Connector>,
        credentials: Arc<dyn CredentialProvider>,
        nick: impl Into<String>,
    ) -> Self {
        SessionConfig {
            connector,
            credentials,
            nick: nick.into(),
            backoff: Backoff::default(),
            keepalive: Duration::from_secs(60),
        }
    }
}

/// Reconnect delays: `initial`, doubling up to `max`.
#[derive(Debug, Clone, Copy)]
pub struct Backoff {
    pub initial: Duration,
    pub max: Duration,
}

impl Default for Backoff {
    fn default() -> Self {
        Backoff {
            initial: Duration::from_secs(1),
            max: Duration::from_secs(60),
        }
    }
}

impl Backoff {
    /// Delay before reconnect attempt `attempt` (0-based).
    pub fn delay(&self, attempt: u32) -> Duration {
        let factor = 2u32.saturating_pow(attempt.min(16));
        self.initial.saturating_mul(factor).min(self.max)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionState {
    Connecting,
    Connected,
    /// Lost or failed; another attempt follows after `retry_in`.
    Disconnected {
        error: String,
        retry_in: Duration,
    },
    /// The server (or token refresh) rejected the login. No more attempts;
    /// the user needs to log in again.
    AuthFailed {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// State of the control connection (`network: None`) or a network's.
    Connection {
        network: Option<NetworkId>,
        state: ConnectionState,
    },
    /// A network was added or changed; carries all its current attributes.
    NetworkUpdated(NetworkInfo),
    NetworkRemoved(NetworkId),
    BufferAdded {
        network: NetworkId,
        name: String,
        kind: BufferKind,
    },
    /// The buffer is gone: we left the channel, or the front end closed it.
    BufferRemoved {
        network: NetworkId,
        name: String,
    },
    /// A channel's full member list, after NAMES and after every change.
    Members {
        network: NetworkId,
        channel: String,
        members: Vec<Member>,
    },
    /// A channel's topic: when joining, and whenever it changes. `None`:
    /// no topic set.
    Topic {
        network: NetworkId,
        channel: String,
        topic: Option<String>,
    },
    /// A live message.
    Message {
        network: NetworkId,
        buffer: String,
        message: ChatMessage,
    },
    /// A page of history, oldest first, in reply to
    /// [`Command::LoadHistory`].
    History {
        network: NetworkId,
        buffer: String,
        messages: Vec<ChatMessage>,
    },
    /// Read marker (`draft/read-marker`); `None` means nothing read yet.
    ReadMarker {
        network: NetworkId,
        buffer: String,
        time: Option<DateTime<Utc>>,
    },
    /// Part of the channel list, in reply to [`Command::ListChannels`].
    /// Big networks list tens of thousands of channels, so they arrive in
    /// chunks; `done` marks the last one (which may be empty).
    ChannelList {
        network: NetworkId,
        channels: Vec<ChannelListing>,
        done: bool,
    },
    /// Something failed that the user may want to see, e.g. a `FAIL` reply
    /// or a command for a disconnected network.
    Error {
        network: Option<NetworkId>,
        message: String,
    },
    /// The session has shut down; no more events follow.
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    SendMessage {
        network: NetworkId,
        target: String,
        text: String,
    },
    /// Joins channels (comma-separated), with a key for `+k` channels.
    Join {
        network: NetworkId,
        channel: String,
        key: Option<String>,
    },
    Part {
        network: NetworkId,
        channel: String,
        reason: Option<String>,
    },
    /// Fetch up to `limit` messages before `before` (or the latest if
    /// `None`). Answered with [`Event::History`].
    LoadHistory {
        network: NetworkId,
        target: String,
        before: Option<DateTime<Utc>>,
        limit: usize,
    },
    /// Adds a network to soju (`BOUNCER ADDNETWORK`): attributes such as
    /// `name`, `host`, `port`, `tls` (`1`/`0`) and `nickname`. It appears as
    /// [`Event::NetworkUpdated`]; a rejection arrives as [`Event::Error`].
    AddNetwork {
        attributes: BTreeMap<String, String>,
    },
    /// Changes a network's attributes (`BOUNCER CHANGENETWORK`); an empty
    /// value removes the attribute.
    ChangeNetwork {
        network: NetworkId,
        attributes: BTreeMap<String, String>,
    },
    /// Deletes a network from soju (`BOUNCER DELNETWORK`).
    RemoveNetwork {
        network: NetworkId,
    },
    /// Asks for the network's channel list (`LIST`). Answered with
    /// [`Event::ChannelList`] chunks.
    ListChannels {
        network: NetworkId,
    },
    /// Sends a raw IRC line on a network, e.g. `WHOIS alice`.
    SendRaw {
        network: NetworkId,
        line: String,
    },
    /// Closes a buffer: parts a channel (removed when the server confirms)
    /// or forgets a private conversation until its history lists it again.
    CloseBuffer {
        network: NetworkId,
        name: String,
    },
    /// Mark everything up to `time` as read, on every device.
    MarkRead {
        network: NetworkId,
        target: String,
        time: DateTime<Utc>,
    },
    Shutdown,
}

/// Sends commands to a running session. Cheap to clone.
#[derive(Clone)]
pub struct SessionHandle {
    tx: mpsc::UnboundedSender<Command>,
}

impl SessionHandle {
    /// Queues a command. Returns `false` if the session has stopped.
    pub fn send(&self, command: Command) -> bool {
        self.tx.send(command).is_ok()
    }

    pub fn shutdown(&self) {
        let _ = self.tx.send(Command::Shutdown);
    }
}

/// Starts a session on the current tokio runtime.
pub fn start(config: SessionConfig) -> (SessionHandle, mpsc::UnboundedReceiver<Event>) {
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let (event_tx, event_rx) = mpsc::unbounded_channel();
    tokio::spawn(coordinator::run(Arc::new(config), cmd_rx, event_tx));
    (SessionHandle { tx: cmd_tx }, event_rx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_up_to_max() {
        let b = Backoff::default();
        let secs: Vec<u64> = (0..9).map(|a| b.delay(a).as_secs()).collect();
        assert_eq!(secs, [1, 2, 4, 8, 16, 32, 60, 60, 60]);
        assert_eq!(b.delay(u32::MAX), Duration::from_secs(60));
    }
}
