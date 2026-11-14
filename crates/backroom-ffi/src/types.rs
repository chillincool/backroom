//! FFI-friendly copies of the core's chat model and session events.
//!
//! The core types use chrono and `BTreeMap`; these use `SystemTime` (Swift
//! `Date`), `Duration` (`TimeInterval`) and `HashMap` (`Dictionary`).

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use backroom_core::{live, model};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum BufferKind {
    /// Notices and replies from the server itself, in the buffer named `*`.
    Server,
    Channel,
    /// A private conversation, named after the other user's nick.
    Query,
}

impl From<model::BufferKind> for BufferKind {
    fn from(kind: model::BufferKind) -> Self {
        match kind {
            model::BufferKind::Server => BufferKind::Server,
            model::BufferKind::Channel => BufferKind::Channel,
            model::BufferKind::Query => BufferKind::Query,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum MessageKind {
    Privmsg,
    Notice,
    /// CTCP ACTION (`/me`).
    Action,
    Join,
    Part,
    Kick,
    Topic,
}

impl From<model::MessageKind> for MessageKind {
    fn from(kind: model::MessageKind) -> Self {
        match kind {
            model::MessageKind::Privmsg => MessageKind::Privmsg,
            model::MessageKind::Notice => MessageKind::Notice,
            model::MessageKind::Action => MessageKind::Action,
            model::MessageKind::Join => MessageKind::Join,
            model::MessageKind::Part => MessageKind::Part,
            model::MessageKind::Kick => MessageKind::Kick,
            model::MessageKind::Topic => MessageKind::Topic,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ChatMessage {
    /// IRCv3 `msgid`, when the server provides one.
    pub id: Option<String>,
    pub time: SystemTime,
    pub sender: String,
    pub kind: MessageKind,
    /// Text as sent, mIRC formatting included; see [`strip_formatting`](crate::strip_formatting).
    pub text: String,
    /// Sent by this user, including echoes from other devices.
    pub own: bool,
}

impl From<model::ChatMessage> for ChatMessage {
    fn from(m: model::ChatMessage) -> Self {
        ChatMessage {
            id: m.id,
            time: m.time.into(),
            sender: m.sender,
            kind: m.kind.into(),
            text: m.text,
            own: m.own,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct Network {
    /// soju network ID; pass it back in commands.
    pub id: String,
    /// Display name: soju's `name`, else `host`, else the ID.
    pub name: String,
    /// soju's upstream state: `connected`, `connecting` or `disconnected`.
    pub upstream_state: Option<String>,
    /// All attributes soju reported (`host`, `nickname`, `error`, …).
    pub attributes: HashMap<String, String>,
}

impl From<model::NetworkInfo> for Network {
    fn from(info: model::NetworkInfo) -> Self {
        Network {
            name: info.name().to_owned(),
            upstream_state: info.upstream_state().map(str::to_owned),
            attributes: info.attributes.into_iter().collect(),
            id: info.id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum ConnectionState {
    Connecting,
    Connected,
    /// Lost or failed; another attempt follows after `retry_in`.
    Disconnected {
        error: String,
        retry_in: Duration,
    },
    /// The login was rejected. No more attempts; log in again.
    AuthFailed {
        reason: String,
    },
}

impl From<live::ConnectionState> for ConnectionState {
    fn from(state: live::ConnectionState) -> Self {
        match state {
            live::ConnectionState::Connecting => ConnectionState::Connecting,
            live::ConnectionState::Connected => ConnectionState::Connected,
            live::ConnectionState::Disconnected { error, retry_in } => {
                ConnectionState::Disconnected { error, retry_in }
            }
            live::ConnectionState::AuthFailed { reason } => ConnectionState::AuthFailed { reason },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum SessionEvent {
    /// State of the control connection (`network` is nil) or a network's.
    Connection {
        network: Option<String>,
        state: ConnectionState,
    },
    /// A network was added or changed; carries all its current attributes.
    NetworkUpdated {
        network: Network,
    },
    NetworkRemoved {
        id: String,
    },
    BufferAdded {
        network: String,
        name: String,
        kind: BufferKind,
    },
    /// A live message.
    Message {
        network: String,
        buffer: String,
        message: ChatMessage,
    },
    /// A page of history, oldest first, answering `load_history`.
    History {
        network: String,
        buffer: String,
        messages: Vec<ChatMessage>,
    },
    /// Read marker; nil `time` means nothing has been read yet.
    ReadMarker {
        network: String,
        buffer: String,
        time: Option<SystemTime>,
    },
    /// Something failed that the user may want to see.
    Error {
        network: Option<String>,
        message: String,
    },
    /// The session has shut down; `next_event` returns nil after this.
    Stopped,
}

impl From<live::Event> for SessionEvent {
    fn from(event: live::Event) -> Self {
        use live::Event as E;
        match event {
            E::Connection { network, state } => SessionEvent::Connection {
                network,
                state: state.into(),
            },
            E::NetworkUpdated(info) => SessionEvent::NetworkUpdated {
                network: info.into(),
            },
            E::NetworkRemoved(id) => SessionEvent::NetworkRemoved { id },
            E::BufferAdded {
                network,
                name,
                kind,
            } => SessionEvent::BufferAdded {
                network,
                name,
                kind: kind.into(),
            },
            E::Message {
                network,
                buffer,
                message,
            } => SessionEvent::Message {
                network,
                buffer,
                message: message.into(),
            },
            E::History {
                network,
                buffer,
                messages,
            } => SessionEvent::History {
                network,
                buffer,
                messages: messages.into_iter().map(Into::into).collect(),
            },
            E::ReadMarker {
                network,
                buffer,
                time,
            } => SessionEvent::ReadMarker {
                network,
                buffer,
                time: time.map(Into::into),
            },
            E::Error { network, message } => SessionEvent::Error { network, message },
            E::Stopped => SessionEvent::Stopped,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use chrono::{DateTime, Utc};

    use super::*;

    #[test]
    fn network_uses_display_name_and_state() {
        let info = model::NetworkInfo {
            id: "7".into(),
            attributes: BTreeMap::from([
                ("host".to_owned(), "irc.libera.chat".to_owned()),
                ("state".to_owned(), "connected".to_owned()),
            ]),
        };
        let net = Network::from(info);
        assert_eq!(net.id, "7");
        assert_eq!(net.name, "irc.libera.chat");
        assert_eq!(net.upstream_state.as_deref(), Some("connected"));
        assert_eq!(net.attributes.len(), 2);
    }

    #[test]
    fn message_time_survives_conversion() {
        let time = DateTime::<Utc>::from_timestamp(1_790_000_000, 123_000_000).unwrap();
        let event = SessionEvent::from(live::Event::Message {
            network: "1".into(),
            buffer: "#backroom".into(),
            message: model::ChatMessage {
                id: Some("abc".into()),
                time,
                sender: "erin".into(),
                kind: model::MessageKind::Action,
                text: "waves".into(),
                own: false,
            },
        });
        let SessionEvent::Message { message, .. } = event else {
            panic!("wrong event: {event:?}");
        };
        assert_eq!(DateTime::<Utc>::from(message.time), time);
        assert_eq!(message.kind, MessageKind::Action);
    }

    #[test]
    fn disconnected_keeps_retry_delay() {
        let event = SessionEvent::from(live::Event::Connection {
            network: None,
            state: live::ConnectionState::Disconnected {
                error: "reset".into(),
                retry_in: Duration::from_secs(4),
            },
        });
        assert_eq!(
            event,
            SessionEvent::Connection {
                network: None,
                state: ConnectionState::Disconnected {
                    error: "reset".into(),
                    retry_in: Duration::from_secs(4),
                },
            }
        );
    }
}
