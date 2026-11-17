//! Logins in the desktop's secret store: the Secret Service (GNOME
//! Keyring, KWallet, KeePassXC) on the host, the Secret portal in a
//! Flatpak. One item per login, as on macOS.
//!
//! The core calls the store synchronously, sometimes from its own runtime's
//! threads, so all keyring work runs on one thread of our own with its own
//! small runtime; callers block until it answers. Without any secret
//! service the values are kept in memory only (and the UI says so).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};

use backroom_ffi::{BackroomError, SecretStore};
use secrecy::{ExposeSecret, SecretString};

const APP: &str = "net.chillincool.backroom";

static UNAVAILABLE: AtomicBool = AtomicBool::new(false);

/// True once the keyring turned out to be missing: logins are only
/// remembered until the app quits.
pub fn keyring_unavailable() -> bool {
    UNAVAILABLE.load(Ordering::Relaxed)
}

enum Request {
    Load(String, mpsc::Sender<Result<Option<SecretString>, String>>),
    Save(String, SecretString, mpsc::Sender<Result<(), String>>),
    Clear(String, mpsc::Sender<Result<(), String>>),
}

/// The keyring thread.
struct Worker {
    tx: Mutex<mpsc::Sender<Request>>,
}

fn worker() -> &'static Worker {
    static WORKER: OnceLock<Worker> = OnceLock::new();
    WORKER.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Request>();
        std::thread::Builder::new()
            .name("backroom-keyring".into())
            .spawn(move || run(rx))
            .expect("couldn't start the keyring thread");
        Worker { tx: Mutex::new(tx) }
    })
}

fn run(rx: mpsc::Receiver<Request>) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("keyring runtime: {e}");
            return;
        }
    };
    // `None`: not tried yet; `Some(None)`: no keyring, memory only.
    let mut keyring: Option<Option<oo7::Keyring>> = None;
    let mut memory: HashMap<String, SecretString> = HashMap::new();
    while let Ok(request) = rx.recv() {
        let ring = keyring.get_or_insert_with(|| match runtime.block_on(oo7::Keyring::new()) {
            Ok(k) => Some(k),
            Err(e) => {
                tracing::warn!("no secret service ({e}); logins are kept in memory only");
                UNAVAILABLE.store(true, Ordering::Relaxed);
                None
            }
        });
        match request {
            Request::Load(account, reply) => {
                let result = match ring {
                    Some(k) => runtime.block_on(load(k, &account)),
                    None => Ok(memory.get(&account).cloned()),
                };
                let _ = reply.send(result);
            }
            Request::Save(account, value, reply) => {
                let result = match ring {
                    Some(k) => runtime.block_on(save(k, &account, &value)),
                    None => {
                        memory.insert(account, value);
                        Ok(())
                    }
                };
                let _ = reply.send(result);
            }
            Request::Clear(account, reply) => {
                let result = match ring {
                    Some(k) => runtime
                        .block_on(k.delete(&attributes(&account)))
                        .map_err(|e| e.to_string()),
                    None => {
                        memory.remove(&account);
                        Ok(())
                    }
                };
                let _ = reply.send(result);
            }
        }
    }
}

fn attributes(account: &str) -> HashMap<&str, &str> {
    HashMap::from([("application", APP), ("account", account)])
}

async fn load(keyring: &oo7::Keyring, account: &str) -> Result<Option<SecretString>, String> {
    let items = keyring
        .search_items(&attributes(account))
        .await
        .map_err(|e| e.to_string())?;
    let Some(item) = items.first() else {
        return Ok(None);
    };
    if item.is_locked().await.map_err(|e| e.to_string())? {
        // The desktop asks for the keyring password.
        item.unlock().await.map_err(|e| e.to_string())?;
    }
    let secret = item.secret().await.map_err(|e| e.to_string())?;
    let text = std::str::from_utf8(secret.as_bytes())
        .map_err(|_| "the saved login isn't text".to_owned())?;
    Ok(Some(SecretString::from(text.to_owned())))
}

async fn save(keyring: &oo7::Keyring, account: &str, value: &SecretString) -> Result<(), String> {
    let label = if account == "standalone" {
        "Backroom: network passwords".to_owned()
    } else {
        format!("Backroom: {account}")
    };
    keyring
        .create_item(
            &label,
            &attributes(account),
            oo7::Secret::text(value.expose_secret()),
            true,
        )
        .await
        .map_err(|e| e.to_string())
}

fn store_error(message: String) -> BackroomError {
    BackroomError::Store { message }
}

fn ask<T>(
    make: impl FnOnce(mpsc::Sender<Result<T, String>>) -> Request,
) -> Result<T, BackroomError> {
    let (reply, answer) = mpsc::channel();
    worker()
        .tx
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .send(make(reply))
        .map_err(|_| store_error("the keyring thread stopped".into()))?;
    answer
        .recv()
        .map_err(|_| store_error("the keyring thread stopped".into()))?
        .map_err(store_error)
}

/// One login's item. Never logs the value.
#[derive(Debug)]
pub struct KeyringStore {
    account: String,
}

impl KeyringStore {
    pub fn new(account: String) -> Arc<Self> {
        Arc::new(KeyringStore { account })
    }
}

impl SecretStore for KeyringStore {
    fn load(&self) -> Result<Option<String>, BackroomError> {
        let value = ask(|reply| Request::Load(self.account.clone(), reply))?;
        Ok(value.map(|v| v.expose_secret().to_owned()))
    }

    fn save(&self, value: String) -> Result<(), BackroomError> {
        ask(|reply| Request::Save(self.account.clone(), SecretString::from(value), reply))
    }

    fn clear(&self) -> Result<(), BackroomError> {
        ask(|reply| Request::Clear(self.account.clone(), reply))
    }
}
