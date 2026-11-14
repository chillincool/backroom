//! DEV/TESTING ONLY: stores OIDC tokens in a 0600 JSON file.
//!
//! The Mac app will use the Keychain through the same `TokenStore` trait.

use std::fs;
use std::io::Write as _;
use std::path::PathBuf;

use backroom_core::auth::{StoreError, TokenSet, TokenStore};
use secrecy::ExposeSecret;
use zeroize::Zeroize;

pub struct TokenFile {
    path: PathBuf,
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
        let parsed = std::str::from_utf8(&bytes)
            .map_err(|_| err("stored tokens are not UTF-8"))
            .and_then(TokenSet::from_json);
        bytes.zeroize();
        parsed
            .map(Some)
            .map_err(|e| err(format!("{}: {}", self.path.display(), e.0)))
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

        let json = tokens.to_json();

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
            .and_then(|mut f| {
                f.write_all(json.expose_secret().as_bytes())
                    .and_then(|()| f.sync_all())
            })
            .and_then(|()| fs::rename(&tmp, &self.path));
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
