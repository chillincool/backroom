//! Platform-neutral chat model shared by every front end: networks, buffers
//! (channels, private conversations, the server buffer) and messages.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};

use crate::proto::Message;

/// soju network ID (`BOUNCER NETWORK <netid>`).
pub type NetworkId = String;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkInfo {
    pub id: NetworkId,
    /// Attributes from soju: `name`, `state`, `host`, `nickname`, `error`, …
    pub attributes: BTreeMap<String, String>,
}

impl NetworkInfo {
    pub fn name(&self) -> &str {
        self.attributes
            .get("name")
            .or_else(|| self.attributes.get("host"))
            .map(String::as_str)
            .unwrap_or(&self.id)
    }

    /// soju's view of the upstream connection: `connected`, `connecting` or
    /// `disconnected`.
    pub fn upstream_state(&self) -> Option<&str> {
        self.attributes.get("state").map(String::as_str)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BufferKind {
    /// Notices and replies from the server itself. Named [`SERVER_BUFFER`].
    Server,
    Channel,
    /// A private conversation, named after the other user's nick.
    Query,
}

/// A channel member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub nick: String,
    /// Membership prefixes, highest first (`@`, `+`, `@+`, …); empty if none.
    pub prefix: String,
    /// Marked away (from `away-notify` or WHO); false if unknown.
    pub away: bool,
}

/// Name of the per-network server buffer.
pub const SERVER_BUFFER: &str = "*";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageKind {
    Privmsg,
    Notice,
    /// CTCP ACTION (`/me`).
    Action,
    Join,
    Part,
    Kick,
    Topic,
    /// Someone disconnected; the text is their reason. Shown in each
    /// channel they shared with us.
    Quit,
    /// Someone changed nick; `sender` is the old nick, the text the new one.
    Nick,
    /// Channel modes changed; the text is the modes and their arguments
    /// (`+o alice`).
    Mode,
    /// An invitation. In the server buffer: we were invited, the text is the
    /// channel. In a channel (`invite-notify`): the text is who was invited.
    Invite,
    /// A server reply (numeric), as text.
    Reply,
    /// An error reply (`4xx`/`5xx` numeric), as text.
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatMessage {
    /// IRCv3 `msgid`, when the server provides one.
    pub id: Option<String>,
    /// `server-time` if present, otherwise the time it was received.
    pub time: DateTime<Utc>,
    pub sender: String,
    pub kind: MessageKind,
    /// Text as sent, mIRC formatting codes included. For JOIN, empty; for
    /// PART/KICK the reason; for TOPIC the new topic.
    pub text: String,
    /// Sent by this user (including echoes from other devices).
    pub own: bool,
}

/// What a WHOIS found out about someone.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WhoisInfo {
    pub nick: String,
    pub user: Option<String>,
    pub host: Option<String>,
    pub realname: Option<String>,
    pub server: Option<String>,
    pub server_info: Option<String>,
    /// Services account (`RPL_WHOISACCOUNT`).
    pub account: Option<String>,
    /// Channels, with membership prefixes (`@#ops`).
    pub channels: Vec<String>,
    pub idle_secs: Option<u64>,
    pub signon: Option<DateTime<Utc>>,
    /// Away message, if away.
    pub away: Option<String>,
    pub operator: bool,
    /// Connected with TLS (`RPL_WHOISSECURE`).
    pub secure: bool,
    /// Other lines (actual host, certificate, bot, …) as text.
    pub other: Vec<String>,
}

/// One channel from a `LIST` reply (`RPL_LIST`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelListing {
    pub name: String,
    /// Visible users, as the server counts them.
    pub users: u32,
    /// Topic, mIRC formatting included; often prefixed with modes like `[+nt]`.
    pub topic: String,
}

/// Network-specific facts needed to route messages.
#[derive(Debug, Clone)]
pub struct RouteContext<'a> {
    pub own_nick: &'a str,
    /// Channel prefixes from ISUPPORT `CHANTYPES` (default `#&`).
    pub chantypes: &'a str,
}

impl RouteContext<'_> {
    pub fn is_channel(&self, name: &str) -> bool {
        name.chars()
            .next()
            .is_some_and(|c| self.chantypes.contains(c))
    }

    pub fn is_me(&self, nick: &str) -> bool {
        nick.eq_ignore_ascii_case(self.own_nick)
    }
}

/// Buffer lookup key: names compare case-insensitively.
pub fn buffer_key(name: &str) -> String {
    name.to_ascii_lowercase()
}

