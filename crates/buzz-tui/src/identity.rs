use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use thiserror::Error;

const KEYRING_SERVICE: &str = "buzz-tui";
const KEYRING_ACCOUNT: &str = "identity";

#[derive(Debug, Error)]
pub enum IdentityStoreError {
    #[error("failed to read identity: {0}")]
    Read(std::io::Error),
    #[error("failed to store identity: {0}")]
    Write(std::io::Error),
}

#[derive(Clone, Debug)]
pub struct IdentityStore {
    path: PathBuf,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IdentityStorage {
    Keyring,
    File,
}

impl IdentityStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> Result<Option<String>, IdentityStoreError> {
        if let Some(secret) = load_keyring_secret() {
            return Ok(Some(secret));
        }
        if !self.path.exists() {
            return Ok(None);
        }
        fs::read_to_string(&self.path)
            .map(|value| Some(value.trim().to_string()))
            .map_err(IdentityStoreError::Read)
    }

    pub fn store(&self, secret: &str) -> Result<IdentityStorage, IdentityStoreError> {
        if store_keyring_secret(secret) {
            let _ = fs::remove_file(&self.path);
            return Ok(IdentityStorage::Keyring);
        }
        self.store_file(secret)?;
        Ok(IdentityStorage::File)
    }

    fn store_file(&self, secret: &str) -> Result<(), IdentityStoreError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(IdentityStoreError::Write)?;
        }
        let mut options = OpenOptions::new();
        options.create(true).truncate(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&self.path)
            .map_err(IdentityStoreError::Write)?;
        file.write_all(secret.as_bytes())
            .map_err(IdentityStoreError::Write)?;
        file.write_all(b"\n").map_err(IdentityStoreError::Write)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.path, fs::Permissions::from_mode(0o600))
                .map_err(IdentityStoreError::Write)?;
        }
        Ok(())
    }
}

pub fn identity_file_path(override_path: Option<&str>) -> PathBuf {
    if let Some(path) = override_path.filter(|path| !path.trim().is_empty()) {
        return PathBuf::from(path);
    }
    if let Ok(path) = std::env::var("BUZZ_TUI_IDENTITY_FILE") {
        if !path.trim().is_empty() {
            return PathBuf::from(path);
        }
    }
    if let Ok(config_home) = std::env::var("XDG_CONFIG_HOME") {
        if !config_home.trim().is_empty() {
            return PathBuf::from(config_home)
                .join("buzz")
                .join("tui-identity.key");
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        if !home.trim().is_empty() {
            return PathBuf::from(home)
                .join(".config")
                .join("buzz")
                .join("tui-identity.key");
        }
    }
    PathBuf::from("buzz-tui-identity.key")
}

#[cfg(feature = "system-keyring")]
fn keyring_entry() -> Option<keyring::Entry> {
    keyring::Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT).ok()
}

#[cfg(feature = "system-keyring")]
fn load_keyring_secret() -> Option<String> {
    keyring_entry().and_then(|entry| entry.get_password().ok())
}

#[cfg(not(feature = "system-keyring"))]
fn load_keyring_secret() -> Option<String> {
    None
}

#[cfg(feature = "system-keyring")]
fn store_keyring_secret(secret: &str) -> bool {
    keyring_entry().is_some_and(|entry| entry.set_password(secret).is_ok())
}

#[cfg(not(feature = "system-keyring"))]
fn store_keyring_secret(_secret: &str) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_store_round_trips() {
        let dir = std::env::temp_dir().join(format!("buzz-tui-identity-{}", std::process::id()));
        let path = dir.join("identity.key");
        let store = IdentityStore::new(path.clone());
        store.store_file("nsec1test").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "nsec1test\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn explicit_identity_path_wins() {
        assert_eq!(
            identity_file_path(Some("/tmp/buzz-identity")),
            PathBuf::from("/tmp/buzz-identity")
        );
    }
}
