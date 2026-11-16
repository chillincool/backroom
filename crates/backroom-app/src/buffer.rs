//! One buffer's state (a channel, a private conversation or the server
//! buffer) and one network's.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, SystemTime};

use backroom_ffi::{
    BufferKind, ChatMessage, ConnectionState, Member, MessageKind, Network, TypingState,
};
use serde::{Deserialize, Serialize};

use crate::mentions::MentionMatcher;

/// A buffer is identified by its network and case-folded name.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct BufferId {
    pub network: String,
    pub key: String,
}

impl BufferId {
    pub fn new(network: impl Into<String>, name: &str) -> Self {
        BufferId {
            network: network.into(),
            key: name.to_lowercase(),
        }
    }
}

/// Someone's latest typing notification in a buffer.
#[derive(Debug, Clone)]
pub struct Typing {
    pub nick: String,
    pub state: TypingState,
    pub since: SystemTime,
}

#[derive(Debug, Clone)]
pub struct BufferModel {
    pub id: BufferId,
    /// Name as the server spelled it; used as the IRC target.
    pub name: String,
    pub kind: BufferKind,
    messages: Vec<ChatMessage>,
    /// Everything up to here has been read (on any device).
    pub read_marker: Option<SystemTime>,
    /// Where the read marker was when the user last came to this buffer:
    /// the "New" line goes above the first message after it.
    pub unread_line: Option<SystemTime>,
    pub history_requested: bool,
    pub history_loading: bool,
    /// The server has no older messages.
    pub history_exhausted: bool,
    /// The user cleared the buffer (`/clear`); older history isn't loaded.
    pub cleared: bool,
    /// Channel topic, mIRC formatting included.
    pub topic: Option<String>,
    members: Vec<Member>,
    prefix_by_nick: HashMap<String, String>,
    /// Finds mentions of our nick and highlight words.
    pub mentions: MentionMatcher,
    /// Who's typing here, by case-folded nick.
    pub typing: HashMap<String, Typing>,
    /// A private conversation's partner is online (MONITOR); `None` if unknown.
    pub online: Option<bool>,
    /// A message to scroll to (its `stable_id`), e.g. from search.
    pub scroll_target: Option<String>,
    /// Lines sent from this buffer's composer, oldest first, for ↑/↓.
    pub sent_history: Vec<String>,
    known_ids: HashSet<String>,
}

impl BufferModel {
    pub fn new(id: BufferId, name: &str, kind: BufferKind) -> Self {
        BufferModel {
            id,
            name: name.to_owned(),
            kind,
            messages: Vec::new(),
            read_marker: None,
            unread_line: None,
            history_requested: false,
            history_loading: false,
            history_exhausted: false,
            cleared: false,
            topic: None,
            members: Vec::new(),
            prefix_by_nick: HashMap::new(),
            mentions: MentionMatcher::default(),
            typing: HashMap::new(),
            online: None,
            scroll_target: None,
            sent_history: Vec::new(),
            known_ids: HashSet::new(),
        }
    }

    pub fn messages(&self) -> &[ChatMessage] {
        &self.messages
    }

    /// Channel members, highest rank first.
    pub fn members(&self) -> &[Member] {
        &self.members
    }

    pub fn set_members(&mut self, members: Vec<Member>) {
        self.prefix_by_nick.clear();
        for m in &members {
            self.prefix_by_nick
                .entry(m.nick.to_lowercase())
                .or_insert_with(|| m.prefix.clone());
        }
        self.members = members;
    }

    /// A member's prefixes, highest first ("" if none); `None` if not a member.
    pub fn prefix(&self, nick: &str) -> Option<&str> {
        self.prefix_by_nick
            .get(&nick.to_lowercase())
            .map(String::as_str)
    }

    /// Messages from others after the read marker.
    pub fn unread(&self) -> usize {
        self.unread_counts().0
    }

    /// Unread messages that mention us (every unread message in a private
    /// conversation does).
    pub fn unread_mentions(&self) -> usize {
        self.unread_counts().1
    }

    /// (all, mentions)
    pub fn unread_counts(&self) -> (usize, usize) {
        let (mut all, mut mentioned) = (0, 0);
        for m in self.messages.iter().rev() {
            if self.read_marker.is_some_and(|marker| m.time <= marker) {
                break;
            }
            if m.own || !self.is_chat(m) || self.mentions.is_ignored(&m.sender) {
                continue;
            }
            all += 1;
            if self.kind == BufferKind::Query || self.is_mention(m) {
                mentioned += 1;
            }
        }
        (all, mentioned)
    }

    /// Counts towards unread: chat, and invitations to us.
    pub fn is_chat(&self, m: &ChatMessage) -> bool {
        is_chat(m.kind) || (m.kind == MessageKind::Invite && self.kind == BufferKind::Server)
    }

    /// Mentions our nick or a highlight word, or invites us.
    pub fn is_mention(&self, m: &ChatMessage) -> bool {
        if m.own {
            return false;
        }
        match m.kind {
            MessageKind::Privmsg | MessageKind::Notice | MessageKind::Action => {
                self.mentions.matches(&m.text)
            }
            MessageKind::Invite => self.kind == BufferKind::Server,
            _ => false,
        }
    }

    /// Nicks typing right now: active within 6 s, paused within 30 s (as
    /// the typing spec says to expire them).
    pub fn typing_now(&self, now: SystemTime) -> Vec<String> {
        let mut nicks: Vec<String> = self
            .typing
            .values()
            .filter(|t| {
                let age = now.duration_since(t.since).unwrap_or(Duration::ZERO);
                let limit = if t.state == TypingState::Active {
                    6
                } else {
                    30
                };
                t.state != TypingState::Done && age < Duration::from_secs(limit)
            })
            .map(|t| t.nick.clone())
            .collect();
        nicks.sort();
        nicks
    }

