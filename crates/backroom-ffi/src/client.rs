use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::SystemTime;

use backroom_core::auth::{
    OidcClient, OidcConfig, OidcCredentials, PendingAuthorization, TokenStore,
};
use backroom_core::live::{self, Command, Event, SessionConfig, SessionHandle};
use backroom_core::transport::{TlsOptions, connector_for_url};
use tokio::runtime::Runtime;
use tokio::sync::mpsc::UnboundedReceiver;

use crate::store::{ForeignTokenStore, SecretStore};
use crate::{BackroomError, SessionEvent};

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ClientConfig {
    /// soju's URL: `wss://`, `ws://`, `ircs://` or `irc+insecure://`.
    pub server_url: String,
    /// OIDC issuer (the Keycloak realm URL).
    pub issuer: String,
    pub client_id: String,
    /// Scopes besides `openid`, e.g. `soju_roles`.
    pub scopes: Vec<String>,
    /// Where the login page sends the browser back to, e.g.
    /// `backroom://oauth/callback`. Must be registered on the client.
    pub redirect_uri: String,
    /// Nick to register with; soju replaces it with each network's nick.
    pub nick: String,
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
    store: Arc<dyn TokenStore>,
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
        let mut cached = self.oidc.lock().await;
        if let Some(client) = cached.as_ref() {
            return Ok(client.clone());
        }
        let config = OidcConfig {
            issuer: self.config.issuer.clone(),
            client_id: self.config.client_id.clone(),
            scopes: self.config.scopes.clone(),
        };
        let client = self.runtime.spawn(OidcClient::discover(config)).await??;
        *cached = Some(client.clone());
        Ok(client)
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
            store: Arc::new(ForeignTokenStore(secret_store)),
            oidc: tokio::sync::Mutex::new(None),
            pending: Mutex::new(None),
            session: Mutex::new(None),
        }))
    }

    /// True if tokens are stored, so `connect` can be tried without a login.
    pub fn has_login(&self) -> Result<bool, BackroomError> {
        Ok(self.store.load()?.is_some())
    }

    /// Starts a login and returns the URL to open in the login sheet. Pass
    /// the URL it redirects to to `complete_login`.
    pub async fn begin_login(&self) -> Result<String, BackroomError> {
        let pending = self.oidc().await?.begin(&self.config.redirect_uri)?;
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
        if self.store.load()?.is_none() {
            return Err(BackroomError::NotLoggedIn {
                message: "no stored login".into(),
            });
        }
        let connector =
            connector_for_url(&self.config.server_url, TlsOptions::default()).map_err(|e| {
                BackroomError::Config {
                    message: e.to_string(),
                }
            })?;
        let credentials = OidcCredentials::new(self.oidc().await?, self.store.clone());
        let config = SessionConfig::new(
            Arc::from(connector),
            Arc::new(credentials),
            self.config.nick.clone(),
        );
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

    pub fn join(&self, network: String, channel: String) -> Result<(), BackroomError> {
        self.command(Command::Join { network, channel })
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
        self.store.clear()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::store::tests::MemoryStore;

    use super::*;

    fn config(server_url: &str) -> ClientConfig {
        ClientConfig {
            server_url: server_url.into(),
            issuer: "http://127.0.0.1:1/realms/none".into(),
            client_id: "backroom".into(),
            scopes: vec!["soju_roles".into()],
            redirect_uri: "backroom://oauth/callback".into(),
            nick: "dave".into(),
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
        let err = client.join("1".into(), "#backroom".into()).unwrap_err();
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

    /// Polls a future on a separate runtime, the way Swift's executor polls
    /// it from outside the client's own runtime.
    fn block_on_elsewhere<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(f)
    }
}
