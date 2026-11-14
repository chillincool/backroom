//! Channel membership: built from NAMES replies, then kept current through
//! JOIN, PART, KICK, QUIT, NICK and MODE.

use std::collections::{BTreeMap, HashMap};

use crate::model::{Member, buffer_key};
use crate::proto::Message;

/// Membership prefixes from ISUPPORT `PREFIX`, highest rank first.
#[derive(Debug, Clone)]
struct Prefixes {
    modes: Vec<char>,
    symbols: Vec<char>,
}

impl Prefixes {
    /// Parses `(qaohv)~&@%+`. Without one (soju may only learn the
    /// network's PREFIX after we registered) assume the common full set:
    /// nicks can't start with these characters, so it's safe either way.
    fn parse(value: Option<&str>) -> Self {
        let parsed = value.and_then(|v| {
            let (modes, symbols) = v.strip_prefix('(')?.split_once(')')?;
            (modes.chars().count() == symbols.chars().count()).then(|| Prefixes {
                modes: modes.chars().collect(),
                symbols: symbols.chars().collect(),
            })
        });
        parsed.unwrap_or(Prefixes {
            modes: vec!['q', 'a', 'o', 'h', 'v'],
            symbols: vec!['~', '&', '@', '%', '+'],
        })
    }

    fn rank(&self, symbol: char) -> usize {
        self.symbols
            .iter()
            .position(|&s| s == symbol)
            .unwrap_or(usize::MAX)
    }

    /// Adds or removes `symbol`, keeping the prefix ordered by rank.
    fn update(&self, prefix: &str, symbol: char, add: bool) -> String {
        let mut chars: Vec<char> = prefix.chars().filter(|&c| c != symbol).collect();
        if add {
            chars.push(symbol);
        }
        chars.sort_by_key(|&c| self.rank(c));
        chars.into_iter().collect()
    }
}

#[derive(Debug, Default)]
struct Channel {
    /// Name as the server spells it.
    name: String,
    /// Keyed by [`buffer_key`] of the nick.
    members: HashMap<String, Member>,
    /// Between the first RPL_NAMREPLY and RPL_ENDOFNAMES.
    collecting: bool,
}

#[derive(Debug)]
pub(super) struct Members {
    prefixes: Prefixes,
    /// ISUPPORT `CHANMODES` types A, B and C, which take a parameter.
    param_modes: [String; 3],
    channels: HashMap<String, Channel>,
}

impl Members {
    pub(super) fn new(isupport: &BTreeMap<String, String>) -> Self {
        let mut param_modes: [String; 3] = Default::default();
        if let Some(chanmodes) = isupport.get("CHANMODES") {
            for (slot, kind) in param_modes.iter_mut().zip(chanmodes.split(',')) {
                *slot = kind.to_owned();
            }
        }
        Members {
            prefixes: Prefixes::parse(isupport.get("PREFIX").map(String::as_str)),
            param_modes,
            channels: HashMap::new(),
        }
    }

    /// Applies an ISUPPORT token that arrived after registration (`None`
    /// for a removed one). Lists already built keep their prefixes until
    /// the next NAMES.
    pub(super) fn set_isupport(&mut self, key: &str, value: Option<&str>) {
        match key {
            "PREFIX" => self.prefixes = Prefixes::parse(value),
            "CHANMODES" => {
                self.param_modes = Default::default();
                for (slot, kind) in self
                    .param_modes
                    .iter_mut()
                    .zip(value.unwrap_or("").split(','))
                {
                    *slot = kind.to_owned();
                }
            }
            _ => {}
        }
    }

