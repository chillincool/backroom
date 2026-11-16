//! What a line typed into the composer asks for, and Tab completion.

/// What a line typed into the composer asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComposerAction {
    Message(String),
    Action(String),
    /// Channels (comma-separated) and an optional key.
    Join {
        channels: String,
        key: Option<String>,
    },
    Part {
        channel: String,
        reason: Option<String>,
    },
    /// Leave and rejoin.
    Cycle {
        channel: String,
    },
    /// Open a private conversation, optionally sending a first message.
    Query {
        nick: String,
        text: Option<String>,
    },
    /// Open the channel browser.
    ListChannels,
    /// Forget this buffer's messages on this device (until reloaded).
    Clear,
    /// Leave the channel or close the conversation.
    Close,
    Ignore(String),
    Unignore(String),
    /// Sent to the server as is.
    Raw(String),
    Invalid(String),
}

pub const CHANNEL_PREFIXES: &str = "#&+!";

/// Every command, for help and completion.
pub const COMMANDS: &[&str] = &[
    "away", "back", "ban", "clear", "close", "ctcp", "cycle", "deop", "devoice", "ignore",
    "invite", "join", "kick", "list", "me", "mode", "msg", "names", "nick", "notice", "op", "part",
    "query", "quote", "topic", "unban", "unignore", "voice", "whois",
];

pub fn is_channel(name: &str) -> bool {
    name.chars()
        .next()
        .is_some_and(|c| CHANNEL_PREFIXES.contains(c))
}

