use backroom_core::auth::{AuthError, StoreError};

/// Every error the bindings return. Swift sees `BackroomError`, and throws
/// `BackroomError.Store` from its `SecretStore` implementation.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum BackroomError {
    /// No usable login: never logged in, logged out, or the refresh token
    /// was rejected. Start a new login.
    #[error("not logged in: {message}")]
    NotLoggedIn { message: String },
    /// The login failed or was cancelled (denied, state mismatch, token
    /// request failed, discovery failed).
    #[error("{message}")]
    Auth { message: String },
    /// Bad `ClientConfig`, e.g. an unsupported server URL.
    #[error("invalid configuration: {message}")]
    Config { message: String },
    /// The secret store failed.
    #[error("secret store: {message}")]
    Store { message: String },
    #[error("complete_login called without begin_login")]
    NoLoginInProgress,
    /// No session is running; call `connect` first.
    #[error("not connected")]
    NotConnected,
    #[error("internal error: {message}")]
    Internal { message: String },
}

impl From<AuthError> for BackroomError {
    fn from(e: AuthError) -> Self {
        match e {
            AuthError::NotLoggedIn(message) => BackroomError::NotLoggedIn { message },
            AuthError::Store(StoreError(message)) => BackroomError::Store { message },
            other => BackroomError::Auth {
                message: other.to_string(),
            },
        }
    }
}

impl From<StoreError> for BackroomError {
    fn from(StoreError(message): StoreError) -> Self {
        BackroomError::Store { message }
    }
}

/// Swift threw something that isn't a `BackroomError`.
impl From<uniffi::UnexpectedUniFFICallbackError> for BackroomError {
    fn from(e: uniffi::UnexpectedUniFFICallbackError) -> Self {
        BackroomError::Store { message: e.reason }
    }
}

impl From<tokio::task::JoinError> for BackroomError {
    fn from(e: tokio::task::JoinError) -> Self {
        BackroomError::Internal {
            message: e.to_string(),
        }
    }
}