    /// Applies `msg` and returns the channels whose member lists changed and
    /// are complete (not mid-NAMES).
    pub(super) fn apply(&mut self, msg: &Message, own_nick: &str) -> Vec<String> {
        let source = msg.source_nick().unwrap_or_default();
        let own = !source.is_empty() && source.eq_ignore_ascii_case(own_nick);
        match msg.command.as_str() {
            // RPL_NAMREPLY <me> <symbol> <channel> :<names>
            "353" => {
                let (Some(chan), Some(names)) = (msg.param(2), msg.param(3)) else {
                    return vec![];
                };
                let prefixes = self.prefixes.clone();
                let ch = self.channel_mut(chan);
                if !ch.collecting {
                    ch.members.clear();
                    ch.collecting = true;
                }
                for entry in names.split(' ').filter(|n| !n.is_empty()) {
                    let split = entry
                        .find(|c: char| !prefixes.symbols.contains(&c))
                        .unwrap_or(entry.len());
                    let (prefix, rest) = entry.split_at(split);
                    // userhost-in-names: nick!user@host
                    let nick = rest.split('!').next().unwrap_or(rest);
                    if !nick.is_empty() {
                        ch.members.insert(
                            buffer_key(nick),
                            Member {
                                nick: nick.to_owned(),
                                prefix: prefix.to_owned(),
                            },
                        );
                    }
                }
                vec![]
            }
            // RPL_ENDOFNAMES <me> <channel>
            "366" => {
                let Some(chan) = msg.param(1) else {
                    return vec![];
                };
                match self.channels.get_mut(&buffer_key(chan)) {
                    Some(ch) => {
                        ch.collecting = false;
                        vec![ch.name.clone()]
                    }
                    None => vec![],
                }
            }
            "JOIN" => {
                let Some(chan) = msg.param(0) else {
                    return vec![];
                };
                if own {
                    // Fresh list; reported once NAMES has filled it.
                    self.channels.remove(&buffer_key(chan));
                }
                let ch = self.channel_mut(chan);
                ch.members.insert(
                    buffer_key(source),
                    Member {
                        nick: source.to_owned(),
                        prefix: String::new(),
                    },
                );
                if own { vec![] } else { self.complete(chan) }
            }
            "PART" => {
                let Some(chan) = msg.param(0) else {
                    return vec![];
                };
                self.remove(chan, source, own)
            }
            // KICK <channel> <nick> [reason]
            "KICK" => {
                let (Some(chan), Some(target)) = (msg.param(0), msg.param(1)) else {
                    return vec![];
                };
                self.remove(chan, target, target.eq_ignore_ascii_case(own_nick))
            }
            "QUIT" => {
                let key = buffer_key(source);
                let mut changed = vec![];
                for ch in self.channels.values_mut() {
                    if ch.members.remove(&key).is_some() && !ch.collecting {
                        changed.push(ch.name.clone());
                    }
                }
                changed
            }
            "NICK" => {
                let Some(new) = msg.param(0) else {
                    return vec![];
                };
                let key = buffer_key(source);
                let mut changed = vec![];
                for ch in self.channels.values_mut() {
                    if let Some(mut member) = ch.members.remove(&key) {
                        member.nick = new.to_owned();
                        ch.members.insert(buffer_key(new), member);
                        if !ch.collecting {
                            changed.push(ch.name.clone());
                        }
                    }
                }
                changed
            }
            // MODE <channel> <modes> [args…]
            "MODE" => {
                let (Some(chan), Some(modes)) = (msg.param(0), msg.param(1)) else {
                    return vec![];
                };
                let key = buffer_key(chan);
                let Some(ch) = self.channels.get_mut(&key) else {
                    return vec![];
                };
                let mut args = msg.params.iter().skip(2);
                let mut add = true;
                let mut changed = false;
                for mode in modes.chars() {
                    match mode {
                        '+' => add = true,
                        '-' => add = false,
                        m => {
                            if let Some(i) = self.prefixes.modes.iter().position(|&p| p == m) {
                                let Some(nick) = args.next() else { break };
                                if let Some(member) = ch.members.get_mut(&buffer_key(nick)) {
                                    member.prefix = self.prefixes.update(
                                        &member.prefix,
                                        self.prefixes.symbols[i],
                                        add,
                                    );
                                    changed = true;
                                }
                            } else if self.param_modes[0].contains(m)
                                || self.param_modes[1].contains(m)
                                || (add && self.param_modes[2].contains(m))
                            {
                                args.next();
                            }
                        }
                    }
                }
                if changed && !ch.collecting {
                    vec![ch.name.clone()]
                } else {
                    vec![]
                }
            }
            _ => vec![],
        }
    }

    /// Members of `channel`, highest rank first, then by nick.
    pub(super) fn list(&self, channel: &str) -> Vec<Member> {
        let Some(ch) = self.channels.get(&buffer_key(channel)) else {
            return vec![];
        };
        let rank = |m: &Member| {
            m.prefix
                .chars()
                .next()
                .map_or(usize::MAX, |c| self.prefixes.rank(c))
        };
        let mut members: Vec<Member> = ch.members.values().cloned().collect();
        members.sort_by(|a, b| {
            rank(a)
                .cmp(&rank(b))
                .then_with(|| buffer_key(&a.nick).cmp(&buffer_key(&b.nick)))
        });
        members
    }

    fn channel_mut(&mut self, name: &str) -> &mut Channel {
        self.channels
            .entry(buffer_key(name))
            .or_insert_with(|| Channel {
                name: name.to_owned(),
                ..Default::default()
            })
    }

    fn complete(&self, chan: &str) -> Vec<String> {
        match self.channels.get(&buffer_key(chan)) {
            Some(ch) if !ch.collecting => vec![ch.name.clone()],
            _ => vec![],
        }
    }

