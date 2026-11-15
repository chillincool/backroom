//! Standalone mode's local state: networks, joined channels, history and
//! read markers in SQLite ([`LocalStore`]), passwords behind
//! [`SecretVault`] (the Keychain on Apple platforms), and optional
//! plain-text logs ([`TextLog`]).
//!
//! With soju, all of this lives on the bouncer; without one, the
//! coordinator keeps it here and answers the same commands from it.

pub(crate) mod network;
mod store;
mod textlog;

use std::path::PathBuf;
use std::sync::Arc;

pub use network::{NetworkConfig, SECRET_ATTRIBUTES, SaslLogin};
pub use store::{LocalError, LocalStore};
pub use textlog::TextLog;

use secrecy::SecretString;

use crate::auth::StoreError;
use crate::transport::TlsOptions;

/// Secure storage for passwords, by key. The front end implements it with
/// the platform's secret store. Never log the values.
pub trait SecretVault: Send + Sync {
    fn get(&self, key: &str) -> Result<Option<SecretString>, StoreError>;
    fn set(&self, key: &str, value: &SecretString) -> Result<(), StoreError>;
    /// Succeeds if there's nothing to delete.
    fn delete(&self, key: &str) -> Result<(), StoreError>;
}

/// Everything a standalone session needs besides the nick.
pub struct Standalone {
    pub store: Arc<LocalStore>,
    pub secrets: Arc<dyn SecretVault>,
    /// Also write plain-text logs under this folder.
    pub text_logs: Option<PathBuf>,
    /// Extra CAs for servers with private certificates.
    pub tls: TlsOptions,
}

/// Vault key for a network's secret attribute (`pass`, `sasl-password`, …).
pub(crate) fn secret_key(network: &str, attribute: &str) -> String {
    format!("network/{network}/{attribute}")
}

/// Vault key for a channel key (`+k`).
pub(crate) fn channel_key_key(network: &str, channel: &str) -> String {
    format!(
        "network/{network}/channel-key/{}",
        crate::model::buffer_key(channel)
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use secrecy::ExposeSecret;

    use super::*;

    /// In-memory vault for tests.
    #[derive(Default)]
    pub(crate) struct MemoryVault(pub Mutex<HashMap<String, String>>);

    impl SecretVault for MemoryVault {
        fn get(&self, key: &str) -> Result<Option<SecretString>, StoreError> {
            Ok(self.0.lock().unwrap().get(key).cloned().map(Into::into))
        }
        fn set(&self, key: &str, value: &SecretString) -> Result<(), StoreError> {
            self.0
                .lock()
                .unwrap()
                .insert(key.to_owned(), value.expose_secret().to_owned());
            Ok(())
        }
        fn delete(&self, key: &str) -> Result<(), StoreError> {
            self.0.lock().unwrap().remove(key);
            Ok(())
        }
    }
}
