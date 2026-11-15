//! Optional plain-text logs, the old-fashioned way: one file per channel
//! per day, `<folder>/<network>/<buffer>/2026-11-14.log`, in local time.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::Local;

use crate::model::{ChatMessage, MessageKind, buffer_key};

pub struct TextLog {
    dir: PathBuf,
}

impl TextLog {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        TextLog { dir: dir.into() }
    }

    /// Appends `msg` to today's file for `buffer` on network `network`
    /// (its display name).
    pub fn write(&self, network: &str, buffer: &str, msg: &ChatMessage) -> std::io::Result<()> {
        let time = msg.time.with_timezone(&Local);
        let path = self
            .dir
            .join(file_name(network))
            .join(file_name(&buffer_key(buffer)))
            .join(format!("{}.log", time.format("%Y-%m-%d")));
        let line = format!("[{}] {}\n", time.format("%H:%M:%S"), line(msg));
        append(&path, &line)
    }
}

fn append(path: &Path, line: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?
        .write_all(line.as_bytes())
}

/// A safe file or folder name: no path separators, no leading dot, no
/// control characters.
fn file_name(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    if out.starts_with('.') || out.is_empty() {
        out.insert(0, '_');
    }
    out
}

/// irssi-style text for a message, formatting codes kept as sent.
fn line(m: &ChatMessage) -> String {
    let (nick, text) = (&m.sender, &m.text);
    let with_reason = |verb: &str| {
        if text.is_empty() {
            format!("<-- {nick} {verb}")
        } else {
            format!("<-- {nick} {verb} ({text})")
        }
    };
    match m.kind {
        MessageKind::Privmsg => format!("<{nick}> {text}"),
        MessageKind::Action => format!("* {nick} {text}"),
        MessageKind::Notice => format!("-{nick}- {text}"),
        MessageKind::Join => format!("--> {nick} joined"),
        MessageKind::Part => with_reason("left"),
        MessageKind::Quit => with_reason("quit"),
        MessageKind::Kick => format!("<-- {nick} kicked: {text}"),
        MessageKind::Topic => format!("*** {nick} changed the topic to: {text}"),
        MessageKind::Nick => format!("*** {nick} is now known as {text}"),
        MessageKind::Mode => format!("*** {nick} sets mode {text}"),
        MessageKind::Invite => format!("*** {nick} invited {text}"),
        MessageKind::Reply | MessageKind::Error => format!("*** {text}"),
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    #[test]
    fn writes_one_file_per_buffer_and_day() {
        let dir = tempfile::tempdir().unwrap();
        let log = TextLog::new(dir.path());
        let time = Local.with_ymd_and_hms(2026, 11, 14, 9, 5, 7).unwrap();
        let mut m = ChatMessage {
            id: None,
            time: time.with_timezone(&chrono::Utc),
            sender: "erin".into(),
            kind: MessageKind::Privmsg,
            text: "hi there".into(),
            own: false,
        };
        log.write("Libera Chat", "#Rust", &m).unwrap();
        m.kind = MessageKind::Action;
        m.text = "waves".into();
        log.write("Libera Chat", "#rust", &m).unwrap();

        let file = dir.path().join("Libera Chat/#rust/2026-11-14.log");
        let text = std::fs::read_to_string(file).unwrap();
        assert_eq!(
            text,
            "[09:05:07] <erin> hi there\n[09:05:07] * erin waves\n"
        );
    }

    #[test]
    fn names_cannot_escape_the_folder() {
        assert_eq!(file_name("../etc"), "_.._etc");
        assert_eq!(file_name("a/b\\c:d"), "a_b_c_d");
        assert_eq!(file_name(""), "_");
        assert_eq!(file_name("#ch\u{7}an"), "#ch_an");
    }

    #[test]
    fn line_formats() {
        let m = |kind, text: &str| ChatMessage {
            id: None,
            time: chrono::Utc::now(),
            sender: "erin".into(),
            kind,
            text: text.into(),
            own: false,
        };
        assert_eq!(line(&m(MessageKind::Notice, "psst")), "-erin- psst");
        assert_eq!(line(&m(MessageKind::Part, "")), "<-- erin left");
        assert_eq!(line(&m(MessageKind::Quit, "bye")), "<-- erin quit (bye)");
        assert_eq!(
            line(&m(MessageKind::Nick, "erin2")),
            "*** erin is now known as erin2"
        );
    }
}
