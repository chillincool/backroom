use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::SystemTime;

use backroom_core::auth::{
    CredentialProvider, OidcClient, OidcConfig, OidcCredentials, PendingAuthorization,
    StaticCredentials, TokenStore,
};
use backroom_core::live::{self, Command, Event, SessionConfig, SessionHandle};
use backroom_core::sasl::SaslCredentials;
use backroom_core::transport::{TlsOptions, connector_for_url};
use secrecy::SecretString;
use tokio::runtime::Runtime;
use tokio::sync::mpsc::UnboundedReceiver;

use crate::store::{ForeignTokenStore, SecretStore};
use crate::{BackroomError, SessionEvent};

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ClientConfig {
    /// soju's URL: `wss://`, `ws://`, `ircs://` or `irc+insecure://`.
    pub server_url: String,
    pub login: LoginMethod,
    /// Nick to register with; soju replaces it with each network's nick.
    pub nick: String,
    /// Our answer to CTCP VERSION, e.g. "Backroom 0.1.0 (macOS)".
    pub client_version: String,
}

/// How the account logs in to soju.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum LoginMethod {
    /// soju's `auth oauth2`: a browser login (`begin_login`,
    /// `complete_login`), then SASL OAUTHBEARER with the access token.
    Oidc {
        /// OIDC issuer (the Keycloak realm URL).
        issuer: String,
        client_id: String,
        /// Scopes besides `openid`, e.g. `soju_roles`.
        scopes: Vec<String>,
        /// Where the login page sends the browser back to, e.g.
        /// `backroom://oauth/callback`. Must be registered on the client.
        redirect_uri: String,
    },
    /// soju's own accounts (`auth internal`, also `auth http`): SASL PLAIN
    /// with a password saved by `login_with_password`.
    Password { username: String },
}

struct Running {
    handle: SessionHandle,
    events: Arc<tokio::sync::Mutex<UnboundedReceiver<Event>>>,
}

/// One account on one soju. Owns a tokio runtime; every method is safe to
/// call from any thread.
#[derive(uniffi::Object)]
pub struct BackroomClient {
    runtime: Runtime,
    config: ClientConfig,
    /// OIDC tokens, in the secret store.
    store: Arc<dyn TokenStore>,
    /// The same secret store, holding the password for
    /// [`LoginMethod::Password`].
    secret: Arc<dyn SecretStore>,
    /// The password after the first read (each Keychain read can prompt).
    /// `None`: not read yet. `Some(None)`: read, nothing stored.
    password: Mutex<Option<Option<SecretString>>>,
    /// Filled by the first discovery, then reused.
    oidc: tokio::sync::Mutex<Option<OidcClient>>,
    pending: Mutex<Option<PendingAuthorization>>,
    session: Mutex<Option<Running>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl BackroomClient {
    async fn oidc(&self) -> Result<OidcClient, BackroomError> {
        let LoginMethod::Oidc {
            issuer,
            client_id,
            scopes,
            ..
        } = &self.config.login
        else {
            return Err(uses_password());
        };
        let mut cached = self.oidc.lock().await;
        if let Some(client) = cached.as_ref() {
            return Ok(client.clone());
        }
        let config = OidcConfig {
            issuer: issuer.clone(),
            client_id: client_id.clone(),
            scopes: scopes.clone(),
        };
        let client = self.runtime.spawn(OidcClient::discover(config)).await??;
        *cached = Some(client.clone());
        Ok(client)
    }

    fn stored_password(&self) -> Result<Option<SecretString>, BackroomError> {
        let mut cache = lock(&self.password);
        if let Some(cached) = cache.as_ref() {
            return Ok(cached.clone());
        }
        let password = self.secret.load()?.map(SecretString::from);
        *cache = Some(password.clone());
        Ok(password)
    }

    /// The credentials for `connect`, or `NotLoggedIn`.
    async fn credentials(&self) -> Result<Arc<dyn CredentialProvider>, BackroomError> {
        let not_logged_in = || BackroomError::NotLoggedIn {
            message: "no stored login".into(),
        };
        match &self.config.login {
            LoginMethod::Oidc { .. } => {
                if self.store.load()?.is_none() {
                    return Err(not_logged_in());
                }
                Ok(Arc::new(OidcCredentials::new(
                    self.oidc().await?,
                    self.store.clone(),
                )))
            }
            LoginMethod::Password { username } => {
                let password = self.stored_password()?.ok_or_else(not_logged_in)?;
                Ok(Arc::new(StaticCredentials(SaslCredentials::Plain {
                    username: username.clone(),
                    password,
                })))
            }
        }
    }

    fn command(&self, command: Command) -> Result<(), BackroomError> {
        let session = lock(&self.session);
        match session.as_ref() {
            Some(running) if running.handle.send(command) => Ok(()),
            _ => Err(BackroomError::NotConnected),
        }
    }
}

#[uniffi::export]
impl BackroomClient {
    #[uniffi::constructor]
    pub fn new(
        config: ClientConfig,
        secret_store: Arc<dyn SecretStore>,
    ) -> Result<Arc<Self>, BackroomError> {
        connector_for_url(&config.server_url, TlsOptions::default()).map_err(|e| {
            BackroomError::Config {
                message: e.to_string(),
            }
        })?;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("backroom")
            .enable_all()
            .build()
            .map_err(|e| BackroomError::Internal {
                message: e.to_string(),
            })?;
        Ok(Arc::new(BackroomClient {
            runtime,
            config,
            store: Arc::new(ForeignTokenStore::new(secret_store.clone())),
            secret: secret_store,
            password: Mutex::new(None),
            oidc: tokio::sync::Mutex::new(None),
            pending: Mutex::new(None),
            session: Mutex::new(None),
        }))
    }

