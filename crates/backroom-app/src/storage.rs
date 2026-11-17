//! Where the app keeps its files, and reading and writing them as JSON.

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;

/// The app's folders. On Linux: `$XDG_CONFIG_HOME/backroom` for settings,
/// `$XDG_DATA_HOME/backroom` for the standalone database and text logs,
/// `$XDG_STATE_HOME/backroom` for tabs and window state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub config: PathBuf,
    pub data: PathBuf,
    pub state: PathBuf,
}

impl Paths {
    /// All three under one folder (tests, portable setups).
    pub fn under(root: &Path) -> Self {
        Paths {
            config: root.join("config"),
            data: root.join("data"),
            state: root.join("state"),
        }
    }

    pub fn preferences(&self) -> PathBuf {
        self.config.join("preferences.json")
    }

    pub fn servers(&self) -> PathBuf {
        self.config.join("servers.json")
    }

    /// Tabs, pane layouts, whether "No bouncer" was connected.
    pub fn session_state(&self) -> PathBuf {
        self.state.join("session.json")
    }

    pub fn standalone_database(&self) -> PathBuf {
        self.data.join("standalone.sqlite")
    }

    pub fn default_text_logs(&self) -> PathBuf {
        self.data.join("logs")
    }
}

/// Reads a JSON file; `None` if it's missing or unreadable (logged).
pub fn read_json<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            tracing::warn!("couldn't read {}: {e}", path.display());
            return None;
        }
    };
    match serde_json::from_slice(&bytes) {
        Ok(value) => Some(value),
        Err(e) => {
            tracing::warn!("ignoring {}: {e}", path.display());
            None
        }
    }
}

/// Writes a JSON file atomically (a temporary file, then a rename), making
/// its folder if needed. Failures are logged.
pub fn write_json<T: Serialize>(path: &Path, value: &T) {
    let result = (|| -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_vec_pretty(value)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, path)
    })();
    if let Err(e) = result {
        tracing::warn!("couldn't save {}: {e}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_missing_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a/b.json");
        assert_eq!(read_json::<Vec<u32>>(&path), None);
        write_json(&path, &vec![1u32, 2]);
        assert_eq!(read_json::<Vec<u32>>(&path), Some(vec![1, 2]));
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(read_json::<Vec<u32>>(&path), None);
    }
}
