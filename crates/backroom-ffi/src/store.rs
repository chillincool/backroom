use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use backroom_core::auth::{StoreError, TokenSet, TokenStore};
use backroom_core::local::SecretVault;
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

/// Standalone mode's passwords (network logins, channel keys) as one JSON
/// object in a single foreign [`SecretStore`] item: one Keychain read at
/// most, so at most one prompt, and nothing to clean up item by item.
pub(crate) struct ForeignVault {
    store: Arc<dyn SecretStore>,
    /// `None` until the first read.
    cache: Mutex<Option<BTreeMap<String, SecretString>>>,
}

impl ForeignVault {
    pub(crate) fn new(store: Arc<dyn SecretStore>) -> Self {
        ForeignVault {
            store,
            cache: Mutex::new(None),
        }
    }

    /// Runs `f` on the loaded map; saves it if `f` says it changed.
    fn with<T>(
        &self,
        f: impl FnOnce(&mut BTreeMap<String, SecretString>) -> (T, bool),
    ) -> Result<T, StoreError> {
        let mut cache = self.cache.lock().unwrap_or_else(PoisonError::into_inner);
        if cache.is_none() {
            let map = match self.store.load().map_err(store_error)? {
                Some(json) => {
                    let json = SecretString::from(json);
                    serde_json::from_str::<BTreeMap<String, String>>(json.expose_secret())
                        .map_err(|_| StoreError("the saved passwords are unreadable".into()))?
                        .into_iter()
                        .map(|(k, v)| (k, SecretString::from(v)))
                        .collect()
                }
                None => BTreeMap::new(),
            };
            *cache = Some(map);
        }
        let mut map = cache.clone().unwrap_or_default();
        let (out, changed) = f(&mut map);
        if changed {
            let plain: BTreeMap<&str, &str> = map
                .iter()
                .map(|(k, v)| (k.as_str(), v.expose_secret()))
                .collect();
            let json = SecretString::from(
                serde_json::to_string(&plain).map_err(|e| StoreError(e.to_string()))?,
            );
            if map.is_empty() {
                self.store.clear()
            } else {
                self.store.save(json.expose_secret().to_owned())
            }
            .map_err(store_error)?;
            *cache = Some(map);
        }
        Ok(out)
    }
}

impl SecretVault for ForeignVault {
    fn get(&self, key: &str) -> Result<Option<SecretString>, StoreError> {
        self.with(|map| (map.get(key).cloned(), false))
    }

    fn set(&self, key: &str, value: &SecretString) -> Result<(), StoreError> {
        self.with(|map| {
            let changed = map
                .get(key)
                .is_none_or(|old| old.expose_secret() != value.expose_secret());
            map.insert(key.to_owned(), value.clone());
            ((), changed)
        })
    }

    fn delete(&self, key: &str) -> Result<(), StoreError> {
        self.with(|map| ((), map.remove(key).is_some()))
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
    fn vault_keeps_every_secret_in_one_item() {
        let counting = Arc::new(CountingStore::default());
        let vault = ForeignVault::new(counting.clone());
        assert!(vault.get("network/1/pass").unwrap().is_none());
        vault.set("network/1/pass", &"one".into()).unwrap();
        vault.set("network/2/pass", &"two".into()).unwrap();
        let saved = counting.inner.0.lock().unwrap().clone().unwrap();
        assert_eq!(saved, r#"{"network/1/pass":"one","network/2/pass":"two"}"#);

        // A new launch reads the item once.
        let vault = ForeignVault::new(counting.clone());
        for _ in 0..3 {
            let two = vault.get("network/2/pass").unwrap().unwrap();
            assert_eq!(two.expose_secret(), "two");
        }
        assert_eq!(counting.loads.load(std::sync::atomic::Ordering::SeqCst), 2);

        vault.delete("network/1/pass").unwrap();
        vault.delete("network/2/pass").unwrap();
        vault.delete("network/3/pass").unwrap();
        assert!(counting.inner.0.lock().unwrap().is_none());
    }

    #[test]
    fn vault_errors_pass_through() {
        let vault = ForeignVault::new(Arc::new(FailingStore));
        assert_eq!(vault.get("k").unwrap_err().0, "keychain locked");
        let corrupt = ForeignVault::new(Arc::new(MemoryStore(Mutex::new(Some("[".into())))));
        assert!(corrupt.get("k").is_err());
    }

    #[test]
    fn corrupt_value_is_a_store_error() {
        let memory = Arc::new(MemoryStore(Mutex::new(Some("not json".into()))));
        let store = ForeignTokenStore::new(memory);
        assert!(store.load().is_err());
    }
}
