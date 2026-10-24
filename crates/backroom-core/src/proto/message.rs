//! IRC message parsing and serialization, including IRCv3 message tags.
//!
//! Grammar follows <https://modern.ircdocs.horse/#messages> and
//! <https://ircv3.net/specs/extensions/message-tags>.

use std::fmt;

/// A single IRC message.
///
/// `command` is stored upper-cased. Tags keep their wire order; a tag without
/// a value is stored with an empty value, which the spec treats as equivalent.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Message {
    pub tags: Vec<(String, String)>,
    pub source: Option<String>,
    pub command: String,
    pub params: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    #[error("empty message")]
    Empty,
    #[error("message has tags or a source but no command")]
    MissingCommand,
}

impl Message {
    pub fn new<C, I, P>(command: C, params: I) -> Self
    where
        C: Into<String>,
        I: IntoIterator<Item = P>,
        P: Into<String>,
    {
        Message {
            tags: Vec::new(),
            source: None,
            command: command.into().to_ascii_uppercase(),
            params: params.into_iter().map(Into::into).collect(),
        }
    }

    pub fn with_tag(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.tags.push((key.into(), value.into()));
        self
    }

    /// Parses one line. A trailing CR and/or LF is tolerated and stripped.
    pub fn parse(line: &str) -> Result<Message, ParseError> {
        let mut rest = line.trim_end_matches(['\r', '\n']);
        if rest.trim_matches(' ').is_empty() {
            return Err(ParseError::Empty);
        }

        let mut msg = Message::default();

        if let Some(after) = rest.strip_prefix('@') {
            let (tags, tail) = split_word(after);
            msg.tags = parse_tags(tags);
            rest = tail;
        }

        rest = rest.trim_start_matches(' ');
        if let Some(after) = rest.strip_prefix(':') {
            let (source, tail) = split_word(after);
            msg.source = Some(source.to_owned());
            rest = tail;
        }

        rest = rest.trim_start_matches(' ');
        let (command, mut rest) = split_word(rest);
        if command.is_empty() {
            return Err(ParseError::MissingCommand);
        }
        msg.command = command.to_ascii_uppercase();

        loop {
            rest = rest.trim_start_matches(' ');
            if rest.is_empty() {
                break;
            }
            if let Some(trailing) = rest.strip_prefix(':') {
                msg.params.push(trailing.to_owned());
                break;
            }
            let (param, tail) = split_word(rest);
            msg.params.push(param.to_owned());
            rest = tail;
        }

        Ok(msg)
    }

    /// Returns the value of a tag, or `None` if the tag is absent.
    pub fn tag(&self, key: &str) -> Option<&str> {
        self.tags
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    pub fn param(&self, i: usize) -> Option<&str> {
        self.params.get(i).map(String::as_str)
    }

    /// The nickname part of the source (`nick!user@host`).
    pub fn source_nick(&self) -> Option<&str> {
        let source = self.source.as_deref()?;
        Some(source.split(['!', '@']).next().unwrap_or(source))
    }
}

impl fmt::Display for Message {
    /// Serializes without a trailing CRLF; transports add framing.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.tags.is_empty() {
            f.write_str("@")?;
            for (i, (key, value)) in self.tags.iter().enumerate() {
                if i > 0 {
                    f.write_str(";")?;
                }
                f.write_str(key)?;
                if !value.is_empty() {
                    f.write_str("=")?;
                    f.write_str(&escape_tag_value(value))?;
                }
            }
            f.write_str(" ")?;
        }
        if let Some(source) = &self.source {
            write!(f, ":{source} ")?;
        }
        f.write_str(&self.command)?;
        let last = self.params.len().saturating_sub(1);
        for (i, param) in self.params.iter().enumerate() {
            let needs_colon =
                i == last && (param.is_empty() || param.contains(' ') || param.starts_with(':'));
            if needs_colon {
                write!(f, " :{param}")?;
            } else {
                write!(f, " {param}")?;
            }
        }
        Ok(())
    }
}

fn split_word(s: &str) -> (&str, &str) {
    match s.find(' ') {
        Some(i) => (&s[..i], &s[i + 1..]),
        None => (s, ""),
    }
}

fn parse_tags(s: &str) -> Vec<(String, String)> {
    s.split(';')
        .filter(|t| !t.is_empty())
        .map(|t| match t.split_once('=') {
            Some((k, v)) => (k.to_owned(), unescape_tag_value(v)),
            None => (t.to_owned(), String::new()),
        })
        .collect()
}

/// Escapes a tag value per the message-tags spec. Also used for
/// `soju.im/bouncer-networks` attributes, which share the encoding.
pub fn escape_tag_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            ';' => out.push_str("\\:"),
            ' ' => out.push_str("\\s"),
            '\\' => out.push_str("\\\\"),
            '\r' => out.push_str("\\r"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out
}