    /// `nick` left `chan`; if it was us, forget the channel.
    fn remove(&mut self, chan: &str, nick: &str, own: bool) -> Vec<String> {
        let key = buffer_key(chan);
        if own {
            self.channels.remove(&key);
            return vec![];
        }
        let removed = self
            .channels
            .get_mut(&key)
            .is_some_and(|ch| ch.members.remove(&buffer_key(nick)).is_some());
        if removed { self.complete(chan) } else { vec![] }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn members() -> Members {
        Members::new(&BTreeMap::from([
            ("PREFIX".to_owned(), "(qaohv)~&@%+".to_owned()),
            ("CHANMODES".to_owned(), "beI,k,l,imnst".to_owned()),
        ]))
    }

    fn apply(m: &mut Members, line: &str) -> Vec<String> {
        m.apply(&Message::parse(line).unwrap(), "me")
    }

    fn nicks(m: &Members, chan: &str) -> Vec<String> {
        m.list(chan)
            .into_iter()
            .map(|m| format!("{}{}", m.prefix, m.nick))
            .collect()
    }

    fn joined(m: &mut Members) {
        apply(m, ":me!u@h JOIN #Rust");
        apply(m, ":srv 353 me = #rust :@+alice bob!b@host +carol me");
        assert!(
            apply(m, ":srv 353 me = #rust :~dave").is_empty(),
            "mid-NAMES"
        );
        assert_eq!(apply(m, ":srv 366 me #rust :End of /NAMES list"), ["#Rust"]);
    }

    #[test]
    fn names_build_a_ranked_list() {
        let mut m = members();
        joined(&mut m);
        assert_eq!(
            nicks(&m, "#rust"),
            ["~dave", "@+alice", "+carol", "bob", "me"]
        );
    }

    #[test]
    fn joins_parts_quits_and_kicks() {
        let mut m = members();
        joined(&mut m);
        assert_eq!(apply(&mut m, ":erin!e@h JOIN #rust"), ["#Rust"]);
        assert_eq!(apply(&mut m, ":bob!b@h PART #rust :bye"), ["#Rust"]);
        assert_eq!(apply(&mut m, ":carol!c@h QUIT :gone"), ["#Rust"]);
        assert_eq!(apply(&mut m, ":dave!d@h KICK #rust erin :out"), ["#Rust"]);
        assert_eq!(nicks(&m, "#rust"), ["~dave", "@+alice", "me"]);
        // Someone we don't know leaving changes nothing.
        assert!(apply(&mut m, ":zed!z@h QUIT :x").is_empty());
    }

    #[test]
    fn nick_changes_keep_prefixes() {
        let mut m = members();
        joined(&mut m);
        assert_eq!(apply(&mut m, ":alice!a@h NICK Alicia"), ["#Rust"]);
        assert_eq!(nicks(&m, "#rust")[1], "@+Alicia");
    }

    #[test]
    fn modes_update_prefixes_and_skip_other_params() {
        let mut m = members();
        joined(&mut m);
        // +k takes a key, +l a limit, +b a mask: none of them are nicks.
        assert_eq!(
            apply(
                &mut m,
                ":dave!d@h MODE #rust +kov-v+lb secret bob carol carol 10 x!*@*"
            ),
            ["#Rust"]
        );
        assert_eq!(
            nicks(&m, "#rust"),
            ["~dave", "@+alice", "@bob", "carol", "me"]
        );
        assert!(apply(&mut m, ":dave!d@h MODE #rust +m").is_empty());
    }

    #[test]
    fn leaving_forgets_the_channel() {
        let mut m = members();
        joined(&mut m);
        assert!(apply(&mut m, ":me!u@h PART #rust").is_empty());
        assert!(m.list("#rust").is_empty());
        joined(&mut m);
        assert!(apply(&mut m, ":dave!d@h KICK #rust me :bye").is_empty());
        assert!(m.list("#rust").is_empty());
    }

    #[test]
    fn rejoining_starts_fresh() {
        let mut m = members();
        joined(&mut m);
        apply(&mut m, ":me!u@h JOIN #rust");
        assert_eq!(nicks(&m, "#rust"), ["me"]);
    }

    #[test]
    fn without_isupport_the_common_prefixes_are_known() {
        let mut m = Members::new(&BTreeMap::new());
        apply(&mut m, ":me!u@h JOIN #x");
        apply(&mut m, ":srv 353 me = #x :%caitlin &Bob ~own me");
        apply(&mut m, ":srv 366 me #x :End");
        assert_eq!(nicks(&m, "#x"), ["~own", "&Bob", "%caitlin", "me"]);
    }

    #[test]
    fn late_isupport_changes_the_prefixes() {
        let mut m = Members::new(&BTreeMap::new());
        m.set_isupport("PREFIX", Some("(Yov)!@+"));
        apply(&mut m, ":me!u@h JOIN #x");
        apply(&mut m, ":srv 353 me = #x :+v !y @o me");
        apply(&mut m, ":srv 366 me #x :End");
        assert_eq!(nicks(&m, "#x"), ["!y", "@o", "+v", "me"]);
    }

    #[test]
    fn default_prefixes() {
        let mut m = Members::new(&BTreeMap::new());
        apply(&mut m, ":me!u@h JOIN #x");
        apply(&mut m, ":srv 353 me = #x :+v @o me");
        apply(&mut m, ":srv 366 me #x :End");
        assert_eq!(nicks(&m, "#x"), ["@o", "+v", "me"]);
    }
}
