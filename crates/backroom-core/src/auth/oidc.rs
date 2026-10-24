//! OpenID Connect authorization code flow with PKCE (RFC 7636) for a
//! public client, plus refresh. Token endpoint calls use the `oauth2` crate.

use chrono::{TimeDelta, Utc};
use oauth2::basic::BasicClient;
use oauth2::{
    AuthUrl, AuthorizationCode, ClientId, CsrfToken, PkceCodeChallenge, PkceCodeVerifier,
    RedirectUrl, RefreshToken, RequestTokenError, Scope, TokenResponse, TokenUrl,
};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use url::Url;

use super::{AuthError, TokenSet};

#[derive(Debug, Clone)]
pub struct OidcConfig {
    /// Issuer URL, e.g. `https://login.chillincool.net/realms/chillincool`.
    pub issuer: String,
    pub client_id: String,
    /// Scopes in addition to `openid`.
    pub scopes: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct ProviderMetadata {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
}

/// State kept between [`OidcClient::begin`] and [`OidcClient::complete`].
pub struct PendingAuthorization {
    /// Open this in the user's browser.
    pub auth_url: Url,
    redirect_uri: String,
    state: CsrfToken,
    verifier: PkceCodeVerifier,
}

#[derive(Clone)]
pub struct OidcClient {
    config: OidcConfig,
    metadata: ProviderMetadata,
    http: oauth2::reqwest::Client,
}

impl OidcClient {
    /// Fetches `/.well-known/openid-configuration` for the issuer.
    pub async fn discover(config: OidcConfig) -> Result<Self, AuthError> {
        let http = oauth2::reqwest::ClientBuilder::new()
            // Following redirects from token endpoints enables SSRF; see oauth2 docs.
            .redirect(oauth2::reqwest::redirect::Policy::none())
            .user_agent(concat!("Backroom/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| AuthError::Discovery(e.to_string()))?;

        let issuer = config.issuer.trim_end_matches('/');
        let url = format!("{issuer}/.well-known/openid-configuration");
        let response = http
            .get(&url)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| AuthError::Discovery(format!("{url}: {e}")))?;
        let body = response
            .bytes()
            .await
            .map_err(|e| AuthError::Discovery(e.to_string()))?;
        let metadata: ProviderMetadata = serde_json::from_slice(&body)
            .map_err(|e| AuthError::Discovery(format!("bad metadata: {e}")))?;
        if metadata.issuer.trim_end_matches('/') != issuer {
            return Err(AuthError::Discovery(format!(
                "issuer mismatch: configured {issuer}, server says {}",
                metadata.issuer
            )));
        }

        Ok(OidcClient {
            config,
            metadata,
            http,
        })
    }

    pub fn config(&self) -> &OidcConfig {
        &self.config
    }

    fn client(
        &self,
    ) -> Result<
        BasicClient<
            oauth2::EndpointSet,
            oauth2::EndpointNotSet,
            oauth2::EndpointNotSet,
            oauth2::EndpointNotSet,
            oauth2::EndpointSet,
        >,
        AuthError,
    > {
        let auth = AuthUrl::new(self.metadata.authorization_endpoint.clone())
            .map_err(|e| AuthError::InvalidUrl(e.to_string()))?;
        let token = TokenUrl::new(self.metadata.token_endpoint.clone())
            .map_err(|e| AuthError::InvalidUrl(e.to_string()))?;
        Ok(
            BasicClient::new(ClientId::new(self.config.client_id.clone()))
                .set_auth_uri(auth)
                .set_token_uri(token),
        )
    }

    /// Starts a login. The front end opens `auth_url` and later passes the
    /// full redirect URL to [`complete`](Self::complete).
    pub fn begin(&self, redirect_uri: &str) -> Result<PendingAuthorization, AuthError> {
        let redirect = RedirectUrl::new(redirect_uri.to_owned())
            .map_err(|e| AuthError::InvalidUrl(e.to_string()))?;
        let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
        let scopes = std::iter::once("openid".to_owned())
            .chain(
                self.config
                    .scopes
                    .iter()
                    .filter(|s| *s != "openid")
                    .cloned(),
            )
            .map(Scope::new);
        let (auth_url, state) = self
            .client()?
            .set_redirect_uri(redirect)
            .authorize_url(CsrfToken::new_random)
            .add_scopes(scopes)
            .set_pkce_challenge(challenge)
            .url();
        Ok(PendingAuthorization {
            auth_url,
            redirect_uri: redirect_uri.to_owned(),
            state,
            verifier,
        })
    }

    /// Validates the redirect and exchanges the code for tokens.
    pub async fn complete(
        &self,
        pending: PendingAuthorization,
        callback_url: &str,
    ) -> Result<TokenSet, AuthError> {
        let callback =
            Url::parse(callback_url).map_err(|e| AuthError::InvalidUrl(e.to_string()))?;
        let mut code = None;
        let mut state = None;
        let mut error = None;
        let mut description = None;
        for (k, v) in callback.query_pairs() {
            match k.as_ref() {
                "code" => code = Some(v.into_owned()),
                "state" => state = Some(v.into_owned()),
                "error" => error = Some(v.into_owned()),
                "error_description" => description = Some(v.into_owned()),
                _ => {}
            }
        }
        if let Some(error) = error {
            return Err(AuthError::AuthorizationDenied { error, description });
        }
        if state.as_deref() != Some(pending.state.secret().as_str()) {
            return Err(AuthError::StateMismatch);
        }
        let code = code.ok_or(AuthError::MissingCode)?;

        let redirect = RedirectUrl::new(pending.redirect_uri)
            .map_err(|e| AuthError::InvalidUrl(e.to_string()))?;
        let response = self
            .client()?
            .set_redirect_uri(redirect)
            .exchange_code(AuthorizationCode::new(code))
            .set_pkce_verifier(pending.verifier)
            .request_async(&self.http)
            .await
            .map_err(token_error)?;
        Ok(to_token_set(&response))
    }

    pub async fn refresh(&self, refresh_token: &SecretString) -> Result<TokenSet, AuthError> {
        let token = RefreshToken::new(refresh_token.expose_secret().to_owned());
        let response = self
            .client()?
            .exchange_refresh_token(&token)
            .request_async(&self.http)
            .await
            .map_err(|e| match token_error(e) {
                AuthError::TokenRequest(msg) if msg.starts_with("invalid_grant") => {
                    AuthError::NotLoggedIn(format!("refresh token rejected ({msg})"))
                }
                other => other,
            })?;
        Ok(to_token_set(&response))
    }
}

fn to_token_set(response: &oauth2::basic::BasicTokenResponse) -> TokenSet {
    TokenSet {
        access_token: SecretString::from(response.access_token().secret().to_owned()),
        refresh_token: response
            .refresh_token()
            .map(|t| SecretString::from(t.secret().to_owned())),
        expires_at: response
            .expires_in()
            .and_then(|d| TimeDelta::from_std(d).ok())
            .map(|d| Utc::now() + d),
        scope: response.scopes().map(|scopes| {
            scopes
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        }),
    }
}

/// Converts token endpoint errors without ever including the response body,
/// which may contain tokens.
fn token_error<E: std::error::Error>(
    err: RequestTokenError<E, oauth2::basic::BasicErrorResponse>,
) -> AuthError {
    let msg = match err {
        RequestTokenError::ServerResponse(resp) => {
            let mut msg = resp.error().as_ref().to_owned();
            if let Some(desc) = resp.error_description() {
                msg.push_str(": ");
                msg.push_str(desc);
            }
            msg
        }
        RequestTokenError::Request(e) => format!("request error: {e}"),
        RequestTokenError::Parse(e, _body) => format!("unparseable response: {e}"),
        RequestTokenError::Other(e) => e,
    };
    AuthError::TokenRequest(msg)
}
