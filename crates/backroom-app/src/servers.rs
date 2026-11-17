//! Saved soju servers, how to log in to each, and the built-in "No
//! bouncer" entry. Nothing secret: passwords and tokens live in the
//! platform's secret store, one item per login (`secret_account`).

use std::collections::HashMap;
use std::path::Path;

use backroom_ffi::{ClientConfig, LoginMethod};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Login {
    /// soju's own accounts (`auth internal`): username and password.
    Password,
    /// soju's `auth oauth2`: a browser login at `issuer`.
    SingleSignOn,
    /// No bouncer: Backroom connects to each network itself and keeps
    /// history on this computer.
    Standalone,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerSettings {
    pub id: Uuid,
    /// Shown in the server menu; the host when empty.
    pub name: String,
    pub server_url: String,
    pub login: Login,
    /// The soju username, for `Password`.
    pub username: String,
    /// The OIDC issuer (Keycloak realm URL), for `SingleSignOn`.
    pub issuer: String,
    pub nick: String,
}

impl Default for ServerSettings {
    fn default() -> Self {
        ServerSettings::blank()
    }
}

pub const CLIENT_ID: &str = "backroom";
pub const REDIRECT_URI: &str = "backroom://oauth/callback";
pub const CALLBACK_SCHEME: &str = "backroom";
/// `soju_roles` puts soju in the token's audience. `offline_access` gets a
/// refresh token that outlives Keycloak's idle browser session.
pub const SCOPES: &[&str] = &["soju_roles", "offline_access"];

const STANDALONE_ID: Uuid = Uuid::from_u128(0xB0B0);

impl ServerSettings {
    /// What "Add Server" starts from.
    pub fn blank() -> Self {
        ServerSettings {
            id: Uuid::new_v4(),
            name: String::new(),
            server_url: String::new(),
            login: Login::Password,
            username: String::new(),
            issuer: String::new(),
            nick: String::new(),
        }
    }

    /// Without a bouncer. Only the nick (the default for networks) can be
    /// changed.
    pub fn standalone() -> Self {
        ServerSettings {
            id: STANDALONE_ID,
            name: "No bouncer".into(),
            login: Login::Standalone,
            ..ServerSettings::blank()
        }
    }

    /// The local e2e stack (`mise run e2e:up`): log in as alice / alice.
    pub fn e2e() -> Self {
        ServerSettings {
            id: Uuid::from_u128(0xE2E0_0000_0000_0000_0000_0000_0000_0001),
            name: "Test stack (alice, SSO)".into(),
            server_url: "ws://127.0.0.1:18081/".into(),
            login: Login::SingleSignOn,
            username: String::new(),
            issuer: "http://localhost:18080/realms/e2e".into(),
            nick: "alice".into(),
        }
    }

    /// The e2e stack's soju with its own accounts: dave / dave-secret.
    pub fn e2e_password() -> Self {
        ServerSettings {
            id: Uuid::from_u128(0xE2E0_0000_0000_0000_0000_0000_0000_0002),
            name: "Test stack (dave, password)".into(),
            server_url: "ws://127.0.0.1:28081/".into(),
            login: Login::Password,
            username: "dave".into(),
            issuer: String::new(),
            nick: "dave".into(),
        }
    }

    pub fn is_standalone(&self) -> bool {
        self.login == Login::Standalone
    }

    /// Same server, login and nick as `other`: only the name differs.
    pub fn connects_like(&self, other: Option<&ServerSettings>) -> bool {
        other.is_some_and(|o| {
            let mut o = o.clone();
            o.name.clone_from(&self.name);
            &o == self
        })
    }

    /// The name, or else the server's host.
    pub fn display_name(&self) -> String {
        if !self.name.is_empty() {
            return self.name.clone();
        }
        url_host(&self.server_url).unwrap_or_else(|| self.server_url.clone())
    }

    /// Whether the fields the login method needs are filled in.
    pub fn is_complete(&self) -> bool {
        match self.login {
            Login::Standalone => true,
            _ if self.server_url.trim().is_empty() => false,
            Login::Password => !self.username.trim().is_empty(),
            Login::SingleSignOn => !self.issuer.trim().is_empty(),
        }
    }

    /// The core's config. `database` is the standalone history database;
    /// `text_logs` where standalone mode also writes plain-text logs.
    pub fn client_config(
        &self,
        database: &Path,
        text_logs: Option<String>,
        client_version: &str,
    ) -> ClientConfig {
        let login = match self.login {
            Login::Password => LoginMethod::Password {
                username: self.username.trim().to_owned(),
            },
            Login::SingleSignOn => LoginMethod::Oidc {
                issuer: self.issuer.trim().to_owned(),
                client_id: CLIENT_ID.into(),
                scopes: SCOPES.iter().map(|s| (*s).to_owned()).collect(),
                redirect_uri: REDIRECT_URI.into(),
            },
            Login::Standalone => LoginMethod::Standalone {
                database: database.display().to_string(),
                text_logs,
            },
        };
        // soju gives each network its own nick; this is only the default.
        let nick = [self.nick.trim(), self.username.trim()]
            .into_iter()
            .find(|n| !n.is_empty())
            .unwrap_or("backroom")
            .to_owned();
        ClientConfig {
            server_url: self.server_url.trim().to_owned(),
            login,
            nick,
            client_version: client_version.to_owned(),
        }
    }

    /// The secret store item for this login: tokens belong to one issuer
    /// and server, a password to one user and server. Standalone mode keeps
    /// all network passwords and channel keys in one item.
    pub fn secret_account(&self) -> String {
        match self.login {
            Login::SingleSignOn => format!("{} {}", self.issuer.trim(), self.server_url.trim()),
            Login::Password => format!(
                "password {} {}",
                self.username.trim(),
                self.server_url.trim()
            ),
            Login::Standalone => "standalone".into(),
        }
    }
}

/// The host of a URL like `wss://irc.example.net/socket`.
fn url_host(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = if host.starts_with('[') {
        host.split_inclusive(']').next()?
    } else {
        host.split(':').next()?
    };
    (!host.is_empty()).then(|| host.to_owned())
}

/// The saved servers and which one the app uses. One session at a time;
/// each server keeps its own login in the secret store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerList {
    pub servers: Vec<ServerSettings>,
    pub selected: Option<Uuid>,
}

