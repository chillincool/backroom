//! End-to-end tests against the Docker Compose stack in test/e2e.
//!
//!     scripts/e2e.sh all          # or: mise run e2e
//!
//! Every login method soju supports (except PAM) is tested over WebSocket,
//! ircs:// and irc+insecure://. Keycloak logins go through the real login
//! form, like a browser, and Backroom's own PKCE code.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use backroom_core::auth::jwt_peek::peek_claims;
use backroom_core::auth::{
    CredentialProvider, OidcClient, OidcConfig, OidcCredentials, StaticCredentials, StoreError,
    TokenSet, TokenStore,
};
use backroom_core::proto::Message;
use backroom_core::sasl::SaslCredentials;
use backroom_core::session::RegistrationError;
use backroom_core::transport::{TlsOptions, TransportError, connector_for_url};
use backroom_core::{Client, ClientError, ConnectOptions};
use chrono::{TimeDelta, Utc};
use secrecy::{ExposeSecret, SecretString};

const ISSUER: &str = "http://localhost:18080/realms/e2e";
const REDIRECT_URI: &str = "http://127.0.0.1:47615/callback";

/// A soju instance's three listeners.
struct Soju {
    ws: &'static str,
    irc: &'static str,
    ircs: &'static str,
}

impl Soju {
    fn urls(&self) -> [&'static str; 3] {
        [self.ws, self.irc, self.ircs]
    }
}

const SOJU_OAUTH2: Soju = Soju {
    ws: "ws://127.0.0.1:18081/",
    irc: "irc+insecure://127.0.0.1:16667",
    ircs: "ircs://localhost:16697",
};
const SOJU_INTERNAL: Soju = Soju {
    ws: "ws://127.0.0.1:28081/",
    irc: "irc+insecure://127.0.0.1:26667",
    ircs: "ircs://localhost:26697",
};
const SOJU_HTTP: Soju = Soju {
    ws: "ws://127.0.0.1:38081/",
    irc: "irc+insecure://127.0.0.1:36667",
    ircs: "ircs://localhost:36697",
};

/// TLS options trusting the e2e CA from scripts/e2e.sh.
fn tls() -> TlsOptions {
    let path = std::env::var("BACKROOM_E2E_CA").unwrap_or_else(|_| {
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../.e2e/certs/ca.crt").to_owned()
    });
    TlsOptions::default()
        .with_ca_file(path.as_ref())
        .expect("e2e CA missing; run scripts/e2e.sh up")
}

/// Short unique suffix for nicks, networks and channels.
fn unique() -> String {
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!("{}{n}", Utc::now().timestamp_millis() % 1_000_000)
}

async fn connect(
    url: &str,
    creds: &dyn CredentialProvider,
    nick: &str,
    bind: Option<String>,
) -> Result<Client, ClientError> {
    let connector = connector_for_url(url, tls()).unwrap();
    let mut opts = ConnectOptions::new(nick);
    opts.bind_network = bind;
    Client::connect(connector.as_ref(), creds, opts).await
}

fn plain(user: &str, password: &str) -> StaticCredentials {
    StaticCredentials(SaslCredentials::Plain {
        username: user.into(),
        password: SecretString::from(password.to_owned()),
    })
}

fn bearer(token: &SecretString) -> StaticCredentials {
    StaticCredentials(SaslCredentials::OAuthBearer {
        token: token.clone(),
    })
}

fn assert_sasl_failed(result: Result<Client, ClientError>, url: &str) -> String {
    match result {
        Err(ClientError::Registration(RegistrationError::SaslFailed { line, .. })) => {
            assert!(
                line.contains(" 904 "),
                "{url}: unexpected failure line {line}"
            );
            line
        }
        Err(e) => panic!("{url}: expected a SASL failure, got {e}"),
        Ok(_) => panic!("{url}: login unexpectedly succeeded"),
    }
}

// ---------------------------------------------------------------------------
// Keycloak

async fn oidc() -> OidcClient {
    OidcClient::discover(OidcConfig {
        issuer: ISSUER.into(),
        client_id: "backroom".into(),
        scopes: vec!["soju_roles".into()],
    })
    .await
    .expect("Keycloak discovery")
}

