//! Authentication: OIDC tokens, token storage, and SASL credential providers.
//!
//! Platform pieces stay outside the core:
//! - Opening a browser and receiving the redirect is done by the front end
//!   (loopback listener in the CLI, `ASWebAuthenticationSession` on Apple),
//!   between [`oidc::OidcClient::begin`] and [`oidc::OidcClient::complete`].
//! - Persisting tokens goes through [`TokenStore`] (a file in the CLI,
//!   the Keychain on Apple).

pub mod jwt_peek;
pub mod oidc;

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, TimeDelta, Utc};
use secrecy::SecretString;

use crate::sasl::SaslCredentials;
pub use oidc::{OidcClient, OidcConfig, PendingAuthorization};

/// Refresh access tokens that expire within this window before connecting.
const REFRESH_MARGIN: TimeDelta = TimeDelta::seconds(60);

#[derive(Clone)]
pub struct TokenSet {
    pub access_token: SecretString,
    pub refresh_token: Option<SecretString>,
    pub expires_at: Option<DateTime<Utc>>,
    /// Granted scopes, if the token endpoint returned them.
    pub scope: Option<String>,
}

impl fmt::Debug for TokenSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TokenSet")
            .field("access_token", &"[redacted]")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "[redacted]"),
            )
            .field("expires_at", &self.expires_at)
            .field("scope", &self.scope)
            .finish()
    }
}

impl TokenSet {
    /// True if the access token is expired or expires within `margin`.
    /// Tokens without a known expiry are treated as valid.
    pub fn expires_within(&self, margin: TimeDelta) -> bool {
        self.expires_at
            .is_some_and(|exp| exp - margin <= Utc::now())
    }
}

#[derive(Debug, thiserror::Error)]
#[error("token store: {0}")]
pub struct StoreError(pub String);

/// Persists tokens between runs. Implemented by each front end.
pub trait TokenStore: Send + Sync {
    fn load(&self) -> Result<Option<TokenSet>, StoreError>;
    fn save(&self, tokens: &TokenSet) -> Result<(), StoreError>;
    fn clear(&self) -> Result<(), StoreError>;
}

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("OIDC discovery failed: {0}")]
    Discovery(String),
    #[error("invalid URL: {0}")]
    InvalidUrl(String),
    #[error("authorization denied: {error}{}", description.as_deref().map(|d| format!(" ({d})")).unwrap_or_default())]
    AuthorizationDenied {
        error: String,
        description: Option<String>,
    },
    #[error("authorization response state mismatch (possible CSRF); try logging in again")]
    StateMismatch,
    #[error("authorization response has no code")]
    MissingCode,
    #[error("token request failed: {0}")]
    TokenRequest(String),
    #[error("not logged in: {0}")]
    NotLoggedIn(String),
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// Supplies SASL credentials for each (re)connect.
#[async_trait]
pub trait CredentialProvider: Send + Sync {
    async fn credentials(&self) -> Result<SaslCredentials, AuthError>;
}

/// Fixed credentials, e.g. SASL PLAIN with a soju password.
pub struct StaticCredentials(pub SaslCredentials);

#[async_trait]
impl CredentialProvider for StaticCredentials {
    async fn credentials(&self) -> Result<SaslCredentials, AuthError> {
        Ok(self.0.clone())
    }
}

/// OAUTHBEARER credentials backed by stored OIDC tokens, refreshed when
/// they are about to expire.
pub struct OidcCredentials {
    oidc: OidcClient,
    store: Arc<dyn TokenStore>,
    current: tokio::sync::Mutex<Option<TokenSet>>,
}

impl OidcCredentials {
    pub fn new(oidc: OidcClient, store: Arc<dyn TokenStore>) -> Self {
        OidcCredentials {
            oidc,
            store,
            current: tokio::sync::Mutex::new(None),
        }
    }

    /// Returns a token set whose access token is valid for at least the
    /// refresh margin, refreshing and persisting it if needed.
    pub async fn fresh_tokens(&self) -> Result<TokenSet, AuthError> {
        let mut current = self.current.lock().await;
        if current.is_none() {
            *current = self.store.load()?;
        }
        let Some(tokens) = current.as_ref() else {
            return Err(AuthError::NotLoggedIn("no stored tokens".into()));
        };
        if !tokens.expires_within(REFRESH_MARGIN) {
            return Ok(tokens.clone());
        }

        let Some(refresh_token) = tokens.refresh_token.clone() else {
            return Err(AuthError::NotLoggedIn(
                "access token expired and no refresh token is stored".into(),
            ));
        };
        tracing::debug!("access token expiring; refreshing");
        let mut refreshed = self.oidc.refresh(&refresh_token).await?;
        // Keycloak normally rotates refresh tokens, but keep the old one if not.
        if refreshed.refresh_token.is_none() {
            refreshed.refresh_token = Some(refresh_token);
        }
        self.store.save(&refreshed)?;
        *current = Some(refreshed.clone());
        Ok(refreshed)
    }
}

#[async_trait]
impl CredentialProvider for OidcCredentials {
    async fn credentials(&self) -> Result<SaslCredentials, AuthError> {
        let tokens = self.fresh_tokens().await?;
        Ok(SaslCredentials::OAuthBearer {
            token: tokens.access_token,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(expires_in: Option<i64>) -> TokenSet {
        TokenSet {
            access_token: SecretString::from("access-secret"),
            refresh_token: Some(SecretString::from("refresh-secret")),
            expires_at: expires_in.map(|s| Utc::now() + TimeDelta::seconds(s)),
            scope: Some("openid soju_roles".into()),
        }
    }

    #[test]
    fn expiry_margin() {
        assert!(tokens(Some(30)).expires_within(REFRESH_MARGIN));
        assert!(tokens(Some(-5)).expires_within(REFRESH_MARGIN));
        assert!(!tokens(Some(300)).expires_within(REFRESH_MARGIN));
        assert!(!tokens(None).expires_within(REFRESH_MARGIN));
    }

    #[test]
    fn debug_is_redacted() {
        let dbg = format!("{:?}", tokens(Some(300)));
        assert!(!dbg.contains("access-secret"));
        assert!(!dbg.contains("refresh-secret"));
        assert!(dbg.contains("soju_roles"));
    }
}