impl Default for ServerList {
    fn default() -> Self {
        ServerList::new(Vec::new(), None)
    }
}

impl ServerList {
    /// "No bouncer" is always there, last.
    pub fn new(servers: Vec<ServerSettings>, selected: Option<Uuid>) -> Self {
        let selected = selected.or_else(|| servers.first().map(|s| s.id));
        let mut list = ServerList { servers, selected };
        list.add_standalone();
        list
    }

    fn add_standalone(&mut self) {
        if !self.servers.iter().any(ServerSettings::is_standalone) {
            self.servers.push(ServerSettings::standalone());
        }
    }

    /// The soju servers, without "No bouncer".
    pub fn bouncers(&self) -> impl Iterator<Item = &ServerSettings> {
        self.servers.iter().filter(|s| !s.is_standalone())
    }

    /// The selected server, or the first.
    pub fn current(&self) -> Option<&ServerSettings> {
        self.servers
            .iter()
            .find(|s| Some(s.id) == self.selected)
            .or_else(|| self.servers.first())
    }

    /// Adds `server`, or replaces the saved one with its id.
    pub fn save(&mut self, server: ServerSettings) {
        let id = server.id;
        match self.servers.iter_mut().find(|s| s.id == id) {
            Some(existing) => *existing = server,
            None => {
                // Before "No bouncer", which stays last.
                let at = self
                    .servers
                    .iter()
                    .position(ServerSettings::is_standalone)
                    .unwrap_or(self.servers.len());
                self.servers.insert(at, server);
            }
        }
        if self.selected.is_none() {
            self.selected = Some(id);
        }
    }

