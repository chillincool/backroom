use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

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

/// Adapts a foreign [`SecretStore`] to the core's [`TokenStore`], keeping
/// the tokens in memory after the first read: each Keychain read can prompt
/// the user, and the core loads tokens for every connection.
pub(crate) struct ForeignTokenStore {
    store: Arc<dyn SecretStore>,
    /// `None`: not read yet. `Some(None)`: read, nothing stored.
    cache: Mutex<Option<Option<TokenSet>>>,
}

impl ForeignTokenStore {
    pub(crate) fn new(store: Arc<dyn SecretStore>) -> Self {
        ForeignTokenStore {
            store,
            cache: Mutex::new(None),
        }
    }

    fn cache(&self) -> MutexGuard<'_, Option<Option<TokenSet>>> {
        self.cache.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn store_error(e: BackroomError) -> StoreError {
    match e {
        BackroomError::Store { message } => StoreError(message),
        other => StoreError(other.to_string()),
    }
}

impl TokenStore for ForeignTokenStore {
    fn load(&self) -> Result<Option<TokenSet>, StoreError> {
        let mut cache = self.cache();
        if let Some(cached) = cache.as_ref() {
            return Ok(cached.clone());
        }
        let tokens = match self.store.load().map_err(store_error)? {
            Some(value) => {
                let value = SecretString::from(value);
                Some(TokenSet::from_json(value.expose_secret())?)
            }
            None => None,
        };
        *cache = Some(tokens.clone());
        Ok(tokens)
    }

    fn save(&self, tokens: &TokenSet) -> Result<(), StoreError> {
        let mut cache = self.cache();
        self.store
            .save(tokens.to_json().expose_secret().to_owned())
            .map_err(store_error)?;
        *cache = Some(Some(tokens.clone()));
        Ok(())
    }

    fn clear(&self) -> Result<(), StoreError> {
        let mut cache = self.cache();
        self.store.clear().map_err(store_error)?;
        *cache = Some(None);
        Ok(())
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
        let store = ForeignTokenStore::new(Arc::new(MemoryStore::default()));
        assert!(store.load().unwrap().is_none());
        store.save(&tokens()).unwrap();
        let loaded = store.load().unwrap().unwrap();
        assert_eq!(loaded.access_token.expose_secret(), "access-secret");
        store.clear().unwrap();
        assert!(store.load().unwrap().is_none());
    }

    /// Counts reads, like Keychain prompts.
    #[derive(Default)]
    struct CountingStore {
        inner: MemoryStore,
        loads: std::sync::atomic::AtomicUsize,
    }

    impl SecretStore for CountingStore {
        fn load(&self) -> Result<Option<String>, BackroomError> {
            self.loads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.inner.load()
        }
        fn save(&self, value: String) -> Result<(), BackroomError> {
            self.inner.save(value)
        }
        fn clear(&self) -> Result<(), BackroomError> {
            self.inner.clear()
        }
    }

    #[test]
    fn reads_the_foreign_store_once() {
        let counting = Arc::new(CountingStore::default());
        counting
            .inner
            .save(tokens().to_json().expose_secret().to_owned())
            .unwrap();
        let store = ForeignTokenStore::new(counting.clone());
        for _ in 0..5 {
            assert!(store.load().unwrap().is_some());
        }
        store.save(&tokens()).unwrap();
        assert!(store.load().unwrap().is_some());
        store.clear().unwrap();
        assert!(store.load().unwrap().is_none());
        assert_eq!(counting.loads.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn foreign_errors_pass_through() {
        let store = ForeignTokenStore::new(Arc::new(FailingStore));
        assert_eq!(store.load().unwrap_err().0, "keychain locked");
        assert_eq!(store.save(&tokens()).unwrap_err().0, "keychain locked");
    }

    #[test]
    fn corrupt_value_is_a_store_error() {
        let memory = Arc::new(MemoryStore(Mutex::new(Some("not json".into()))));
        let store = ForeignTokenStore::new(memory);
        assert!(store.load().is_err());
    }
}