    /// Forgets the messages shown so far, on this device only.
    pub fn clear(&mut self) {
        self.messages.clear();
        self.cleared = true;
    }

    /// Adds messages, skipping ones already present (history and live
    /// messages overlap), keeping time order. Returns whether anything was
    /// added.
    pub fn merge(&mut self, incoming: Vec<ChatMessage>) -> bool {
        let fresh: Vec<ChatMessage> = incoming
            .into_iter()
            .filter(|m| m.id.as_ref().is_none_or(|id| !self.known_ids.contains(id)))
            .collect();
        if fresh.is_empty() {
            return false;
        }
        self.known_ids
            .extend(fresh.iter().filter_map(|m| m.id.clone()));
        let appends = self
            .messages
            .last()
            .is_none_or(|last| fresh.iter().all(|m| m.time >= last.time));
        self.messages.extend(fresh);
        if !appends {
            // Stable: equal times keep their arrival order.
            self.messages.sort_by_key(|m| m.time);
        }
        true
    }
}

/// Counts towards unread (not joins, parts and the like).
pub fn is_chat(kind: MessageKind) -> bool {
    matches!(
        kind,
        MessageKind::Privmsg | MessageKind::Notice | MessageKind::Action
    )
}

/// Comings and goings, folded into one line when the user wants.
pub fn is_presence(kind: MessageKind) -> bool {
    matches!(
        kind,
        MessageKind::Join | MessageKind::Part | MessageKind::Quit
    )
}

/// Identity in lists: the server's msgid, else what the message is.
pub fn stable_id(m: &ChatMessage) -> String {
    match &m.id {
        Some(id) => id.clone(),
        None => {
            let t = m
                .time
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default();
            format!(
                "{}.{:09}|{}|{:?}|{}",
                t.as_secs(),
                t.subsec_nanos(),
                m.sender,
                m.kind,
                m.text
            )
        }
    }
}

#[derive(Debug, Clone)]
pub struct NetworkState {
    pub network: Network,
    pub connection: ConnectionState,
    /// Buffers in the order they appeared; the server buffer first.
    pub buffers: Vec<BufferId>,
    /// Our nick there, once registered.
    pub nick: Option<String>,
    /// We're marked away there.
    pub away: bool,
}

impl NetworkState {
    pub fn new(network: Network) -> Self {
        NetworkState {
            network,
            connection: ConnectionState::Connecting,
            buffers: Vec::new(),
            nick: None,
            away: false,
        }
    }

    pub fn id(&self) -> &str {
        &self.network.id
    }
}

/// One line describing a connection state.
pub fn connection_summary(state: &ConnectionState) -> String {
    match state {
        ConnectionState::Connecting => "Connecting…".into(),
        ConnectionState::Connected => "Connected".into(),
        ConnectionState::Disconnected { error, retry_in } => {
            format!(
                "Disconnected ({error}); retrying in {} s",
                retry_in.as_secs_f64().round()
            )
        }
        ConnectionState::AuthFailed { reason } => format!("Login rejected: {reason}"),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn msg(secs: u64, sender: &str, kind: MessageKind, text: &str) -> ChatMessage {
        ChatMessage {
            id: None,
            time: SystemTime::UNIX_EPOCH + Duration::from_secs(secs),
            sender: sender.into(),
            kind,
            text: text.into(),
            own: false,
        }
    }

    #[test]
    fn merge_skips_known_ids_and_keeps_order() {
        let mut b = BufferModel::new(BufferId::new("1", "#a"), "#a", BufferKind::Channel);
        let mut late = msg(20, "x", MessageKind::Privmsg, "late");
        late.id = Some("b".into());
        let mut early = msg(10, "x", MessageKind::Privmsg, "early");
        early.id = Some("a".into());
        assert!(b.merge(vec![late.clone()]));
        assert!(b.merge(vec![early.clone(), late.clone()]));
        assert!(!b.merge(vec![late]));
        let texts: Vec<_> = b.messages().iter().map(|m| m.text.as_str()).collect();
        assert_eq!(texts, ["early", "late"]);
    }

    #[test]
    fn unread_counts_mentions_and_queries() {
        let mut b = BufferModel::new(BufferId::new("1", "#a"), "#a", BufferKind::Channel);
        b.mentions = MentionMatcher::new(Some("dave"), &[], &[]);
        b.merge(vec![
            msg(1, "x", MessageKind::Privmsg, "old"),
            msg(2, "x", MessageKind::Privmsg, "hi dave"),
            msg(3, "x", MessageKind::Join, ""),
            msg(4, "y", MessageKind::Privmsg, "hello"),
        ]);
        b.read_marker = Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1));
        assert_eq!(b.unread_counts(), (2, 1));
        b.kind = BufferKind::Query;
        assert_eq!(b.unread_counts(), (2, 2));
    }

    #[test]
    fn typing_expires() {
        let mut b = BufferModel::new(BufferId::new("1", "#a"), "#a", BufferKind::Channel);
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(100);
        b.typing.insert(
            "x".into(),
            Typing {
                nick: "x".into(),
                state: TypingState::Active,
                since: t0,
            },
        );
        assert_eq!(b.typing_now(t0 + Duration::from_secs(5)), ["x"]);
        assert!(b.typing_now(t0 + Duration::from_secs(7)).is_empty());
    }
}