    pub fn remove(&mut self, id: Uuid) {
        self.servers.retain(|s| s.id != id || s.is_standalone());
        if self.selected == Some(id) {
            self.selected = self.servers.first().map(|s| s.id);
        }
    }

    pub fn load(path: &Path) -> ServerList {
        let mut list: ServerList = crate::storage::read_json(path).unwrap_or_default();
        list.add_standalone();
        list
    }

    pub fn save_to(&self, path: &Path) {
        crate::storage::write_json(path, self);
    }
}

/// What the network form edits, as soju network attributes.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NetworkFields {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub tls: bool,
    pub nick: String,
    /// Without a bouncer: how to log in to the network. `None` leaves it
    /// alone (soju keeps its own).
    pub login: Option<NetworkLogin>,
}

impl NetworkFields {
    /// The form's fields for a saved network's attributes.
    pub fn from_attributes(attributes: &HashMap<String, String>, standalone: bool) -> Self {
        let get = |k: &str| attributes.get(k).cloned().unwrap_or_default();
        let tls = attributes.get("tls").is_none_or(|t| t != "0");
        NetworkFields {
            name: get("name"),
            host: get("host"),
            port: attributes
                .get("port")
                .and_then(|p| p.parse().ok())
                .unwrap_or(if tls { 6697 } else { 6667 }),
            tls,
            nick: get("nickname"),
            login: standalone.then(|| NetworkLogin::from_attributes(attributes)),
        }
    }

    pub fn attributes(&self, keep_empty: bool) -> HashMap<String, String> {
        let mut attributes = HashMap::from([
            ("host".to_owned(), self.host.trim().to_owned()),
            ("port".to_owned(), self.port.to_string()),
            (
                "tls".to_owned(),
                if self.tls { "1" } else { "0" }.to_owned(),
            ),
        ]);
        for (key, value) in [("name", &self.name), ("nickname", &self.nick)] {
            let value = value.trim();
            if keep_empty || !value.is_empty() {
                attributes.insert(key.to_owned(), value.to_owned());
            }
        }
        if let Some(login) = &self.login {
            attributes.extend(login.attributes());
        }
        attributes
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NetworkLoginMethod {
    #[default]
    None,
    Sasl,
    ServerPassword,
    NickServ,
}

impl NetworkLoginMethod {
    pub const ALL: [NetworkLoginMethod; 4] = [
        NetworkLoginMethod::None,
        NetworkLoginMethod::Sasl,
        NetworkLoginMethod::ServerPassword,
        NetworkLoginMethod::NickServ,
    ];

    pub fn title(self) -> &'static str {
        match self {
            NetworkLoginMethod::None => "None",
            NetworkLoginMethod::Sasl => "SASL (account login)",
            NetworkLoginMethod::ServerPassword => "Server password",
            NetworkLoginMethod::NickServ => "NickServ",
        }
    }

    /// The secret attribute this method uses.
    pub fn secret(self) -> Option<&'static str> {
        match self {
            NetworkLoginMethod::None => None,
            NetworkLoginMethod::Sasl => Some("sasl-password"),
            NetworkLoginMethod::ServerPassword => Some("pass"),
            NetworkLoginMethod::NickServ => Some("nickserv-password"),
        }
    }
}

/// How a standalone network logs in, as the core's attributes. Passwords go
/// to the secret store; the network only says which ones are saved
/// (`pass-set`, …).
#[derive(Clone, PartialEq, Eq, Default)]
pub struct NetworkLogin {
    pub method: NetworkLoginMethod,
    /// The SASL account; the nick when empty.
    pub username: String,
    /// A new password; empty keeps the saved one.
    pub password: String,
}

impl std::fmt::Debug for NetworkLogin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NetworkLogin")
            .field("method", &self.method)
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .finish()
    }
}