/// Which buffer a message belongs to, and the message itself. Returns `None`
/// for traffic that isn't shown in a buffer (numerics, CTCP queries, …).
pub fn route(msg: &Message, ctx: &RouteContext<'_>) -> Option<(String, BufferKind, ChatMessage)> {
    let sender = msg.source_nick().unwrap_or("").to_owned();
    // A source without `!user@host` is the server (or soju itself).
    let from_server = msg.source.as_deref().is_none_or(|s| !s.contains('!'));
    let own = ctx.is_me(&sender);
    let chat = |kind, text: &str| ChatMessage {
        id: msg.tag("msgid").map(str::to_owned),
        time: msg
            .tag("time")
            .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
            .map(|t| t.with_timezone(&Utc))
            .unwrap_or_else(Utc::now),
        sender: sender.clone(),
        kind,
        text: text.to_owned(),
        own,
    };

    match msg.command.as_str() {
        "PRIVMSG" | "NOTICE" => {
            let target = msg.param(0)?;
            let text = msg.param(1).unwrap_or("");
            let (kind, text) = match text.strip_prefix('\u{1}') {
                Some(ctcp) => {
                    let ctcp = ctcp.strip_suffix('\u{1}').unwrap_or(ctcp);
                    match ctcp.strip_prefix("ACTION") {
                        Some(action) if msg.command == "PRIVMSG" => (
                            MessageKind::Action,
                            action.strip_prefix(' ').unwrap_or(action),
                        ),
                        // Other CTCP requests and replies aren't chat.
                        _ => return None,
                    }
                }
                None if msg.command == "NOTICE" => (MessageKind::Notice, text),
                None => (MessageKind::Privmsg, text),
            };
            let (buffer, buffer_kind) = if ctx.is_channel(target) {
                (target, BufferKind::Channel)
            } else if from_server || target == "*" {
                (SERVER_BUFFER, BufferKind::Server)
            } else if own {
                // Our own message to someone (echo-message or another device).
                (target, BufferKind::Query)
            } else {
                (sender.as_str(), BufferKind::Query)
            };
            Some((buffer.to_owned(), buffer_kind, chat(kind, text)))
        }
        "JOIN" => {
            let channel = msg.param(0)?;
            Some((
                channel.to_owned(),
                BufferKind::Channel,
                chat(MessageKind::Join, ""),
            ))
        }
        "PART" => {
            let channel = msg.param(0)?;
            let reason = msg.param(1).unwrap_or("");
            Some((
                channel.to_owned(),
                BufferKind::Channel,
                chat(MessageKind::Part, reason),
            ))
        }
        "KICK" => {
            let channel = msg.param(0)?;
            let kicked = msg.param(1)?;
            let reason = msg.param(2).unwrap_or("");
            let mut m = chat(MessageKind::Kick, format!("{kicked} {reason}").trim_end());
            m.own = ctx.is_me(kicked) || own;
            Some((channel.to_owned(), BufferKind::Channel, m))
        }
        "TOPIC" => {
            let channel = msg.param(0)?;
            Some((
                channel.to_owned(),
                BufferKind::Channel,
                chat(MessageKind::Topic, msg.param(1).unwrap_or("")),
            ))
        }
        // MODE <channel> <modes> [args…]; user modes aren't shown.
        "MODE" => {
            let channel = msg.param(0).filter(|t| ctx.is_channel(t))?;
            let modes = msg.params[1..].join(" ");
            Some((
                channel.to_owned(),
                BufferKind::Channel,
                chat(MessageKind::Mode, &modes),
            ))
        }
        // INVITE <nick> <channel>
        "INVITE" => {
            let (target, channel) = (msg.param(0)?, msg.param(1)?);
            if ctx.is_me(target) {
                Some((
                    SERVER_BUFFER.to_owned(),
                    BufferKind::Server,
                    chat(MessageKind::Invite, channel),
                ))
            } else {
                // invite-notify: someone else was invited to our channel.
                Some((
                    channel.to_owned(),
                    BufferKind::Channel,
                    chat(MessageKind::Invite, target),
                ))
            }
        }
        _ => None,
    }
}

/// A QUIT or NICK as a chat line; the caller decides which buffers show it
/// (the channels the user shared with us, and our conversation with them).
pub fn quit_or_nick(msg: &Message, ctx: &RouteContext<'_>) -> Option<ChatMessage> {
    let kind = match msg.command.as_str() {
        "QUIT" => MessageKind::Quit,
        "NICK" => MessageKind::Nick,
        _ => return None,
    };
    let sender = msg.source_nick()?.to_owned();
    Some(ChatMessage {
        id: msg.tag("msgid").map(str::to_owned),
        time: msg
            .tag("time")
            .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
            .map(|t| t.with_timezone(&Utc))
            .unwrap_or_else(Utc::now),
        own: ctx.is_me(&sender),
        sender,
        kind,
        text: msg.param(0).unwrap_or("").to_owned(),
    })
}