/// Logs in through Keycloak's login form, like a browser, using Backroom's
/// PKCE flow. Returns the login page's text if Keycloak doesn't redirect
/// back (e.g. the gate denied access).
async fn browser_login(oidc: &OidcClient, user: &str) -> Result<TokenSet, String> {
    let pending = oidc.begin(REDIRECT_URI).unwrap();
    let http = reqwest::Client::builder()
        .cookie_store(true)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();

    let page = http
        .get(pending.auth_url.as_str())
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let action = page
        .split("action=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap_or_else(|| panic!("no login form in: {page}"))
        .replace("&amp;", "&");

    let response = http
        .post(&action)
        .form(&[("username", user), ("password", user)])
        .send()
        .await
        .unwrap();
    let location = response
        .headers()
        .get("location")
        .and_then(|l| l.to_str().ok())
        .map(str::to_owned);
    match location {
        Some(callback) if callback.starts_with(REDIRECT_URI) => {
            Ok(oidc.complete(pending, &callback).await.unwrap())
        }
        _ => Err(response.text().await.unwrap_or_default()),
    }
}

/// Password grant via `e2e-direct`, which skips the browser flow and its
/// gate: stands in for a token obtained some other way.
async fn direct_grant(user: &str) -> SecretString {
    let body: serde_json::Value = reqwest::Client::new()
        .post(format!("{ISSUER}/protocol/openid-connect/token"))
        .form(&[
            ("grant_type", "password"),
            ("client_id", "e2e-direct"),
            ("username", user),
            ("password", user),
            ("scope", "openid"),
        ])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    SecretString::from(
        body["access_token"]
            .as_str()
            .unwrap_or_else(|| panic!("no token for {user}: {body}"))
            .to_owned(),
    )
}

#[derive(Default)]
struct MemoryStore(Mutex<Option<TokenSet>>);

impl TokenStore for MemoryStore {
    fn load(&self) -> Result<Option<TokenSet>, StoreError> {
        Ok(self.0.lock().unwrap().clone())
    }
    fn save(&self, tokens: &TokenSet) -> Result<(), StoreError> {
        *self.0.lock().unwrap() = Some(tokens.clone());
        Ok(())
    }
    fn clear(&self) -> Result<(), StoreError> {
        *self.0.lock().unwrap() = None;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// auth oauth2

#[tokio::test]
#[ignore = "needs the e2e stack (scripts/e2e.sh)"]
async fn oauth2_alice_logs_in_on_every_transport() {
    let tokens = browser_login(&oidc().await, "alice")
        .await
        .expect("alice should get through the gate");
    let claims = peek_claims(tokens.access_token.expose_secret()).unwrap();
    assert!(
        claims.audience.contains(&"soju".to_owned()),
        "aud: {:?}",
        claims.audience
    );

    for url in SOJU_OAUTH2.urls() {
        // OAUTHBEARER
        let mut client = connect(url, &bearer(&tokens.access_token), "alice", None)
            .await
            .unwrap_or_else(|e| panic!("{url} OAUTHBEARER: {e}"));
        assert_eq!(client.info().account.as_deref(), Some("alice"));
        client.list_networks().await.unwrap();
        client.quit().await.ok();

        // PLAIN with the token as the password
        let creds = plain("alice", tokens.access_token.expose_secret());
        let client = connect(url, &creds, "alice", None)
            .await
            .unwrap_or_else(|e| panic!("{url} PLAIN+token: {e}"));
        client.quit().await.ok();
    }
}

#[tokio::test]
#[ignore = "needs the e2e stack (scripts/e2e.sh)"]
async fn oauth2_gate_refuses_mallory_at_login() {
    let page = browser_login(&oidc().await, "mallory")
        .await
        .expect_err("mallory has no soju.access and must not get a code");
    assert!(
        page.contains("soju access required"),
        "unexpected page: {page}"
    );
}

#[tokio::test]
#[ignore = "needs the e2e stack (scripts/e2e.sh)"]
async fn oauth2_soju_rejects_tokens_without_the_role() {
    // Both tokens skip the browser gate; only the role differs.
    let mallory = direct_grant("mallory").await;
    let claims = peek_claims(mallory.expose_secret()).unwrap();
    assert!(
        !claims.audience.contains(&"soju".to_owned()),
        "aud: {:?}",
        claims.audience
    );
    let alice = direct_grant("alice").await;

    for url in SOJU_OAUTH2.urls() {
        let line = assert_sasl_failed(connect(url, &bearer(&mallory), "mallory", None).await, url);
        assert!(line.ends_with(":Invalid credentials"), "{url}: {line}");
        connect(url, &bearer(&alice), "alice", None)
            .await
            .unwrap_or_else(|e| panic!("{url} alice via direct grant: {e}"))
            .quit()
            .await
            .ok();
    }
}

#[tokio::test]
#[ignore = "needs the e2e stack (scripts/e2e.sh)"]
async fn oauth2_refreshes_an_expired_token_before_connecting() {
    let oidc = oidc().await;
    let mut tokens = browser_login(&oidc, "alice").await.unwrap();
    let stale = tokens.access_token.expose_secret().to_owned();
    tokens.expires_at = Some(Utc::now() - TimeDelta::seconds(1));

    let store = Arc::new(MemoryStore::default());
    store.save(&tokens).unwrap();
    let creds = OidcCredentials::new(oidc, store.clone());

    connect(SOJU_OAUTH2.ws, &creds, "alice", None)
        .await
        .unwrap()
        .quit()
        .await
        .ok();
    let saved = store.load().unwrap().unwrap();
    assert_ne!(
        saved.access_token.expose_secret(),
        stale,
        "token was not refreshed"
    );
    assert!(!saved.expires_within(TimeDelta::seconds(60)));
}

// ---------------------------------------------------------------------------
// auth internal and auth http

#[tokio::test]
#[ignore = "needs the e2e stack (scripts/e2e.sh)"]
async fn internal_plain_on_every_transport() {
    for url in SOJU_INTERNAL.urls() {
        let client = connect(url, &plain("dave", "dave-secret"), "dave", None)
            .await
            .unwrap_or_else(|e| panic!("{url}: {e}"));
        assert_eq!(client.info().account.as_deref(), Some("dave"));
        client.quit().await.ok();
        assert_sasl_failed(
            connect(url, &plain("dave", "wrong"), "dave", None).await,
            url,
        );
    }
}

#[tokio::test]
#[ignore = "needs the e2e stack (scripts/e2e.sh)"]
async fn http_plain_on_every_transport() {
    for url in SOJU_HTTP.urls() {
        let client = connect(url, &plain("carol", "http-secret"), "carol", None)
            .await
            .unwrap_or_else(|e| panic!("{url}: {e}"));
        assert_eq!(client.info().account.as_deref(), Some("carol"));
        client.quit().await.ok();
        assert_sasl_failed(
            connect(url, &plain("carol", "wrong"), "carol", None).await,
            url,
        );
    }
}

#[tokio::test]
#[ignore = "needs the e2e stack (scripts/e2e.sh)"]
async fn ircs_rejects_an_untrusted_certificate() {
    // No e2e CA: the throwaway certificate must not be accepted.
    let connector = connector_for_url(SOJU_INTERNAL.ircs, TlsOptions::default()).unwrap();
    let result = Client::connect(
        connector.as_ref(),
        &plain("dave", "dave-secret"),
        ConnectOptions::new("dave"),
    )
    .await;
    assert!(
        matches!(result, Err(ClientError::Transport(TransportError::Tls(_)))),
        "expected a TLS error, got {:?}",
        result.err()
    );
}

// ---------------------------------------------------------------------------
// Bouncer networks and chat history, with ergo as the upstream server

#[tokio::test]
#[ignore = "needs the e2e stack (scripts/e2e.sh)"]
async fn history_round_trip_through_ergo() {
    let id = unique();
    let creds = plain("dave", "dave-secret");
    let network = format!("ergo-{id}");
    let nick = format!("dave{id}");
    let channel = format!("#e2e-{id}");

    // Add ergo as an upstream network and wait until soju is connected.
    let mut control = connect(SOJU_INTERNAL.irc, &creds, "dave", None)
        .await
        .unwrap();
    control
        .send(&Message::new(
            "BOUNCER",
            [
                "ADDNETWORK".to_owned(),
                format!("name={network};host=ergo;port=6667;tls=0;nickname={nick}"),
            ],
        ))
        .await
        .unwrap();
    let netid = {
        let mut found = None;
        for _ in 0..30 {
            let networks = control.list_networks().await.unwrap();
            if let Some(n) = networks.iter().find(|n| n.name() == Some(network.as_str()))
                && n.state() == Some("connected")
            {
                found = Some(n.id.clone());
                break;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        found.expect("soju never connected to ergo")
    };
    control.quit().await.ok();

    // Bound connection: join, talk, then read it back.
    for url in SOJU_INTERNAL.urls() {
        let mut client = connect(url, &creds, &nick, Some(netid.clone()))
            .await
            .unwrap_or_else(|e| panic!("{url}: {e}"));
        client
            .send(&Message::new("JOIN", [channel.clone()]))
            .await
            .unwrap();
        client
            .wait_for("JOIN", |m| {
                m.command == "JOIN" && m.param(0) == Some(channel.as_str())
            })
            .await
            .unwrap();
        let text = format!("hello via {url}");
        client
            .send(&Message::new("PRIVMSG", [channel.clone(), text.clone()]))
            .await
            .unwrap();
        // echo-message: soju confirms it took the message.
        client
            .wait_for("echo", |m| {
                m.command == "PRIVMSG" && m.param(1) == Some(text.as_str())
            })
            .await
            .unwrap();
        client.quit().await.ok();
    }

    let mut client = connect(SOJU_INTERNAL.ws, &creds, &nick, Some(netid))
        .await
        .unwrap();
    let history = client.chathistory_latest(&channel, 50).await.unwrap();
    let texts: Vec<&str> = history
        .iter()
        .filter(|m| m.command == "PRIVMSG")
        .filter_map(|m| m.param(1))
        .collect();
    let expected: Vec<String> = SOJU_INTERNAL
        .urls()
        .iter()
        .map(|url| format!("hello via {url}"))
        .collect();
    assert_eq!(texts, expected, "history in order");
    assert!(
        history.iter().all(|m| m.tag("time").is_some()),
        "every message has server-time"
    );
    client.quit().await.ok();
}