    /// True if a login (tokens or password) is stored, so `connect` can be
    /// tried without logging in.
    pub fn has_login(&self) -> Result<bool, BackroomError> {
        Ok(match self.config.login {
            LoginMethod::Oidc { .. } => self.store.load()?.is_some(),
            LoginMethod::Password { .. } => self.stored_password()?.is_some(),
        })
    }

    /// Saves the password for a [`LoginMethod::Password`] account. soju
    /// checks it on `connect`; a wrong one ends in `AuthFailed`.
    pub fn login_with_password(&self, password: String) -> Result<(), BackroomError> {
        if !matches!(self.config.login, LoginMethod::Password { .. }) {
            return Err(BackroomError::Config {
                message: "this account logs in with single sign-on".into(),
            });
        }
        let password = SecretString::from(password);
        let mut cache = lock(&self.password);
        self.secret
            .save(secrecy::ExposeSecret::expose_secret(&password).to_owned())?;
        *cache = Some(Some(password));
        Ok(())
    }

    /// Starts a single sign-on login and returns the URL to open in the
    /// browser. Pass the URL it redirects to to `complete_login`.
    pub async fn begin_login(&self) -> Result<String, BackroomError> {
        let LoginMethod::Oidc { redirect_uri, .. } = &self.config.login else {
            return Err(uses_password());
        };
        let pending = self.oidc().await?.begin(redirect_uri)?;
        let url = pending.auth_url.to_string();
        *lock(&self.pending) = Some(pending);
        Ok(url)
    }

    /// Finishes the login with the redirect URL and saves the tokens.
    pub async fn complete_login(&self, callback_url: String) -> Result<(), BackroomError> {
        let pending = lock(&self.pending)
            .take()
            .ok_or(BackroomError::NoLoginInProgress)?;
        let oidc = self.oidc().await?;
        let tokens = self
            .runtime
            .spawn(async move { oidc.complete(pending, &callback_url).await })
            .await??;
        self.store.save(&tokens)?;
        Ok(())
    }

    /// Starts (or restarts) the session with the stored login. Progress and
    /// failures, including a rejected login, arrive through `next_event`.
    pub async fn connect(&self) -> Result<(), BackroomError> {
        let credentials = self.credentials().await?;
        let connector =
            connector_for_url(&self.config.server_url, TlsOptions::default()).map_err(|e| {
                BackroomError::Config {
                    message: e.to_string(),
                }
            })?;
        let mut config =
            SessionConfig::new(Arc::from(connector), credentials, self.config.nick.clone());
        config.client_version = self.config.client_version.clone();
        let (handle, events) = self.runtime.spawn(async { live::start(config) }).await?;
        let running = Running {
            handle,
            events: Arc::new(tokio::sync::Mutex::new(events)),
        };
        if let Some(old) = lock(&self.session).replace(running) {
            old.handle.shutdown();
        }
        Ok(())
    }

    /// Waits for the next event. Returns nil when no session is running,
    /// including after the `Stopped` event. Call it in a loop from one task.
    pub async fn next_event(&self) -> Option<SessionEvent> {
        let events = lock(&self.session).as_ref()?.events.clone();
        let event = events.lock().await.recv().await;
        if matches!(event, None | Some(Event::Stopped)) {
            let mut session = lock(&self.session);
            if session
                .as_ref()
                .is_some_and(|r| Arc::ptr_eq(&r.events, &events))
            {
                *session = None;
            }
        }
        event.map(Into::into)
    }

