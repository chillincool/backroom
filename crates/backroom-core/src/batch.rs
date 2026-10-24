//! Collects IRCv3 batches (<https://ircv3.net/specs/extensions/batch>).

use std::collections::HashMap;

use crate::proto::Message;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Batch {
    pub kind: String,
    pub params: Vec<String>,
    /// Tags on the opening `BATCH +ref` line (e.g. `label`).
    pub tags: Vec<(String, String)>,
    pub messages: Vec<Message>,
    /// Completed nested batches, in completion order.
    pub children: Vec<Batch>,
}

impl Batch {
    pub fn tag(&self, key: &str) -> Option<&str> {
        self.tags
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Feed {
    /// Not part of any batch; handle normally.
    Unbatched(Message),
    /// Absorbed into an open batch.
    Buffered,
    /// A top-level batch finished.
    Completed(Batch),
}

#[derive(Debug, Default)]
pub struct BatchCollector {
    open: HashMap<String, (Option<String>, Batch)>,
}

impl BatchCollector {
    pub fn feed(&mut self, msg: Message) -> Feed {
        let parent = msg.tag("batch").map(str::to_owned);

        if msg.command == "BATCH" {
            let Some(reference) = msg.param(0) else {
                return Feed::Unbatched(msg);
            };
            if let Some(id) = reference.strip_prefix('+') {
                let batch = Batch {
                    kind: msg.param(1).unwrap_or_default().to_owned(),
                    params: msg.params.iter().skip(2).cloned().collect(),
                    tags: msg.tags.clone(),
                    messages: Vec::new(),
                    children: Vec::new(),
                };
                self.open.insert(id.to_owned(), (parent, batch));
                return Feed::Buffered;
            }
            if let Some(id) = reference.strip_prefix('-') {
                let Some((parent, batch)) = self.open.remove(id) else {
                    tracing::debug!(batch = id, "end of unknown batch");
                    return Feed::Buffered;
                };
                return match parent.and_then(|p| self.open.get_mut(&p)) {
                    Some((_, outer)) => {
                        outer.children.push(batch);
                        Feed::Buffered
                    }
                    None => Feed::Completed(batch),
                };
            }
            return Feed::Unbatched(msg);
        }

        match parent.and_then(|p| self.open.get_mut(&p)) {
            Some((_, batch)) => {
                batch.messages.push(msg);
                Feed::Buffered
            }
            None => Feed::Unbatched(msg),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed_all(c: &mut BatchCollector, lines: &[&str]) -> Vec<Feed> {
        lines
            .iter()
            .map(|l| c.feed(Message::parse(l).unwrap()))
            .collect()
    }

    #[test]
    fn collects_simple_batch() {
        let mut c = BatchCollector::default();
        let out = feed_all(
            &mut c,
            &[
                "@label=L1 :s BATCH +abc chathistory #chan",
                "@batch=abc :n PRIVMSG #chan :one",
                ":n PRIVMSG #other :unbatched",
                "@batch=abc :n PRIVMSG #chan :two",
                ":s BATCH -abc",
            ],
        );
        assert_eq!(out[0], Feed::Buffered);
        assert!(matches!(out[2], Feed::Unbatched(_)));
        let Feed::Completed(b) = &out[4] else {
            panic!()
        };
        assert_eq!(b.kind, "chathistory");
        assert_eq!(b.params, ["#chan"]);
        assert_eq!(b.tag("label"), Some("L1"));
        assert_eq!(b.messages.len(), 2);
        assert_eq!(b.messages[1].param(1), Some("two"));
    }

    #[test]
    fn nests_batches() {
        let mut c = BatchCollector::default();
        let out = feed_all(
            &mut c,
            &[
                "@label=x BATCH +outer labeled-response",
                "@batch=outer BATCH +inner chathistory #c",
                "@batch=inner PRIVMSG #c :hi",
                "BATCH -inner",
                "BATCH -outer",
            ],
        );
        assert_eq!(out[3], Feed::Buffered);
        let Feed::Completed(b) = &out[4] else {
            panic!()
        };
        assert_eq!(b.kind, "labeled-response");
        assert_eq!(b.children.len(), 1);
        assert_eq!(b.children[0].kind, "chathistory");
        assert_eq!(b.children[0].messages.len(), 1);
    }

    #[test]
    fn unknown_batch_end_is_ignored() {
        let mut c = BatchCollector::default();
        assert_eq!(
            c.feed(Message::parse("BATCH -nope").unwrap()),
            Feed::Buffered
        );
    }
}
