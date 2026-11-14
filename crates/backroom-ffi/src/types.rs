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
    /// Someone disconnected; the text is their reason.
    Quit,
    /// Nick change: `sender` is the old nick, the text the new one.
    Nick,
    /// Channel modes and their arguments, e.g. `+o alice`.
    Mode,
    /// In the server buffer: we were invited, the text is the channel. In a
    /// channel: the text is who was invited.
    Invite,
    /// A server reply, as text.
    Reply,
    /// An error reply, as text.
    Error,
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
            model::MessageKind::Quit => MessageKind::Quit,
            model::MessageKind::Nick => MessageKind::Nick,
            model::MessageKind::Mode => MessageKind::Mode,
            model::MessageKind::Invite => MessageKind::Invite,
            model::MessageKind::Reply => MessageKind::Reply,
            model::MessageKind::Error => MessageKind::Error,
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

#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Record)]
pub struct Member {
    pub nick: String,
    /// Membership prefixes, highest first (`@`, `+`, `@+`, …); empty if none.
    pub prefix: String,
    /// Marked away; false if unknown.
    pub away: bool,
}

impl From<model::Member> for Member {
    fn from(m: model::Member) -> Self {
        Member {
            nick: m.nick,
            prefix: m.prefix,
            away: m.away,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum TypingState {
    Active,
    Paused,
    Done,
}

impl From<model::TypingState> for TypingState {
    fn from(s: model::TypingState) -> Self {
        match s {
            model::TypingState::Active => TypingState::Active,
            model::TypingState::Paused => TypingState::Paused,
            model::TypingState::Done => TypingState::Done,
        }
    }
}

impl From<TypingState> for model::TypingState {
    fn from(s: TypingState) -> Self {
        match s {
            TypingState::Active => model::TypingState::Active,
            TypingState::Paused => model::TypingState::Paused,
            TypingState::Done => model::TypingState::Done,
        }
    }
}

/// A message found by a search, and the buffer it belongs to.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SearchHit {
    pub buffer: String,
    pub message: ChatMessage,
}

/// An entry of a channel's ban list.
#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Record)]
pub struct BanEntry {
    pub mask: String,
    pub set_by: Option<String>,
    pub set_at: Option<SystemTime>,
}

impl From<model::BanEntry> for BanEntry {
    fn from(b: model::BanEntry) -> Self {
        BanEntry {
            mask: b.mask,
            set_by: b.set_by,
            set_at: b.set_at.map(Into::into),
        }
    }
}

/// What a WHOIS found out about someone.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct WhoisInfo {
    pub nick: String,
    pub user: Option<String>,
    pub host: Option<String>,
    pub realname: Option<String>,
    pub server: Option<String>,
    pub server_info: Option<String>,
    pub account: Option<String>,
    /// Channels, with membership prefixes (`@#ops`).
    pub channels: Vec<String>,
    pub idle: Option<Duration>,
    pub signon: Option<SystemTime>,
    /// Away message, if away.
    pub away: Option<String>,
    pub operator: bool,
    pub secure: bool,
    pub other: Vec<String>,
}

impl From<model::WhoisInfo> for WhoisInfo {
    fn from(w: model::WhoisInfo) -> Self {
        WhoisInfo {
            nick: w.nick,
            user: w.user,
            host: w.host,
            realname: w.realname,
            server: w.server,
            server_info: w.server_info,
            account: w.account,
            channels: w.channels,
            idle: w.idle_secs.map(Duration::from_secs),
            signon: w.signon.map(Into::into),
            away: w.away,
            operator: w.operator,
            secure: w.secure,
            other: w.other,
        }
    }
}

/// A run of message text in one mIRC style. Colours are `0xRRGGBB`.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FormattedSpan {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
    pub monospace: bool,
    /// Swap foreground and background.
    pub reverse: bool,
    pub fg: Option<u32>,
    pub bg: Option<u32>,
}