    pub fn send_message(
        &self,
        network: String,
        target: String,
        text: String,
    ) -> Result<(), BackroomError> {
        self.command(Command::SendMessage {
            network,
            target,
            text,
        })
    }

    /// Joins channels (comma-separated); `key` is for `+k` channels.
    pub fn join(
        &self,
        network: String,
        channel: String,
        key: Option<String>,
    ) -> Result<(), BackroomError> {
        self.command(Command::Join {
            network,
            channel,
            key,
        })
    }

    /// Searches a buffer's history on soju; answered by `SearchResults`.
    pub fn search(
        &self,
        network: String,
        target: String,
        text: String,
    ) -> Result<(), BackroomError> {
        self.command(Command::Search {
            network,
            target,
            text,
        })
    }

    /// Tells the others in a conversation whether we're typing.
    pub fn set_typing(
        &self,
        network: String,
        target: String,
        state: crate::types::TypingState,
    ) -> Result<(), BackroomError> {
        self.command(Command::Typing {
            network,
            target,
            state: state.into(),
        })
    }

    /// Asks for a channel's modes and ban list (`ChannelModes`, `BanList`).
    pub fn load_channel_settings(
        &self,
        network: String,
        channel: String,
    ) -> Result<(), BackroomError> {
        self.command(Command::LoadChannelSettings { network, channel })
    }

    /// Asks for the network's channel list; it arrives as `ChannelList`
    /// events.
    pub fn list_channels(&self, network: String) -> Result<(), BackroomError> {
        self.command(Command::ListChannels { network })
    }

    pub fn part(
        &self,
        network: String,
        channel: String,
        reason: Option<String>,
    ) -> Result<(), BackroomError> {
        self.command(Command::Part {
            network,
            channel,
            reason,
        })
    }

    /// Adds a network to soju. Attributes: `name`, `host`, `port`, `tls`
    /// (`1`/`0`), `nickname`, … It shows up as `NetworkUpdated`; a
    /// rejection arrives as an `Error` event.
    pub fn add_network(&self, attributes: HashMap<String, String>) -> Result<(), BackroomError> {
        self.command(Command::AddNetwork {
            attributes: attributes.into_iter().collect(),
        })
    }

    /// Changes a network's attributes; an empty value removes one.
    pub fn change_network(
        &self,
        network: String,
        attributes: HashMap<String, String>,
    ) -> Result<(), BackroomError> {
        self.command(Command::ChangeNetwork {
            network,
            attributes: attributes.into_iter().collect(),
        })
    }

    /// Deletes a network from soju, with its channels and history.
    pub fn remove_network(&self, network: String) -> Result<(), BackroomError> {
        self.command(Command::RemoveNetwork { network })
    }

    /// Sends a raw IRC line on a network, e.g. `WHOIS alice`.
    pub fn send_raw(&self, network: String, line: String) -> Result<(), BackroomError> {
        self.command(Command::SendRaw { network, line })
    }

    /// Leaves a channel (its buffer goes once the server confirms) or
    /// closes a private conversation.
    pub fn close_buffer(&self, network: String, name: String) -> Result<(), BackroomError> {
        self.command(Command::CloseBuffer { network, name })
    }

    /// Asks for up to `limit` messages before `before` (or the latest);
    /// they arrive as a `History` event.
    pub fn load_history(
        &self,
        network: String,
        target: String,
        before: Option<SystemTime>,
        limit: u32,
    ) -> Result<(), BackroomError> {
        self.command(Command::LoadHistory {
            network,
            target,
            before: before.map(Into::into),
            limit: limit as usize,
        })
    }

    /// Marks everything up to `time` as read, on every device.
    pub fn mark_read(
        &self,
        network: String,
        target: String,
        time: SystemTime,
    ) -> Result<(), BackroomError> {
        self.command(Command::MarkRead {
            network,
            target,
            time: time.into(),
        })
    }

    /// Stops the session; `next_event` then delivers `Stopped`.
    pub fn disconnect(&self) {
        if let Some(running) = lock(&self.session).as_ref() {
            running.handle.shutdown();
        }
    }

    /// Disconnects and deletes the stored login.
    pub fn logout(&self) -> Result<(), BackroomError> {
        self.disconnect();
        lock(&self.pending).take();
        let mut password = lock(&self.password);
        self.store.clear()?;
        *password = Some(None);
        Ok(())
    }
}

fn uses_password() -> BackroomError {
    BackroomError::Config {
        message: "this account logs in with a password".into(),
    }
}

#[cfg(test)]
mod tests {
    use crate::store::tests::MemoryStore;