/// `/j foo` means #foo, as in most clients.
pub fn with_hashes(names: &str) -> String {
    names
        .split(',')
        .filter(|n| !n.is_empty())
        .map(|n| {
            if is_channel(n) {
                n.to_owned()
            } else {
                format!("#{n}")
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// A nick becomes `nick!*@*`; masks are used as given.
pub fn ban_mask(target: &str) -> String {
    if target.contains(['!', '@', '*']) {
        target.to_owned()
    } else {
        format!("{target}!*@*")
    }
}

/// IRC text for a CTCP ACTION.
pub fn action_text(text: &str) -> String {
    format!("\u{1}ACTION {text}\u{1}")
}

/// The first word and what follows it (`None` if nothing does).
fn split_word(text: &str) -> (Option<&str>, Option<&str>) {
    let text = text.trim_start_matches(' ');
    if text.is_empty() {
        return (None, None);
    }
    match text.split_once(' ') {
        Some((first, rest)) => {
            let rest = rest.trim();
            (Some(first), (!rest.is_empty()).then_some(rest))
        }
        None => (Some(text), None),
    }
}

/// Parses one line. `buffer` is the current buffer, the default target of
/// channel commands. Returns `None` for blank input.
pub fn parse(input: &str, buffer: &str) -> Option<ComposerAction> {
    use ComposerAction as A;
    let line = input.trim_matches(['\n', '\r']);
    if line.trim().is_empty() {
        return None;
    }
    let Some(body) = line.strip_prefix('/') else {
        return Some(A::Message(line.to_owned()));
    };
    // `//text` sends `/text` literally.
    if body.starts_with('/') {
        return Some(A::Message(body.to_owned()));
    }
    let (command, rest) = split_word(body);
    let command = command.unwrap_or("").to_lowercase();
    let rest = rest.unwrap_or("");
    let (first, remainder) = split_word(rest);
    let in_channel = is_channel(buffer);
    let needs_channel = || A::Invalid(format!("/{command} works in a channel."));
    let usage = |text: &str| A::Invalid(format!("Usage: {text}"));

    Some(match command.as_str() {
        "me" => {
            if rest.is_empty() {
                usage("/me <action>")
            } else {
                A::Action(rest.to_owned())
            }
        }
        "join" | "j" => match first {
            None => usage("/join <#channel> [key]"),
            // The key is one word; anything after it is ignored.
            Some(first) => A::Join {
                channels: with_hashes(first),
                key: remainder.and_then(|r| split_word(r).0).map(str::to_owned),
            },
        },
        "part" | "p" | "leave" => match first {
            Some(first) if is_channel(first) => A::Part {
                channel: first.to_owned(),
                reason: remainder.map(str::to_owned),
            },
            _ if !in_channel => needs_channel(),
            _ => A::Part {
                channel: buffer.to_owned(),
                reason: (!rest.is_empty()).then(|| rest.to_owned()),
            },
        },
        "cycle" | "rejoin" => {
            if in_channel {
                A::Cycle {
                    channel: buffer.to_owned(),
                }
            } else {
                needs_channel()
            }
        }
        "msg" | "m" => match (first, remainder) {
            (Some(nick), Some(text)) => A::Query {
                nick: nick.to_owned(),
                text: Some(text.to_owned()),
            },
            _ => usage("/msg <nick> <message>"),
        },
        "query" | "q" => match first {
            None => usage("/query <nick> [message]"),
            Some(nick) => A::Query {
                nick: nick.to_owned(),
                text: remainder.map(str::to_owned),
            },
        },
        "notice" => match (first, remainder) {
            (Some(target), Some(text)) => A::Raw(format!("NOTICE {target} :{text}")),
            _ => usage("/notice <target> <message>"),
        },
        "ctcp" => match (first, remainder) {
            (Some(target), Some(remainder)) => {
                let (ctcp, args) = split_word(remainder);
                let body = [ctcp.map(str::to_uppercase), args.map(str::to_owned)]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" ");
                A::Raw(format!("PRIVMSG {target} :\u{1}{body}\u{1}"))
            }
            _ => usage("/ctcp <nick> <command> [arguments]"),
        },
        "topic" | "t" => {
            if !in_channel {
                needs_channel()
            } else if rest.is_empty() {
                A::Raw(format!("TOPIC {buffer}"))
            } else {
                A::Raw(format!("TOPIC {buffer} :{rest}"))
            }
        }
        "nick" => match first {
            None => usage("/nick <new nick>"),
            Some(nick) => A::Raw(format!("NICK {nick}")),
        },
        "away" => A::Raw(format!(
            "AWAY :{}",
            if rest.is_empty() { "Away" } else { rest }
        )),
        "back" => A::Raw("AWAY".into()),
        "whois" | "wi" => match first {
            None => usage("/whois <nick>"),
            Some(nick) => A::Raw(format!("WHOIS {nick}")),
        },
        "invite" => match first {
            None => usage("/invite <nick> [#channel]"),
            Some(nick) => match remainder.and_then(|r| split_word(r).0) {
                Some(channel) => A::Raw(format!("INVITE {nick} {channel}")),
                None if in_channel => A::Raw(format!("INVITE {nick} {buffer}")),
                None => needs_channel(),
            },
        },
        "kick" | "k" => match first {
            _ if !in_channel => needs_channel(),
            None => usage("/kick <nick> [reason]"),
            Some(nick) => A::Raw(match remainder {
                Some(reason) => format!("KICK {buffer} {nick} :{reason}"),
                None => format!("KICK {buffer} {nick}"),
            }),
        },
        "ban" | "unban" => match first {
            _ if !in_channel => needs_channel(),
            None => usage(&format!("/{command} <nick or mask>")),
            Some(target) => {
                let sign = if command == "ban" { '+' } else { '-' };
                A::Raw(format!("MODE {buffer} {sign}b {}", ban_mask(target)))
            }
        },
        "op" | "deop" | "voice" | "devoice" => {
            if !in_channel {
                needs_channel()
            } else {
                let nicks: Vec<&str> = rest.split(' ').filter(|n| !n.is_empty()).collect();
                if nicks.is_empty() {
                    usage(&format!("/{command} <nick> [nick…]"))
                } else {
                    let sign = if command.starts_with("de") { '-' } else { '+' };
                    let letter = if command.ends_with("op") { "o" } else { "v" };
                    A::Raw(format!(
                        "MODE {buffer} {sign}{} {}",
                        letter.repeat(nicks.len()),
                        nicks.join(" ")
                    ))
                }
            }
        }
        "mode" => {
            // `/mode +m` means this channel; `/mode #c +m` or `/mode me +i` as given.
            if first.is_some_and(|f| f.starts_with(['+', '-'])) {
                if in_channel {
                    A::Raw(format!("MODE {buffer} {rest}"))
                } else {
                    needs_channel()
                }
            } else if rest.is_empty() {
                if in_channel {
                    A::Raw(format!("MODE {buffer}"))
                } else {
                    needs_channel()
                }
            } else {
                A::Raw(format!("MODE {rest}"))
            }
        }
        "names" => match first {
            Some(channel) => A::Raw(format!("NAMES {channel}")),
            None if in_channel => A::Raw(format!("NAMES {buffer}")),
            None => needs_channel(),
        },
        "list" => A::ListChannels,
        "clear" => A::Clear,
        "close" => A::Close,
        "ignore" | "unignore" => match first {
            None => usage(&format!("/{command} <nick>")),
            Some(nick) if command == "ignore" => A::Ignore(nick.to_owned()),
            Some(nick) => A::Unignore(nick.to_owned()),
        },
        "quote" | "raw" => {
            if rest.is_empty() {
                usage("/quote <IRC line>")
            } else {
                A::Raw(rest.to_owned())
            }
        }
        "" => {
            A::Invalid("Type a command after the slash, or start with // to send a slash.".into())
        }
        // Anything else goes to the server as is: /motd, /who, /oper, …
        _ => A::Raw(if rest.is_empty() {
            command.to_uppercase()
        } else {
            format!("{} {rest}", command.to_uppercase())
        }),
    })
}

/// Tab completion of nicks, channels and commands: the first press
/// completes the last word, further presses cycle through the matches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completion {
    /// The line without the word being completed.
    base: String,
    matches: Vec<String>,
    index: usize,
    /// What the line read after the last completion; any other edit ends it.
    pub result: String,
}

impl Completion {
    /// Starts completing `line`'s last word, or returns `None` if nothing fits.
    pub fn start(line: &str, nicks: &[String], channels: &[String]) -> Option<Completion> {
        let word_start = line.rfind(' ').map_or(0, |i| i + 1);
        let word = &line[word_start..];
        if word.is_empty() {
            return None;
        }
        let base = &line[..word_start];
        let commands: Vec<String>;
        let candidates: &[String] = if base.is_empty() && word.starts_with('/') {
            commands = COMMANDS.iter().map(|c| format!("/{c}")).collect();
            &commands
        } else if is_channel(word) {
            channels
        } else {
            nicks
        };
        let lower = word.to_lowercase();
        let matches: Vec<String> = candidates
            .iter()
            .filter(|c| c.to_lowercase().starts_with(&lower) && c.as_str() != word)
            .cloned()
            .collect();
        if matches.is_empty() {
            return None;
        }
        let mut completion = Completion {
            base: base.to_owned(),
            matches,
            index: 0,
            result: String::new(),
        };
        completion.result = completion.line();
        Some(completion)
    }

    /// The line with the current match: "nick: " at the start of a line,
    /// otherwise followed by a space.
    fn line(&self) -> String {
        let m = &self.matches[self.index];
        let at_start = self.base.is_empty() && !m.starts_with('/') && !is_channel(m);
        format!("{}{m}{}", self.base, if at_start { ": " } else { " " })
    }

    pub fn advance(&mut self) {
        self.index = (self.index + 1) % self.matches.len();
        self.result = self.line();
    }
}

#[cfg(test)]
mod tests {
    use super::ComposerAction as A;
    use super::*;

    fn p(input: &str) -> ComposerAction {
        parse(input, "#chan").unwrap()
    }

    #[test]
    fn messages_and_slashes() {
        assert_eq!(parse("  ", "#c"), None);
        assert_eq!(p("hello"), A::Message("hello".into()));
        assert_eq!(p("//shrug"), A::Message("/shrug".into()));
        assert_eq!(p("/me waves"), A::Action("waves".into()));
    }

    #[test]
    fn channel_commands() {
        assert_eq!(
            p("/j foo,#bar key extra"),
            A::Join {
                channels: "#foo,#bar".into(),
                key: Some("key".into())
            }
        );
        assert_eq!(
            p("/part bye now"),
            A::Part {
                channel: "#chan".into(),
                reason: Some("bye now".into())
            }
        );
        assert_eq!(p("/op a b"), A::Raw("MODE #chan +oo a b".into()));
        assert_eq!(p("/devoice a"), A::Raw("MODE #chan -v a".into()));
        assert_eq!(p("/ban bob"), A::Raw("MODE #chan +b bob!*@*".into()));
        assert_eq!(p("/kick bob go"), A::Raw("KICK #chan bob :go".into()));
        assert_eq!(p("/mode +m"), A::Raw("MODE #chan +m".into()));
        assert_eq!(p("/topic hi there"), A::Raw("TOPIC #chan :hi there".into()));
        assert_eq!(
            parse("/kick bob", "alice"),
            Some(A::Invalid("/kick works in a channel.".into()))
        );
    }

    #[test]
    fn other_commands() {
        assert_eq!(
            p("/msg bob hi there"),
            A::Query {
                nick: "bob".into(),
                text: Some("hi there".into())
            }
        );
        assert_eq!(
            p("/ctcp bob version"),
            A::Raw("PRIVMSG bob :\u{1}VERSION\u{1}".into())
        );
        assert_eq!(p("/motd"), A::Raw("MOTD".into()));
        assert_eq!(p("/who #x"), A::Raw("WHO #x".into()));
        assert_eq!(p("/away"), A::Raw("AWAY :Away".into()));
        assert!(matches!(p("/"), A::Invalid(_)));
    }

    #[test]
    fn completes_and_cycles() {
        let nicks = vec!["alice".to_owned(), "Alfred".to_owned()];
        let mut c = Completion::start("al", &nicks, &[]).unwrap();
        assert_eq!(c.result, "alice: ");
        c.advance();
        assert_eq!(c.result, "Alfred: ");
        c.advance();
        assert_eq!(c.result, "alice: ");
        let c = Completion::start("hi al", &nicks, &[]).unwrap();
        assert_eq!(c.result, "hi alice ");
        let c = Completion::start("/jo", &[], &[]).unwrap();
        assert_eq!(c.result, "/join ");
        let c = Completion::start("see #ba", &[], &["#backroom".into()]).unwrap();
        assert_eq!(c.result, "see #backroom ");
        assert!(Completion::start("zz", &nicks, &[]).is_none());
    }
}
