//! DEV/TESTING ONLY: stores OIDC tokens in a 0600 JSON file.
//!
//! The Mac app will use the Keychain through the same `TokenStore` trait.

use std::fs;
use std::io::Write as _;
use std::path::PathBuf;

use backroom_core::auth::{StoreError, TokenSet, TokenStore};
use chrono::DateTime;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

pub struct TokenFile {
    path: PathBuf,
}

#[derive(Serialize, Deserialize)]
struct OnDisk {
    access_token: String,
    refresh_token: Option<String>,
    /// Unix seconds.
    expires_at: Option<i64>,
    scope: Option<String>,
}

impl Drop for OnDisk {
    fn drop(&mut self) {
        self.access_token.zeroize();
        if let Some(t) = &mut self.refresh_token {
            t.zeroize();
        }
    }
}

impl TokenFile {
    pub fn default_location() -> anyhow::Result<Self> {
        let dir = dirs::data_dir().ok_or_else(|| anyhow::anyhow!("no data directory"))?;
        Ok(TokenFile {
            path: dir.join("backroom").join("dev-tokens.json"),
        })
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
}

fn err(e: impl std::fmt::Display) -> StoreError {
    StoreError(e.to_string())
}

impl TokenStore for TokenFile {
    fn load(&self) -> Result<Option<TokenSet>, StoreError> {
        let mut bytes = match fs::read(&self.path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(err(format!("{}: {e}", self.path.display()))),
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = fs::metadata(&self.path)
                && meta.permissions().mode() & 0o077 != 0
            {
                tracing::warn!("{} is readable by other users", self.path.display());
            }
        }
        let parsed: Result<OnDisk, _> = serde_json::from_slice(&bytes);
        bytes.zeroize();
        let disk = parsed.map_err(|e| err(format!("{}: {e}", self.path.display())))?;
        Ok(Some(TokenSet {
            access_token: SecretString::from(disk.access_token.clone()),
            refresh_token: disk.refresh_token.clone().map(SecretString::from),
            expires_at: disk.expires_at.and_then(|t| DateTime::from_timestamp(t, 0)),
            scope: disk.scope.clone(),
        }))
    }

    fn save(&self, tokens: &TokenSet) -> Result<(), StoreError> {
        let dir = self.path.parent().ok_or_else(|| err("bad token path"))?;
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(dir).map_err(err)?;

        let disk = OnDisk {
            access_token: tokens.access_token.expose_secret().to_owned(),
            refresh_token: tokens
                .refresh_token
                .as_ref()
                .map(|t| t.expose_secret().to_owned()),
            expires_at: tokens.expires_at.map(|t| t.timestamp()),
            scope: tokens.scope.clone(),
        };
        let mut json = serde_json::to_vec_pretty(&disk).map_err(err)?;

        let tmp = self.path.with_extension("json.tmp");
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let result = opts
            .open(&tmp)
            .and_then(|mut f| f.write_all(&json).and_then(|()| f.sync_all()))
            .and_then(|()| fs::rename(&tmp, &self.path));
        json.zeroize();
        result.map_err(|e| err(format!("{}: {e}", self.path.display())))
    }

    fn clear(&self) -> Result<(), StoreError> {
        match fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(err(e)),
        }
    }
}