/// Where a numeric reply goes and how it reads. Errors about a channel or
/// nick we have a buffer for go there (`has_buffer` takes a name); the rest
/// go to the server buffer. The text leaves out our own nick and the
/// buffer's name: `404 me #c :Cannot send to channel` reads "Cannot send to
/// channel" in #c; `433 me * bob :Nickname is already in use` reads "bob:
/// Nickname is already in use".
pub fn route_reply(
    msg: &Message,
    has_buffer: impl Fn(&str) -> Option<BufferKind>,
) -> Option<(String, BufferKind, ChatMessage)> {
    let code: u16 = msg.command.parse().ok()?;
    let error = (400..600).contains(&code);
    let middle = msg
        .params
        .get(1..msg.params.len().saturating_sub(1))
        .unwrap_or(&[]);
    let mut words: Vec<&str> = middle
        .iter()
        .map(String::as_str)
        .filter(|w| *w != "*")
        .collect();
    // The first word names the channel or nick an error is about.
    let target = words
        .first()
        .filter(|_| error)
        .and_then(|name| match has_buffer(name) {
            Some(BufferKind::Server) | None => None,
            Some(kind) => Some((name.to_string(), kind)),
        });
    if target.is_some() {
        words.remove(0);
    }
    let trailing = if msg.params.len() > 1 {
        msg.params.last().map(String::as_str).unwrap_or("")
    } else {
        ""
    };
    let text = match (words.is_empty(), trailing.is_empty()) {
        (true, _) => trailing.to_owned(),
        (false, true) => words.join(" "),
        (false, false) => format!("{}: {trailing}", words.join(" ")),
    };
    let (buffer, kind) = target.unwrap_or((SERVER_BUFFER.to_owned(), BufferKind::Server));
    Some((
        buffer,
        kind,
        ChatMessage {
            id: msg.tag("msgid").map(str::to_owned),
            time: msg
                .tag("time")
                .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
                .map(|t| t.with_timezone(&Utc))
                .unwrap_or_else(Utc::now),
            sender: msg.source.clone().unwrap_or_default(),
            kind: if error {
                MessageKind::Error
            } else {
                MessageKind::Reply
            },
            text,
            own: false,
        },
    ))
}

