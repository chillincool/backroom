//! Finds our nick and highlight words in message text, and knows whom to
//! ignore.

use std::collections::HashSet;

/// Matches whole words in any case: "bob" matches "bob:" and "@bob", not
/// "bobby".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MentionMatcher {
    /// Lowercased.
    terms: Vec<String>,
    ignored: HashSet<String>,
}

impl MentionMatcher {
    pub fn new(nick: Option<&str>, words: &[String], ignored: &[String]) -> Self {
        let terms = nick
            .into_iter()
            .chain(words.iter().map(String::as_str))
            .map(|t| t.trim().to_lowercase())
            .filter(|t| !t.is_empty())
            .collect();
        MentionMatcher {
            terms,
            ignored: ignored.iter().map(|n| n.to_lowercase()).collect(),
        }
    }

    pub fn matches(&self, text: &str) -> bool {
        if self.terms.is_empty() {
            return false;
        }
        let plain = backroom_ffi::strip_formatting(text.to_owned()).to_lowercase();
        self.terms.iter().any(|term| contains_word(&plain, term))
    }

    pub fn is_ignored(&self, nick: &str) -> bool {
        self.ignored.contains(&nick.to_lowercase())
    }
}

/// Characters that continue a nick or word.
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || "_[]\\`^{|}-".contains(c)
}

fn contains_word(haystack: &str, word: &str) -> bool {
    haystack.match_indices(word).any(|(start, _)| {
        let before = haystack[..start].chars().next_back();
        let after = haystack[start + word.len()..].chars().next();
        !before.is_some_and(is_word_char) && !after.is_some_and(is_word_char)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_words_any_case() {
        let m = MentionMatcher::new(Some("bob"), &["Rust".into()], &[]);
        assert!(m.matches("bob: hi"));
        assert!(m.matches("hey @Bob"));
        assert!(m.matches("I like rust."));
        assert!(!m.matches("bobby"));
        assert!(!m.matches("bob_"));
        assert!(!m.matches("trusty"));
        assert!(m.matches("bobby and bob"));
        // Formatting codes don't hide a mention.
        assert!(m.matches("\u{2}bob\u{2}"));
    }

    #[test]
    fn ignores_case_insensitively() {
        let m = MentionMatcher::new(None, &[], &["Spammer".into()]);
        assert!(m.is_ignored("spammer"));
        assert!(!m.matches("anything"));
    }
}
