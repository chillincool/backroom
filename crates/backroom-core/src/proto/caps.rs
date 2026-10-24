//! IRCv3 capability negotiation helpers (`CAP LS 302`).

use std::collections::BTreeMap;

/// Capabilities this client knows how to use, in request order.
///
/// Names verified against soju v0.11.1 (`downstream.go`). Some are passthrough
/// caps that soju only offers when the bound upstream network supports them
/// (`message-tags`, `labeled-response`, `away-notify`, `account-tag`).
pub const WANTED_CAPS: &[&str] = &[
    "sasl",
    "batch",
    "server-time",
    "message-tags",
    "labeled-response",
    "echo-message",
    "away-notify",
    "account-tag",
    "cap-notify",
    "draft/chathistory",
    "draft/read-marker",
    "soju.im/bouncer-networks",
];

/// Capabilities advertised by the server, name → value (empty if none).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CapSet(BTreeMap<String, String>);

impl CapSet {
    /// Adds the space-separated entries of one `CAP LS` / `CAP NEW` line.
    pub fn extend_from_list(&mut self, list: &str) {
        for entry in list.split(' ').filter(|e| !e.is_empty()) {
            let (name, value) = entry.split_once('=').unwrap_or((entry, ""));
            self.0.insert(name.to_owned(), value.to_owned());
        }
    }

    pub fn contains(&self, name: &str) -> bool {
        self.0.contains_key(name)
    }

    pub fn value(&self, name: &str) -> Option<&str> {
        self.0.get(name).map(String::as_str)
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.0.keys().map(String::as_str)
    }

    /// SASL mechanisms from the `sasl` cap value. Empty when the server
    /// advertises `sasl` without a value (mechanisms unknown).
    pub fn sasl_mechanisms(&self) -> Vec<&str> {
        self.value("sasl")
            .map(|v| v.split(',').filter(|m| !m.is_empty()).collect())
            .unwrap_or_default()
    }
}

/// Splits capability names into `CAP REQ` argument strings that keep each
/// line comfortably under the 512-byte limit.
pub fn chunk_cap_req<'a>(names: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    const MAX: usize = 400;
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for name in names {
        if !cur.is_empty() && cur.len() + 1 + name.len() > MAX {
            out.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(name);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_values_and_mechanisms() {
        let mut caps = CapSet::default();
        caps.extend_from_list("sasl=PLAIN,OAUTHBEARER batch  draft/chathistory");
        caps.extend_from_list("soju.im/bouncer-networks");
        assert!(caps.contains("batch"));
        assert!(caps.contains("soju.im/bouncer-networks"));
        assert_eq!(caps.value("batch"), Some(""));
        assert_eq!(caps.sasl_mechanisms(), ["PLAIN", "OAUTHBEARER"]);
    }

    #[test]
    fn chunks_long_requests() {
        let names: Vec<String> = (0..100)
            .map(|i| format!("vendor.example/cap-{i:03}"))
            .collect();
        let chunks = chunk_cap_req(names.iter().map(String::as_str));
        assert!(chunks.len() > 1);
        assert!(chunks.iter().all(|c| c.len() <= 400));
        let rejoined: Vec<&str> = chunks.iter().flat_map(|c| c.split(' ')).collect();
        assert_eq!(rejoined.len(), 100);
    }
}
