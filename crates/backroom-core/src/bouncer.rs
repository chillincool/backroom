//! `soju.im/bouncer-networks` (soju's `doc/ext/bouncer-networks.md`).

use std::collections::BTreeMap;

use crate::proto::Message;
use crate::proto::message::unescape_tag_value;

/// An upstream network configured in the bouncer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Network {
    pub id: String,
    /// Attributes such as `name`, `state`, `host`, `nickname`, `error`.
    pub attributes: BTreeMap<String, String>,
}

impl Network {
    pub fn name(&self) -> Option<&str> {
        self.attr("name")
    }

    /// `connected`, `connecting` or `disconnected`.
    pub fn state(&self) -> Option<&str> {
        self.attr("state")
    }

    pub fn attr(&self, key: &str) -> Option<&str> {
        self.attributes.get(key).map(String::as_str)
    }

    /// Name if set, otherwise host, otherwise the ID.
    pub fn display_name(&self) -> &str {
        self.name()
            .or_else(|| self.attr("host"))
            .unwrap_or(&self.id)
    }
}

/// A parsed `BOUNCER NETWORK <netid> <attributes>` message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetworkUpdate {
    /// Added or changed. In notifications, only changed attributes are
    /// present and an empty value means the attribute was removed.
    Upsert(Network),
    Removed {
        id: String,
    },
}

pub fn parse_network_message(msg: &Message) -> Option<NetworkUpdate> {
    if msg.command != "BOUNCER" || !msg.param(0)?.eq_ignore_ascii_case("NETWORK") {
        return None;
    }
    let id = msg.param(1)?.to_owned();
    let attrs = msg.param(2).unwrap_or_default();
    if attrs == "*" {
        return Some(NetworkUpdate::Removed { id });
    }
    Some(NetworkUpdate::Upsert(Network {
        id,
        attributes: parse_attributes(attrs),
    }))
}

/// Parses `key=value;key2=value2`, using message-tag escaping.
pub fn parse_attributes(s: &str) -> BTreeMap<String, String> {
    s.split(';')
        .filter(|a| !a.is_empty())
        .map(|a| match a.split_once('=') {
            Some((k, v)) => (k.to_owned(), unescape_tag_value(v)),
            None => (a.to_owned(), String::new()),
        })
        .collect()
}

/// Finds a network by exact ID, then by case-insensitive name.
pub fn find_network<'a>(networks: &'a [Network], query: &str) -> Option<&'a Network> {
    networks.iter().find(|n| n.id == query).or_else(|| {
        networks.iter().find(|n| {
            n.name()
                .is_some_and(|name| name.eq_ignore_ascii_case(query))
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(line: &str) -> Option<NetworkUpdate> {
        parse_network_message(&Message::parse(line).unwrap())
    }

    #[test]
    fn parses_network_with_escaped_attributes() {
        let Some(NetworkUpdate::Upsert(n)) = parse(
            "@batch=a BOUNCER NETWORK 43 name=My\\sAwesome\\sNetwork;state=disconnected;error=TLS\\:\\sbad",
        ) else {
            panic!()
        };
        assert_eq!(n.id, "43");
        assert_eq!(n.name(), Some("My Awesome Network"));
        assert_eq!(n.state(), Some("disconnected"));
        assert_eq!(n.attr("error"), Some("TLS; bad"));
    }

    #[test]
    fn parses_removal_and_valueless_attribute() {
        assert_eq!(
            parse("BOUNCER NETWORK 44 *"),
            Some(NetworkUpdate::Removed { id: "44".into() })
        );
        let Some(NetworkUpdate::Upsert(n)) = parse("BOUNCER NETWORK 44 error") else {
            panic!()
        };
        assert_eq!(n.attr("error"), Some(""));
    }

    #[test]
    fn ignores_other_messages() {
        assert_eq!(parse("BOUNCER ADDNETWORK 44"), None);
        assert_eq!(parse("PRIVMSG #a :b"), None);
    }

    #[test]
    fn finds_by_id_then_name() {
        let nets = vec![
            Network {
                id: "1".into(),
                attributes: parse_attributes("name=Libera"),
            },
            Network {
                id: "2".into(),
                attributes: parse_attributes("name=1"),
            },
        ];
        assert_eq!(find_network(&nets, "1").unwrap().id, "1");
        assert_eq!(find_network(&nets, "libera").unwrap().id, "1");
        assert!(find_network(&nets, "oftc").is_none());
    }
}
