//! A long-lived session: what a chat app runs on.
//!
//! With soju ([`Backend::Bouncer`]), [`start`] connects a control connection
//! (which lists and watches the bouncer's networks) plus one connection
//! bound to each network. Without a bouncer ([`Backend::Standalone`]), it
//! connects to each saved network directly and keeps networks, channels,
//! history and read markers itself ([`crate::local`]). Either way it keeps
//! the connections up and turns IRC traffic into the same [`Event`]s; the
//! front end sends [`Command`]s through the returned [`SessionHandle`].
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
use crate::local::Standalone;
use crate::model::{
    BanEntry, BufferKind, ChannelListing, ChatMessage, Member, NetworkId, NetworkInfo, TypingState,
    WhoisInfo,
};
use crate::transport::Connector;

/// Where the networks are.
#[derive(Clone)]
pub enum Backend {
    /// A soju bouncer: it keeps the networks, channels and history.
    Bouncer {
        connector: Arc<dyn Connector>,
        credentials: Arc<dyn CredentialProvider>,
    },
    /// No bouncer: networks are saved locally and connected to directly.
    Standalone(Arc<Standalone>),
}

#[derive(Clone)]
pub struct SessionConfig {
    pub backend: Backend,
    /// Nick to register with: soju replaces it with each network's own
    /// nick; standalone networks without a `nickname` use it.
    pub nick: String,
    pub backoff: Backoff,
    /// Send a PING after this long without traffic; give up on the
    /// connection after twice as long.
    pub keepalive: Duration,
    /// Our answer to CTCP VERSION, e.g. "Backroom 0.1 (macOS)".
    pub client_version: String,
}

impl SessionConfig {
    /// A session with soju.
    pub fn new(
        connector: Arc<dyn Connector>,
        credentials: Arc<dyn CredentialProvider>,
        nick: impl Into<String>,
    ) -> Self {
        Self::with_backend(
            Backend::Bouncer {
                connector,
                credentials,
            },
            nick,
        )
    }

    /// A session without a bouncer.
    pub fn standalone(local: Standalone, nick: impl Into<String>) -> Self {
        Self::with_backend(Backend::Standalone(Arc::new(local)), nick)
    }

    fn with_backend(backend: Backend, nick: impl Into<String>) -> Self {
        SessionConfig {
            backend,
            nick: nick.into(),
            backoff: Backoff::default(),
            keepalive: Duration::from_secs(60),
            client_version: "Backroom".into(),
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
    /// Messages matching a [`Command::Search`], oldest first, with the buffer
    /// each belongs to; `error` if the server refused.
    SearchResults {
        network: NetworkId,
        target: String,
        text: String,
        results: Vec<(String, ChatMessage)>,
        error: Option<String>,
    },
    /// Someone's typing state in a buffer.
    Typing {
        network: NetworkId,
        buffer: String,
        nick: String,
        state: TypingState,
    },
    /// Whether someone we have a private conversation with is online
    /// (MONITOR, where the network supports it).
    Presence {
        network: NetworkId,
        nick: String,
        online: bool,
    },
    /// A channel's modes (`RPL_CHANNELMODEIS`): `+ntk` and their arguments.
    ChannelModes {
        network: NetworkId,
        channel: String,
        modes: String,
        params: Vec<String>,
    },
    /// A channel's ban list, after [`Command::LoadChannelSettings`].
    BanList {
        network: NetworkId,
        channel: String,
        bans: Vec<BanEntry>,
    },
    /// Our nick on a network: after registering and whenever it changes.
    OwnNick {
        network: NetworkId,
        nick: String,
    },
    /// Whether we're marked away on a network (`RPL_NOWAWAY`/`RPL_UNAWAY`).
    Away {
        network: NetworkId,
        away: bool,
    },
    /// A complete WHOIS reply.
    Whois {
        network: NetworkId,
        info: WhoisInfo,
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
    /// Adds a network (`BOUNCER ADDNETWORK` with soju): attributes such as
    /// `name`, `host`, `port`, `tls` (`1`/`0`) and `nickname`; standalone
    /// networks also take a login (see [`crate::local::NetworkConfig`]). It
    /// appears as [`Event::NetworkUpdated`]; a rejection arrives as
    /// [`Event::Error`].
    AddNetwork {
        attributes: BTreeMap<String, String>,
    },
    /// Changes a network's attributes (`BOUNCER CHANGENETWORK`); an empty
    /// value removes the attribute.
    ChangeNetwork {
        network: NetworkId,
        attributes: BTreeMap<String, String>,
    },
    /// Deletes a network (`BOUNCER DELNETWORK` with soju), with its history.
    RemoveNetwork {
        network: NetworkId,
    },
    /// Asks for the network's channel list (`LIST`). Answered with
    /// [`Event::ChannelList`] chunks.
    ListChannels {
        network: NetworkId,
    },
    /// Searches a buffer's history (`soju.im/search`, or the local store).
    /// Answered with [`Event::SearchResults`].
    Search {
        network: NetworkId,
        target: String,
        text: String,
    },
    /// Tells the others in a conversation that we're typing.
    Typing {
        network: NetworkId,
        target: String,
        state: TypingState,
    },
    /// Asks for a channel's modes and ban list ([`Event::ChannelModes`],
    /// [`Event::BanList`]).
    LoadChannelSettings {
        network: NetworkId,
        channel: String,
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
    /// Mark everything up to `time` as read (with soju, on every device).
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