impl From<backroom_core::proto::formatting::Span> for FormattedSpan {
    fn from(s: backroom_core::proto::formatting::Span) -> Self {
        let st = s.style;
        FormattedSpan {
            text: s.text,
            bold: st.bold,
            italic: st.italic,
            underline: st.underline,
            strikethrough: st.strikethrough,
            monospace: st.monospace,
            reverse: st.reverse,
            fg: st.fg,
            bg: st.bg,
        }
    }
}

/// One channel from the network's channel list.
#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Record)]
pub struct ChannelListing {
    pub name: String,
    pub users: u32,
    /// Topic, mIRC formatting included.
    pub topic: String,
}

impl From<model::ChannelListing> for ChannelListing {
    fn from(c: model::ChannelListing) -> Self {
        ChannelListing {
            name: c.name,
            users: c.users,
            topic: c.topic,
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
    /// We left the channel, or the conversation was closed.
    BufferRemoved {
        network: String,
        name: String,
    },
    /// A channel's full member list, highest rank first.
    Members {
        network: String,
        channel: String,
        members: Vec<Member>,
    },
    /// A channel's topic, when joining and on changes; nil if none is set.
    Topic {
        network: String,
        channel: String,
        topic: Option<String>,
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
    /// Part of the channel list answering `list_channels`; `done` marks
    /// the last chunk.
    ChannelList {
        network: String,
        channels: Vec<ChannelListing>,
        done: bool,
    },
    /// Messages matching a search, oldest first; `error` if it failed.
    SearchResults {
        network: String,
        target: String,
        text: String,
        results: Vec<SearchHit>,
        error: Option<String>,
    },
    /// Someone's typing state in a buffer.
    Typing {
        network: String,
        buffer: String,
        nick: String,
        state: TypingState,
    },
    /// Whether someone we talk with privately is online (MONITOR).
    Presence {
        network: String,
        nick: String,
        online: bool,
    },
    /// A channel's modes, e.g. `+ntk` with arguments `["key"]`.
    ChannelModes {
        network: String,
        channel: String,
        modes: String,
        params: Vec<String>,
    },
    /// A channel's ban list.
    BanList {
        network: String,
        channel: String,
        bans: Vec<BanEntry>,
    },
    /// Our nick on a network, after registering and on every change.
    OwnNick {
        network: String,
        nick: String,
    },
    /// Whether we're marked away on a network.
    Away {
        network: String,
        away: bool,
    },
    /// A complete WHOIS reply.
    Whois {
        network: String,
        info: WhoisInfo,
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
            E::BufferRemoved { network, name } => SessionEvent::BufferRemoved { network, name },
            E::Members {
                network,
                channel,
                members,
            } => SessionEvent::Members {
                network,
                channel,
                members: members.into_iter().map(Into::into).collect(),
            },
            E::Topic {
                network,
                channel,
                topic,
            } => SessionEvent::Topic {
                network,
                channel,
                topic,
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
            E::ChannelList {
                network,
                channels,
                done,
            } => SessionEvent::ChannelList {
                network,
                channels: channels.into_iter().map(Into::into).collect(),
                done,
            },
            E::SearchResults {
                network,
                target,
                text,
                results,
                error,
            } => SessionEvent::SearchResults {
                network,
                target,
                text,
                results: results
                    .into_iter()
                    .map(|(buffer, message)| SearchHit {
                        buffer,
                        message: message.into(),
                    })
                    .collect(),
                error,
            },
            E::Typing {
                network,
                buffer,
                nick,
                state,
            } => SessionEvent::Typing {
                network,
                buffer,
                nick,
                state: state.into(),
            },
            E::Presence {
                network,
                nick,
                online,
            } => SessionEvent::Presence {
                network,
                nick,
                online,
            },
            E::ChannelModes {
                network,
                channel,
                modes,
                params,
            } => SessionEvent::ChannelModes {
                network,
                channel,
                modes,
                params,
            },
            E::BanList {
                network,
                channel,
                bans,
            } => SessionEvent::BanList {
                network,
                channel,
                bans: bans.into_iter().map(Into::into).collect(),
            },
            E::OwnNick { network, nick } => SessionEvent::OwnNick { network, nick },
            E::Away { network, away } => SessionEvent::Away { network, away },
            E::Whois { network, info } => SessionEvent::Whois {
                network,
                info: info.into(),
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
