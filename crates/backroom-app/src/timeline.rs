//! What a buffer's message list shows: messages, joins, parts and quits
//! (alone, folded into runs or hidden), a separator where the day changes
//! and the New line. Ignored people's messages are left out.

use std::collections::HashSet;
use std::time::{Duration, SystemTime};

use backroom_ffi::{ChatMessage, MessageKind};
use chrono::{DateTime, Local, NaiveDate};

use crate::buffer::{is_chat, is_presence, stable_id};
use crate::prefs::PresenceStyle;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimelineItem {
    /// `continued`: the previous line is a message from the same sender, so
    /// the nick can be left out.
    Message {
        message: ChatMessage,
        continued: bool,
    },
    Presence(Vec<ChatMessage>),
    Day(NaiveDate),
    /// Above the first unread message.
    UnreadLine,
}

impl TimelineItem {
    pub fn id(&self) -> String {
        match self {
            TimelineItem::Message { message, .. } => stable_id(message),
            TimelineItem::Presence(run) => {
                format!(
                    "presence-{}",
                    run.first().map(stable_id).unwrap_or_default()
                )
            }
            TimelineItem::Day(date) => format!("day-{date}"),
            TimelineItem::UnreadLine => "unread-line".into(),
        }
    }
}

/// A run of messages from one sender breaks after this long a pause.
pub const RUN_GAP: Duration = Duration::from_secs(30 * 60);

/// The local calendar day of a time.
pub fn local_day(time: SystemTime) -> NaiveDate {
    DateTime::<Local>::from(time).date_naive()
}

pub fn build(
    messages: &[ChatMessage],
    presence: PresenceStyle,
    unread_after: Option<SystemTime>,
    is_ignored: &dyn Fn(&str) -> bool,
    day_of: &dyn Fn(SystemTime) -> NaiveDate,
) -> Vec<TimelineItem> {
    let mut line_due = unread_after.is_some();
    let mut items = Vec::new();
    let mut run: Vec<ChatMessage> = Vec::new();
    let mut day: Option<NaiveDate> = None;
    // The last message shown with its sender, while a run can continue.
    let mut previous: Option<&ChatMessage> = None;

    let flush = |items: &mut Vec<TimelineItem>, run: &mut Vec<ChatMessage>| {
        if !run.is_empty() {
            items.push(TimelineItem::Presence(std::mem::take(run)));
        }
    };

    for message in messages.iter().filter(|m| m.own || !is_ignored(&m.sender)) {
        let start = day_of(message.time);
        if Some(start) != day {
            flush(&mut items, &mut run);
            // No separator above the first day: the header says where history starts.
            if day.is_some() {
                items.push(TimelineItem::Day(start));
            }
            day = Some(start);
            previous = None;
        }
        if line_due
            && unread_after.is_some_and(|after| message.time > after)
            && !message.own
            && is_chat(message.kind)
        {
            flush(&mut items, &mut run);
            items.push(TimelineItem::UnreadLine);
            line_due = false;
            previous = None;
        }
        if is_presence(message.kind) {
            match presence {
                PresenceStyle::Hide => continue,
                PresenceStyle::Fold => run.push(message.clone()),
                PresenceStyle::Show => items.push(TimelineItem::Message {
                    message: message.clone(),
                    continued: false,
                }),
            }
            previous = None;
            continue;
        }
        flush(&mut items, &mut run);
        let continued = message.kind == MessageKind::Privmsg
            && previous.is_some_and(|p| {
                p.kind == MessageKind::Privmsg
                    && p.sender.to_lowercase() == message.sender.to_lowercase()
                    && message.time.duration_since(p.time).unwrap_or_default() < RUN_GAP
            });
        items.push(TimelineItem::Message {
            message: message.clone(),
            continued,
        });
        previous = Some(message);
    }
    flush(&mut items, &mut run);
    items
}

/// "→ alice, bob joined · ← carol left", long lists shortened.
pub fn summary(run: &[ChatMessage]) -> String {
    fn unique<'a>(names: impl Iterator<Item = &'a str>) -> Vec<&'a str> {
        let mut seen = HashSet::new();
        names.filter(|n| seen.insert(n.to_lowercase())).collect()
    }
    fn list(names: &[&str]) -> String {
        const SHOWN: usize = 4;
        if names.len() <= SHOWN + 1 {
            match names {
                [] => String::new(),
                [one] => (*one).to_owned(),
                [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
            }
        } else {
            format!(
                "{} and {} others",
                names[..SHOWN].join(", "),
                names.len() - SHOWN
            )
        }
    }
    let joined = unique(
        run.iter()
            .filter(|m| m.kind == MessageKind::Join)
            .map(|m| m.sender.as_str()),
    );
    let left = unique(
        run.iter()
            .filter(|m| matches!(m.kind, MessageKind::Part | MessageKind::Quit))
            .map(|m| m.sender.as_str()),
    );
    let mut parts = Vec::new();
    if !joined.is_empty() {
        parts.push(format!("→ {} joined", list(&joined)));
    }
    if !left.is_empty() {
        parts.push(format!("← {} left", list(&left)));
    }
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::tests::msg;

    fn utc_day(t: SystemTime) -> NaiveDate {
        DateTime::<chrono::Utc>::from(t).date_naive()
    }

    #[test]
    fn folds_presence_and_continues_runs() {
        let messages = vec![
            msg(10, "a", MessageKind::Privmsg, "1"),
            msg(20, "A", MessageKind::Privmsg, "2"),
            msg(30, "b", MessageKind::Join, ""),
            msg(40, "c", MessageKind::Quit, ""),
            msg(50, "a", MessageKind::Privmsg, "3"),
        ];
        let items = build(&messages, PresenceStyle::Fold, None, &|_| false, &utc_day);
        assert_eq!(items.len(), 4);
        assert!(matches!(
            &items[1],
            TimelineItem::Message {
                continued: true,
                ..
            }
        ));
        let TimelineItem::Presence(run) = &items[2] else {
            panic!("{items:?}")
        };
        assert_eq!(summary(run), "→ b joined · ← c left");
        assert!(matches!(
            &items[3],
            TimelineItem::Message {
                continued: false,
                ..
            }
        ));

        let hidden = build(&messages, PresenceStyle::Hide, None, &|_| false, &utc_day);
        assert_eq!(hidden.len(), 3);
    }

    #[test]
    fn day_separators_and_unread_line() {
        let day = 86_400;
        let messages = vec![
            msg(10, "a", MessageKind::Privmsg, "1"),
            msg(day + 10, "b", MessageKind::Privmsg, "2"),
            msg(day + 20, "ignored", MessageKind::Privmsg, "3"),
        ];
        let after = Some(SystemTime::UNIX_EPOCH + Duration::from_secs(day));
        let items = build(
            &messages,
            PresenceStyle::Fold,
            after,
            &|n| n == "ignored",
            &utc_day,
        );
        assert!(matches!(items[1], TimelineItem::Day(_)));
        assert_eq!(items[2], TimelineItem::UnreadLine);
        assert_eq!(items.len(), 4);
    }

    #[test]
    fn long_summaries_are_shortened() {
        let run: Vec<_> = ["a", "b", "c", "d", "e", "f"]
            .iter()
            .map(|n| msg(1, n, MessageKind::Join, ""))
            .collect();
        assert_eq!(summary(&run), "→ a, b, c, d and 2 others joined");
    }
}