/// Reverses [`escape_tag_value`]. Unknown escapes drop the backslash and a
/// trailing lone backslash is removed, as the spec requires.
pub fn unescape_tag_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some(':') => out.push(';'),
            Some('s') => out.push(' '),
            Some('\\') => out.push('\\'),
            Some('r') => out.push('\r'),
            Some('n') => out.push('\n'),
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_command() {
        let m = Message::parse("PING :irc.example.com").unwrap();
        assert!(m.tags.is_empty());
        assert_eq!(m.source, None);
        assert_eq!(m.command, "PING");
        assert_eq!(m.params, ["irc.example.com"]);
    }

    #[test]
    fn parses_source_and_trailing() {
        let m = Message::parse(":nick!user@host PRIVMSG #chan :hello there").unwrap();
        assert_eq!(m.source.as_deref(), Some("nick!user@host"));
        assert_eq!(m.source_nick(), Some("nick"));
        assert_eq!(m.command, "PRIVMSG");
        assert_eq!(m.params, ["#chan", "hello there"]);
    }

    #[test]
    fn parses_tags_with_escapes_and_empty_values() {
        let m = Message::parse(
            r"@time=2026-10-23T12:00:00.000Z;msgid=abc;+draft/reply;k=a\sb\:c\\d\r\n :srv CMD",
        )
        .unwrap();
        assert_eq!(m.tag("time"), Some("2026-10-23T12:00:00.000Z"));
        assert_eq!(m.tag("msgid"), Some("abc"));
        assert_eq!(m.tag("+draft/reply"), Some(""));
        assert_eq!(m.tag("k"), Some("a b;c\\d\r\n"));
        assert_eq!(m.tag("missing"), None);
    }

    #[test]
    fn unescape_drops_unknown_escape_and_trailing_backslash() {
        assert_eq!(unescape_tag_value(r"a\bc"), "abc");
        assert_eq!(unescape_tag_value("abc\\"), "abc");
    }

    #[test]
    fn empty_trailing_and_colon_in_middle_param() {
        let m = Message::parse("CMD a:b :").unwrap();
        assert_eq!(m.params, ["a:b", ""]);
        let m = Message::parse("CMD a :b c :d").unwrap();
        assert_eq!(m.params, ["a", "b c :d"]);
    }

    #[test]
    fn tolerates_crlf_and_extra_spaces() {
        let m = Message::parse(":src  CMD   a  b\r\n").unwrap();
        assert_eq!(m.command, "CMD");
        assert_eq!(m.params, ["a", "b"]);
    }

    #[test]
    fn command_is_uppercased() {
        assert_eq!(Message::parse("privmsg x :y").unwrap().command, "PRIVMSG");
    }

    #[test]
    fn rejects_empty_and_commandless() {
        assert_eq!(Message::parse(""), Err(ParseError::Empty));
        assert_eq!(Message::parse("\r\n"), Err(ParseError::Empty));
        assert_eq!(Message::parse("@a=b "), Err(ParseError::MissingCommand));
        assert_eq!(Message::parse(":src"), Err(ParseError::MissingCommand));
    }

    #[test]
    fn parses_multiline_cap_ls() {
        let m = Message::parse(":soju CAP * LS * :sasl=PLAIN,OAUTHBEARER batch").unwrap();
        assert_eq!(m.params, ["*", "LS", "*", "sasl=PLAIN,OAUTHBEARER batch"]);
    }

    #[test]
    fn serializes_with_trailing_only_when_needed() {
        assert_eq!(Message::new("NICK", ["me"]).to_string(), "NICK me");
        assert_eq!(
            Message::new("PRIVMSG", ["#c", "hi there"]).to_string(),
            "PRIVMSG #c :hi there"
        );
        assert_eq!(Message::new("X", ["a", ""]).to_string(), "X a :");
        assert_eq!(Message::new("X", [":a"]).to_string(), "X ::a");
        assert_eq!(Message::new("CAP", ["END"]).to_string(), "CAP END");
    }

    #[test]
    fn round_trips_tags() {
        let m = Message::new("TAGMSG", ["#c"])
            .with_tag("label", "x;y z")
            .with_tag("+typing", "");
        let line = m.to_string();
        assert_eq!(line, r"@label=x\:y\sz;+typing TAGMSG #c");
        assert_eq!(Message::parse(&line).unwrap(), m);
    }

    #[test]
    fn round_trips_full_message() {
        let line = r"@batch=1;time=2026-01-01T00:00:00Z :n!u@h PRIVMSG #chan :hello :) world";
        assert_eq!(Message::parse(line).unwrap().to_string(), line);
    }
}
