use std::sync::Arc;

use backroom_core::auth::{StoreError, TokenSet, TokenStore};
use secrecy::{ExposeSecret, SecretString};

use crate::BackroomError;

/// Secure storage for one opaque secret: the login tokens, serialized by
/// Rust. Swift implements it with a Keychain item. Never log the value.
#[uniffi::export(foreign)]
pub trait SecretStore: Send + Sync {
    /// The saved value, or nil if nothing is saved.
    fn load(&self) -> Result<Option<String>, BackroomError>;
    /// Replaces the saved value.
    fn save(&self, value: String) -> Result<(), BackroomError>;
    /// Deletes the saved value; succeeds if there is none.
    fn clear(&self) -> Result<(), BackroomError>;
}

/// Adapts a foreign [`SecretStore`] to the core's [`TokenStore`].
pub(crate) struct ForeignTokenStore(pub Arc<dyn SecretStore>);

fn store_error(e: BackroomError) -> StoreError {
    match e {
        BackroomError::Store { message } => StoreError(message),
        other => StoreError(other.to_string()),
    }
}

impl TokenStore for ForeignTokenStore {
    fn load(&self) -> Result<Option<TokenSet>, StoreError> {
        let Some(value) = self.0.load().map_err(store_error)? else {
            return Ok(None);
        };
        let value = SecretString::from(value);
        TokenSet::from_json(value.expose_secret()).map(Some)
    }

    fn save(&self, tokens: &TokenSet) -> Result<(), StoreError> {
        self.0
            .save(tokens.to_json().expose_secret().to_owned())
            .map_err(store_error)
    }

    fn clear(&self) -> Result<(), StoreError> {
        self.0.clear().map_err(store_error)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::Mutex;

    use super::*;

    /// In-memory stand-in for the Keychain.
    #[derive(Default)]
    pub(crate) struct MemoryStore(pub Mutex<Option<String>>);

    impl SecretStore for MemoryStore {
        fn load(&self) -> Result<Option<String>, BackroomError> {
            Ok(self.0.lock().unwrap().clone())
        }
        fn save(&self, value: String) -> Result<(), BackroomError> {
            *self.0.lock().unwrap() = Some(value);
            Ok(())
        }
        fn clear(&self) -> Result<(), BackroomError> {
            *self.0.lock().unwrap() = None;
            Ok(())
        }
    }

    struct FailingStore;

    impl SecretStore for FailingStore {
        fn load(&self) -> Result<Option<String>, BackroomError> {
            Err(BackroomError::Store {
                message: "keychain locked".into(),
            })
        }
        fn save(&self, _: String) -> Result<(), BackroomError> {
            self.load().map(|_| ())
        }
        fn clear(&self) -> Result<(), BackroomError> {
            self.load().map(|_| ())
        }
    }

    fn tokens() -> TokenSet {
        TokenSet {
            access_token: SecretString::from("access-secret"),
            refresh_token: Some(SecretString::from("refresh-secret")),
            expires_at: None,
            scope: None,
        }
    }

    #[test]
    fn round_trip_through_foreign_store() {
        let store = ForeignTokenStore(Arc::new(MemoryStore::default()));
        assert!(store.load().unwrap().is_none());
        store.save(&tokens()).unwrap();
        let loaded = store.load().unwrap().unwrap();
        assert_eq!(loaded.access_token.expose_secret(), "access-secret");
        store.clear().unwrap();
        assert!(store.load().unwrap().is_none());
    }

    #[test]
    fn foreign_errors_pass_through() {
        let store = ForeignTokenStore(Arc::new(FailingStore));
        assert_eq!(store.load().unwrap_err().0, "keychain locked");
        assert_eq!(store.save(&tokens()).unwrap_err().0, "keychain locked");
    }

    #[test]
    fn corrupt_value_is_a_store_error() {
        let memory = Arc::new(MemoryStore(Mutex::new(Some("not json".into()))));
        let store = ForeignTokenStore(memory);
        assert!(store.load().is_err());
    }
}
