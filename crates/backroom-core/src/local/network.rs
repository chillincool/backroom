//! A standalone network's settings, from the same attribute map soju's
//! `BOUNCER ADDNETWORK` takes, so front ends use one form for both.
//!
//! Attributes: `name`, `host` (required), `port` (default 6697 with TLS,
//! 6667 without), `tls` (`1`/`0`, default `1`), `nickname`, `username`,
//! `realname`, and the login: `pass` (server password), `sasl-username` +
//! `sasl-password` (SASL PLAIN), `nickserv-password` (NickServ IDENTIFY
//! after connecting). Passwords ([`SECRET_ATTRIBUTES`]) live in the
//! [`SecretVault`], never in the database.

use std::collections::BTreeMap;

use secrecy::SecretString;

use super::{SecretVault, secret_key};
use crate::auth::StoreError;
use crate::model::NetworkId;

/// Attributes kept in the vault. Network events carry `<name>-set=1`
/// instead of the value, so front ends can show that one is saved.
pub const SECRET_ATTRIBUTES: &[&str] = &["pass", "sasl-password", "nickserv-password"];

#[derive(Debug, Clone)]
pub struct SaslLogin {
    pub username: String,
    pub password: SecretString,
}

/// What a standalone connection needs. `Debug` doesn't show passwords.
#[derive(Debug, Clone)]
pub struct NetworkConfig {
    pub id: NetworkId,
    pub host: String,
    pub port: u16,
    pub tls: bool,
    pub nick: String,
    pub username: String,
    pub realname: String,
    /// Server password (`PASS`).
    pub password: Option<SecretString>,
    pub sasl: Option<SaslLogin>,
    pub nickserv: Option<SecretString>,
}

/// Checks the attributes a front end sends; `full` when adding a network
/// (`host` required), not when changing some.
pub fn validate(attributes: &BTreeMap<String, String>, full: bool) -> Result<(), String> {
    if full && attributes.get("host").is_none_or(|h| h.trim().is_empty()) {
        return Err("a network needs a host".into());
    }
    if let Some(host) = attributes.get("host")
        && host.contains(['/', ' ', ':'])
        && !host.starts_with('[')
    {
        return Err(format!("not a host name: {host}"));
    }
    if let Some(port) = attributes.get("port").filter(|p| !p.is_empty())
        && port.parse::<u16>().ok().filter(|&p| p > 0).is_none()
    {
        return Err(format!("not a port: {port}"));
    }
    if let Some(tls) = attributes.get("tls").filter(|t| !t.is_empty())
        && tls != "0"
        && tls != "1"
    {
        return Err("tls is 1 or 0".into());
    }
    for (key, value) in attributes {
        if value.contains(['\r', '\n', '\0']) {
            return Err(format!("{key} can't contain line breaks"));
        }
    }
    Ok(())
}

impl NetworkConfig {
    pub fn new(
        id: &str,
        attributes: &BTreeMap<String, String>,
        default_nick: &str,
        secrets: &dyn SecretVault,
    ) -> Result<Self, StoreError> {
        let attr = |key: &str| {
            attributes
                .get(key)
                .map(|v| v.trim())
                .filter(|v| !v.is_empty())
        };
        let tls = attr("tls") != Some("0");
        let nick = attr("nickname").unwrap_or(default_nick).to_owned();
        let secret = |key: &str| secrets.get(&secret_key(id, key));
        let sasl = secret("sasl-password")?.map(|password| SaslLogin {
            username: attr("sasl-username").unwrap_or(&nick).to_owned(),
            password,
        });
        Ok(NetworkConfig {
            id: id.to_owned(),
            host: attr("host").unwrap_or_default().to_owned(),
            port: attr("port").and_then(|p| p.parse().ok()).unwrap_or(if tls {
                6697
            } else {
                6667
            }),
            tls,
            username: attr("username").unwrap_or(&nick).to_owned(),
            realname: attr("realname").unwrap_or("Backroom").to_owned(),
            nick,
            password: secret("pass")?,
            sasl,
            nickserv: secret("nickserv-password")?,
        })
    }

    /// The URL for [`crate::transport::connector_for_url`].
    pub fn url(&self) -> String {
        let scheme = if self.tls { "ircs" } else { "irc+insecure" };
        format!("{scheme}://{}:{}", self.host, self.port)
    }
}

#[cfg(test)]
mod tests {
    use secrecy::ExposeSecret;

    use super::*;
    use crate::local::tests::MemoryVault;

    fn attrs(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn defaults_follow_tls_and_nick() {
        let vault = MemoryVault::default();
        let cfg = NetworkConfig::new("1", &attrs(&[("host", "irc.libera.chat")]), "dave", &vault)
            .unwrap();
        assert_eq!(cfg.url(), "ircs://irc.libera.chat:6697");
        assert_eq!((cfg.nick.as_str(), cfg.username.as_str()), ("dave", "dave"));
        assert!(cfg.sasl.is_none() && cfg.password.is_none() && cfg.nickserv.is_none());

        let plain = attrs(&[("host", "irc.example"), ("tls", "0"), ("nickname", "erin")]);
        let cfg = NetworkConfig::new("1", &plain, "dave", &vault).unwrap();
        assert_eq!(cfg.url(), "irc+insecure://irc.example:6667");
        assert_eq!(cfg.nick, "erin");
    }

    #[test]
    fn passwords_come_from_the_vault() {
        let vault = MemoryVault::default();
        vault
            .set(&secret_key("7", "sasl-password"), &"hunter2".into())
            .unwrap();
        let cfg = NetworkConfig::new("7", &attrs(&[("host", "h")]), "dave", &vault).unwrap();
        let sasl = cfg.sasl.unwrap();
        assert_eq!(sasl.username, "dave");
        assert_eq!(sasl.password.expose_secret(), "hunter2");
        assert!(
            !format!(
                "{:?}",
                NetworkConfig::new("7", &attrs(&[("host", "h")]), "d", &vault)
            )
            .contains("hunter2")
        );
    }

    #[test]
    fn validation() {
        assert!(validate(&attrs(&[("name", "x")]), true).is_err());
        assert!(validate(&attrs(&[("name", "x")]), false).is_ok());
        assert!(validate(&attrs(&[("host", "irc.example"), ("port", "70000")]), true).is_err());
        assert!(validate(&attrs(&[("host", "irc.example/x")]), true).is_err());
        assert!(validate(&attrs(&[("host", "h"), ("tls", "yes")]), true).is_err());
        assert!(validate(&attrs(&[("host", "h"), ("realname", "a\nb")]), true).is_err());
        assert!(
            validate(
                &attrs(&[("host", "h"), ("port", "6697"), ("tls", "1")]),
                true
            )
            .is_ok()
        );
    }
}