/// Formats a timestamp the way IRCv3 `server-time` and `CHATHISTORY` expect.
pub fn irc_timestamp(time: DateTime<Utc>) -> String {
    time.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CTX: RouteContext<'static> = RouteContext {
        own_nick: "me",
        chantypes: "#&",
    };

    fn route_line(line: &str) -> Option<(String, BufferKind, ChatMessage)> {
        route(&Message::parse(line).unwrap(), &CTX)
    }

    #[test]
    fn modes_and_invites() {
        let (buf, kind, m) = route_line(":op!u@h MODE #c +ov alice bob").unwrap();
        assert_eq!(
            (buf.as_str(), kind, m.kind),
            ("#c", BufferKind::Channel, MessageKind::Mode)
        );
        assert_eq!(m.text, "+ov alice bob");
        assert!(
            route_line(":me MODE me :+i").is_none(),
            "user modes aren't shown"
        );

        let (buf, kind, m) = route_line(":op!u@h INVITE me #secret").unwrap();
        assert_eq!((buf.as_str(), kind), (SERVER_BUFFER, BufferKind::Server));
        assert_eq!(
            (m.kind, m.text.as_str(), m.sender.as_str()),
            (MessageKind::Invite, "#secret", "op")
        );
        let (buf, _, m) = route_line(":op!u@h INVITE bob #c").unwrap();
        assert_eq!((buf.as_str(), m.text.as_str()), ("#c", "bob"));
    }

    #[test]
    fn replies_and_errors() {
        let reply = |line: &str| {
            let has = |name: &str| (name == "#c").then_some(BufferKind::Channel);
            route_reply(&Message::parse(line).unwrap(), has).unwrap()
        };
        let (buf, kind, m) = reply(":srv 404 me #c :Cannot send to channel");
        assert_eq!((buf.as_str(), kind), ("#c", BufferKind::Channel));
        assert_eq!(
            (m.kind, m.text.as_str()),
            (MessageKind::Error, "Cannot send to channel")
        );

        let (buf, _, m) = reply(":srv 433 me * bob :Nickname is already in use");
        assert_eq!(buf, SERVER_BUFFER);
        assert_eq!(m.text, "bob: Nickname is already in use");

        let (buf, _, m) = reply(":srv 403 me #nope :No such channel");
        assert_eq!(
            (buf.as_str(), m.text.as_str()),
            (SERVER_BUFFER, "#nope: No such channel")
        );

        let (_, _, m) = reply(":srv 372 me :- Welcome to the network");
        assert_eq!(
            (m.kind, m.text.as_str()),
            (MessageKind::Reply, "- Welcome to the network")
        );
        assert!(route_reply(&Message::parse(":srv PRIVMSG me :hi").unwrap(), |_| None).is_none());
    }

    #[test]
    fn channel_message_with_tags() {
        let (buf, kind, m) =
            route_line("@time=2026-10-24T12:00:00.000Z;msgid=abc :alice!a@h PRIVMSG #chan :hello")
                .unwrap();
        assert_eq!((buf.as_str(), kind), ("#chan", BufferKind::Channel));
        assert_eq!(m.id.as_deref(), Some("abc"));
        assert_eq!(irc_timestamp(m.time), "2026-10-24T12:00:00.000Z");
        assert_eq!(
            (m.sender.as_str(), m.kind, m.text.as_str(), m.own),
            ("alice", MessageKind::Privmsg, "hello", false)
        );
    }

    #[test]
    fn private_messages_go_to_the_other_person() {
        let (buf, kind, m) = route_line(":alice!a@h PRIVMSG me :hi").unwrap();
        assert_eq!(
            (buf.as_str(), kind, m.own),
            ("alice", BufferKind::Query, false)
        );
        // Our own message to alice (echo-message), even with different case.
        let (buf, kind, m) = route_line(":ME!m@h PRIVMSG alice :hey").unwrap();
        assert_eq!(
            (buf.as_str(), kind, m.own),
            ("alice", BufferKind::Query, true)
        );
    }

    #[test]
    fn actions_and_ctcp() {
        let (_, _, m) = route_line(":alice!a@h PRIVMSG #c :\u{1}ACTION waves\u{1}").unwrap();
        assert_eq!((m.kind, m.text.as_str()), (MessageKind::Action, "waves"));
        let (_, _, m) = route_line(":alice!a@h PRIVMSG #c :\u{1}ACTION\u{1}").unwrap();
        assert_eq!((m.kind, m.text.as_str()), (MessageKind::Action, ""));
        assert!(route_line(":alice!a@h PRIVMSG me :\u{1}VERSION\u{1}").is_none());
        assert!(route_line(":alice!a@h NOTICE me :\u{1}VERSION x\u{1}").is_none());
    }

    #[test]
    fn server_notices_go_to_the_server_buffer() {
        let (buf, kind, m) = route_line(":irc.example NOTICE * :Looking up your hostname").unwrap();
        assert_eq!(
            (buf.as_str(), kind, m.kind),
            (SERVER_BUFFER, BufferKind::Server, MessageKind::Notice)
        );
        let (buf, _, _) = route_line(":BouncerServ!BouncerServ@BouncerServ NOTICE me :hi").unwrap();
        assert_eq!(buf, "BouncerServ");
    }

    #[test]
    fn membership_and_topic() {
        let (buf, _, m) = route_line(":me!m@h JOIN #c").unwrap();
        assert_eq!(
            (buf.as_str(), m.kind, m.own),
            ("#c", MessageKind::Join, true)
        );
        let (_, _, m) = route_line(":bob!b@h PART #c :bye").unwrap();
        assert_eq!((m.kind, m.text.as_str()), (MessageKind::Part, "bye"));
        let (_, _, m) = route_line(":op!o@h KICK #c me :spam").unwrap();
        assert_eq!(
            (m.kind, m.text.as_str(), m.own),
            (MessageKind::Kick, "me spam", true)
        );
        let (_, _, m) = route_line(":op!o@h TOPIC #c :new topic").unwrap();
        assert_eq!((m.kind, m.text.as_str()), (MessageKind::Topic, "new topic"));
        assert!(route_line(":srv 001 me :Welcome").is_none());
    }

    #[test]
    fn network_names_fall_back() {
        let mut n = NetworkInfo {
            id: "7".into(),
            attributes: BTreeMap::new(),
        };
        assert_eq!(n.name(), "7");
        n.attributes.insert("host".into(), "irc.libera.chat".into());
        assert_eq!(n.name(), "irc.libera.chat");
        n.attributes.insert("name".into(), "Libera".into());
        assert_eq!(n.name(), "Libera");
    }
}