impl NetworkLogin {
    /// The login saved on `attributes`, without the password.
    pub fn from_attributes(attributes: &HashMap<String, String>) -> Self {
        let method = NetworkLoginMethod::ALL
            .into_iter()
            .find(|m| {
                m.secret().is_some_and(|s| {
                    attributes.get(&format!("{s}-set")).map(String::as_str) == Some("1")
                })
            })
            .unwrap_or_default();
        NetworkLogin {
            method,
            username: attributes.get("sasl-username").cloned().unwrap_or_default(),
            password: String::new(),
        }
    }

    /// The other methods' passwords are cleared; this one's is set if typed.
    pub fn attributes(&self) -> HashMap<String, String> {
        let mut attributes = HashMap::new();
        for other in NetworkLoginMethod::ALL {
            if other != self.method
                && let Some(secret) = other.secret()
            {
                attributes.insert(secret.to_owned(), String::new());
            }
        }
        if let Some(secret) = self.method.secret()
            && !self.password.is_empty()
        {
            attributes.insert(secret.to_owned(), self.password.clone());
        }
        let username = if self.method == NetworkLoginMethod::Sasl {
            self.username.trim().to_owned()
        } else {
            String::new()
        };
        attributes.insert("sasl-username".into(), username);
        attributes
    }

    /// Whether this can be saved: a new method needs its password.
    pub fn is_complete(&self, saved: Option<&NetworkLogin>) -> bool {
        self.method == NetworkLoginMethod::None
            || !self.password.is_empty()
            || saved.is_some_and(|s| s.method == self.method)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standalone_stays_last_and_selection_falls_back() {
        let mut list = ServerList::default();
        assert_eq!(list.servers.len(), 1);
        assert!(list.current().unwrap().is_standalone());
        let server = ServerSettings::e2e_password();
        list.save(server.clone());
        assert!(list.servers.last().unwrap().is_standalone());
        list.selected = Some(server.id);
        list.remove(server.id);
        assert!(list.current().unwrap().is_standalone());
        list.remove(ServerSettings::standalone().id);
        assert_eq!(list.servers.len(), 1, "No bouncer can't be removed");
    }

    #[test]
    fn display_name_falls_back_to_host() {
        let mut s = ServerSettings::blank();
        s.server_url = "wss://user@irc.example.net:443/socket".into();
        assert_eq!(s.display_name(), "irc.example.net");
    }

    #[test]
    fn network_login_attributes() {
        let login = NetworkLogin {
            method: NetworkLoginMethod::Sasl,
            username: "me".into(),
            password: "pw".into(),
        };
        let a = login.attributes();
        assert_eq!(a["sasl-password"], "pw");
        assert_eq!(a["pass"], "");
        assert_eq!(a["nickserv-password"], "");
        assert_eq!(a["sasl-username"], "me");
        assert!(!format!("{login:?}").contains("pw"));

        let saved = HashMap::from([("pass-set".to_owned(), "1".to_owned())]);
        let back = NetworkLogin::from_attributes(&saved);
        assert_eq!(back.method, NetworkLoginMethod::ServerPassword);
        assert!(back.is_complete(Some(&back)));
        let switched = NetworkLogin {
            method: NetworkLoginMethod::NickServ,
            ..Default::default()
        };
        assert!(!switched.is_complete(Some(&back)));
    }

    #[test]
    fn client_config_nick_fallbacks() {
        let s = ServerSettings::e2e_password();
        let config = s.client_config(Path::new("/tmp/x.sqlite"), None, "Backroom test");
        assert_eq!(config.nick, "dave");
        assert!(matches!(config.login, LoginMethod::Password { .. }));
        let standalone = ServerSettings::standalone();
        let config = standalone.client_config(Path::new("/tmp/x.sqlite"), None, "v");
        assert_eq!(config.nick, "backroom");
    }
}