    use super::*;

    fn config(server_url: &str) -> ClientConfig {
        ClientConfig {
            server_url: server_url.into(),
            login: LoginMethod::Oidc {
                issuer: "http://127.0.0.1:1/realms/none".into(),
                client_id: "backroom".into(),
                scopes: vec!["soju_roles".into()],
                redirect_uri: "backroom://oauth/callback".into(),
            },
            nick: "dave".into(),
            client_version: "Backroom test".into(),
        }
    }

    fn client() -> Arc<BackroomClient> {
        BackroomClient::new(
            config("ws://127.0.0.1:1/socket"),
            Arc::new(MemoryStore::default()),
        )
        .unwrap()
    }

    #[test]
    fn rejects_unsupported_server_url() {
        let err = BackroomClient::new(
            config("irc://irc.example.net"),
            Arc::new(MemoryStore::default()),
        )
        .err()
        .unwrap();
        assert!(matches!(err, BackroomError::Config { .. }), "{err:?}");
    }

    #[test]
    fn commands_need_a_session() {
        let client = client();
        let err = client
            .join("1".into(), "#backroom".into(), None)
            .unwrap_err();
        assert!(matches!(err, BackroomError::NotConnected));
        client.disconnect();
    }

    #[test]
    fn connect_needs_a_login() {
        let client = client();
        assert!(!client.has_login().unwrap());
        let err = block_on_elsewhere(client.connect()).unwrap_err();
        assert!(matches!(err, BackroomError::NotLoggedIn { .. }), "{err:?}");
        assert!(block_on_elsewhere(client.next_event()).is_none());
    }

    #[test]
    fn complete_without_begin() {
        let client = client();
        let err = block_on_elsewhere(client.complete_login("backroom://oauth/callback".into()))
            .unwrap_err();
        assert!(matches!(err, BackroomError::NoLoginInProgress));
    }

    #[test]
    fn logout_clears_the_store() {
        let memory = Arc::new(MemoryStore(Mutex::new(Some("{}".into()))));
        let client =
            BackroomClient::new(config("ws://127.0.0.1:1/socket"), memory.clone()).unwrap();
        client.logout().unwrap();
        assert!(memory.0.lock().unwrap().is_none());
    }

    fn password_client(memory: Arc<MemoryStore>) -> Arc<BackroomClient> {
        let mut config = config("ws://127.0.0.1:1/socket");
        config.login = LoginMethod::Password {
            username: "dave".into(),
        };
        BackroomClient::new(config, memory).unwrap()
    }

    #[test]
    fn password_login_is_saved_and_connects() {
        let memory = Arc::new(MemoryStore::default());
        let client = password_client(memory.clone());
        assert!(!client.has_login().unwrap());
        let err = block_on_elsewhere(client.connect()).unwrap_err();
        assert!(matches!(err, BackroomError::NotLoggedIn { .. }), "{err:?}");

        client.login_with_password("dave-secret".into()).unwrap();
        assert_eq!(memory.0.lock().unwrap().as_deref(), Some("dave-secret"));
        assert!(client.has_login().unwrap());
        // Nothing listens on port 1: the session starts and keeps retrying.
        block_on_elsewhere(client.connect()).unwrap();
        client.logout().unwrap();
        assert!(memory.0.lock().unwrap().is_none());
        assert!(!client.has_login().unwrap());
    }

    #[test]
    fn stored_password_is_found_on_the_next_launch() {
        let memory = Arc::new(MemoryStore(Mutex::new(Some("dave-secret".into()))));
        let client = password_client(memory);
        assert!(client.has_login().unwrap());
        block_on_elsewhere(client.connect()).unwrap();
        client.disconnect();
    }

    #[test]
    fn methods_of_the_other_login_are_refused() {
        let password = password_client(Arc::new(MemoryStore::default()));
        let err = block_on_elsewhere(password.begin_login()).unwrap_err();
        assert!(matches!(err, BackroomError::Config { .. }), "{err:?}");

        let sso = client();
        let err = sso.login_with_password("x".into()).unwrap_err();
        assert!(matches!(err, BackroomError::Config { .. }), "{err:?}");
    }

    /// Polls a future on a separate runtime, the way Swift's executor polls
    /// it from outside the client's own runtime.
    fn block_on_elsewhere<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(f)
    }
}
