//! Token custody. The pairing token is a bearer credential; it goes
//! through a `SecretStore` so the UI can keep it in the OS keychain while
//! tests and headless runs keep it in a 0600 file.
//!
//! HARD RULE for the P0/P1 build on the owner's machine: tests and the
//! CLI use ONLY the file or mock store. The keyring implementation is
//! compiled but never exercised in this session, because an unsigned
//! rebuilt binary triggers a keychain password prompt on the owner's
//! screen.

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::Mutex;

/// Store one named secret, read it back, delete it. All three must be
/// idempotent.
pub trait SecretStore: Send + Sync {
    fn set(&self, key: &str, value: &str) -> io::Result<()>;
    fn get(&self, key: &str) -> io::Result<Option<String>>;
    fn delete(&self, key: &str) -> io::Result<()>;
}

/// In-memory store for tests.
#[derive(Default)]
pub struct MemSecretStore {
    map: Mutex<HashMap<String, String>>,
}

impl MemSecretStore {
    pub fn new() -> MemSecretStore {
        MemSecretStore::default()
    }
}

impl SecretStore for MemSecretStore {
    fn set(&self, key: &str, value: &str) -> io::Result<()> {
        self.map.lock().unwrap().insert(key.into(), value.into());
        Ok(())
    }
    fn get(&self, key: &str) -> io::Result<Option<String>> {
        Ok(self.map.lock().unwrap().get(key).cloned())
    }
    fn delete(&self, key: &str) -> io::Result<()> {
        self.map.lock().unwrap().remove(key);
        Ok(())
    }
}

/// A JSON map at a 0600 path. The headless default.
pub struct FileSecretStore {
    path: PathBuf,
    lock: Mutex<()>,
}

impl FileSecretStore {
    pub fn new(path: PathBuf) -> FileSecretStore {
        FileSecretStore {
            path,
            lock: Mutex::new(()),
        }
    }

    fn read_map(&self) -> io::Result<HashMap<String, String>> {
        match std::fs::read(&self.path) {
            Ok(b) => serde_json::from_slice(&b)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(HashMap::new()),
            Err(e) => Err(e),
        }
    }

    fn write_map(&self, map: &HashMap<String, String>) -> io::Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = self.path.with_extension("tmp");
        let bytes = serde_json::to_vec(map).map_err(io::Error::other)?;
        {
            use std::io::Write;
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(&bytes)?;
            f.sync_all()?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
        }
        std::fs::rename(&tmp, &self.path)
    }
}

impl SecretStore for FileSecretStore {
    fn set(&self, key: &str, value: &str) -> io::Result<()> {
        let _g = self.lock.lock().unwrap();
        let mut map = self.read_map()?;
        map.insert(key.into(), value.into());
        self.write_map(&map)
    }
    fn get(&self, key: &str) -> io::Result<Option<String>> {
        let _g = self.lock.lock().unwrap();
        Ok(self.read_map()?.get(key).cloned())
    }
    fn delete(&self, key: &str) -> io::Result<()> {
        let _g = self.lock.lock().unwrap();
        let mut map = self.read_map()?;
        map.remove(key);
        self.write_map(&map)
    }
}

/// The OS keychain, through the `keyring` crate (macOS Keychain, Windows
/// Credential Manager, Linux Secret Service). The UI uses this; the tests
/// in this repo never do.
pub struct KeyringSecretStore {
    service: String,
}

impl KeyringSecretStore {
    pub fn new(service: impl Into<String>) -> KeyringSecretStore {
        KeyringSecretStore {
            service: service.into(),
        }
    }
}

impl SecretStore for KeyringSecretStore {
    fn set(&self, key: &str, value: &str) -> io::Result<()> {
        let e = keyring::Entry::new(&self.service, key).map_err(io::Error::other)?;
        e.set_password(value).map_err(io::Error::other)
    }
    fn get(&self, key: &str) -> io::Result<Option<String>> {
        let e = keyring::Entry::new(&self.service, key).map_err(io::Error::other)?;
        match e.get_password() {
            Ok(v) => Ok(Some(v)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(io::Error::other(e)),
        }
    }
    fn delete(&self, key: &str) -> io::Result<()> {
        let e = keyring::Entry::new(&self.service, key).map_err(io::Error::other)?;
        match e.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(io::Error::other(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exercise(store: &dyn SecretStore) {
        assert_eq!(store.get("tok").unwrap(), None);
        store.set("tok", "secret-1").unwrap();
        assert_eq!(store.get("tok").unwrap().as_deref(), Some("secret-1"));
        store.set("tok", "secret-2").unwrap();
        assert_eq!(store.get("tok").unwrap().as_deref(), Some("secret-2"));
        store.delete("tok").unwrap();
        assert_eq!(store.get("tok").unwrap(), None);
        // idempotent delete
        store.delete("tok").unwrap();
    }

    #[test]
    fn mem_store() {
        exercise(&MemSecretStore::new());
    }

    #[test]
    fn file_store_0600() {
        let dir = std::env::temp_dir().join(format!("connect-secret-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = FileSecretStore::new(dir.join("secrets.json"));
        exercise(&store);
        store.set("tok", "x").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join("secrets.json"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
