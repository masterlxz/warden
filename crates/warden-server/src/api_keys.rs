//! The Warden API's keys (P12): created inside the app (web, desktop, `warden-server api-keys`) and
//! presented as `Authorization: Bearer <key>` to the hub's OpenAI-compatible routes
//! (`openai_api.rs`). Only a SHA-256 of each key is kept, so a leaked `api_keys.json` hands out
//! nothing that works; the key itself is shown once, when it's created.
//!
//! No cache: every call rereads the file, for the same reason as `PairingStore` — the running hub
//! and a `warden-server api-keys revoke` are different processes, and a revocation has to hold on
//! the very next request.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// What every key starts with, so one is easy to recognize (and to find in a leaked log).
pub const KEY_PREFIX: &str = "wdn_";

/// How much of a key the list shows, prefix included — enough to tell keys apart, far too little
/// to guess the rest.
const SHOWN_CHARS: usize = 12;

/// `last_used_at_ms` is only rewritten when older than this, so a busy integration doesn't rewrite
/// the file on every request.
const LAST_USED_GRANULARITY_MS: i64 = 60_000;

/// The longest name a key can have — a label in a list.
pub const MAX_NAME_CHARS: usize = 60;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiKey {
    pub id: String,
    pub name: String,
    /// The first characters of the key, `wdn_` included, for recognizing it in the list.
    pub shown: String,
    pub created_at_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used_at_ms: Option<i64>,
    hash: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct KeysFile {
    #[serde(default)]
    keys: Vec<ApiKey>,
}

/// A key just created: `key` is the only time it exists outside the client that asked for it.
#[derive(Debug, Clone)]
pub struct CreatedApiKey {
    pub key: String,
    pub info: ApiKey,
}

fn hash_key(key: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(key.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

fn now_millis() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as i64
}

/// Serializes this process's read-modify-writes of the file (a hub serving several API calls at
/// once, plus the web creating a key).
static WRITES: Mutex<()> = Mutex::new(());

pub struct ApiKeyStore {
    path: PathBuf,
}

impl ApiKeyStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn load(&self) -> anyhow::Result<KeysFile> {
        match std::fs::read_to_string(&self.path) {
            Ok(contents) => Ok(serde_json::from_str(&contents)?),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(KeysFile::default()),
            Err(err) => Err(err.into()),
        }
    }

    fn save(&self, file: &KeysFile) -> anyhow::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&self.path, serde_json::to_string_pretty(file)?)?;
        Ok(())
    }

    /// Every key, oldest first.
    pub fn list(&self) -> anyhow::Result<Vec<ApiKey>> {
        Ok(self.load()?.keys)
    }

    /// A new key named `name` (trimmed; not empty, at most `MAX_NAME_CHARS`, unique).
    pub fn create(&self, name: &str) -> anyhow::Result<CreatedApiKey> {
        let name = name.trim();
        anyhow::ensure!(!name.is_empty(), "the key needs a name");
        anyhow::ensure!(name.chars().count() <= MAX_NAME_CHARS, "the name is too long (max {MAX_NAME_CHARS} characters)");
        anyhow::ensure!(!name.chars().any(char::is_control), "the name can't have control characters");
        let _guard = WRITES.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut file = self.load()?;
        anyhow::ensure!(!file.keys.iter().any(|k| k.name == name), "there is already a key named '{name}'");
        let key = format!("{KEY_PREFIX}{}", warden_bootstrap::generate_auth_key());
        let info = ApiKey {
            // The hash's start is as unique as the key and says nothing about it.
            id: hash_key(&key)[..12].to_string(),
            name: name.to_string(),
            shown: key.chars().take(SHOWN_CHARS).collect(),
            created_at_ms: now_millis(),
            last_used_at_ms: None,
            hash: hash_key(&key),
        };
        file.keys.push(info.clone());
        self.save(&file)?;
        Ok(CreatedApiKey { key, info })
    }

    /// `false` when there's no key with that id.
    pub fn revoke(&self, id: &str) -> anyhow::Result<bool> {
        let _guard = WRITES.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut file = self.load()?;
        let before = file.keys.len();
        file.keys.retain(|k| k.id != id);
        if file.keys.len() == before {
            return Ok(false);
        }
        self.save(&file)?;
        Ok(true)
    }

    /// The key `presented` belongs to, if any, noting that it was used.
    pub fn authenticate(&self, presented: &str) -> anyhow::Result<Option<ApiKey>> {
        if !presented.starts_with(KEY_PREFIX) {
            return Ok(None);
        }
        let hash = hash_key(presented);
        let _guard = WRITES.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut file = self.load()?;
        let Some(key) = file.keys.iter_mut().find(|k| k.hash == hash) else {
            return Ok(None);
        };
        let now = now_millis();
        let stale = key.last_used_at_ms.is_none_or(|last| now - last >= LAST_USED_GRANULARITY_MS);
        if stale {
            key.last_used_at_ms = Some(now);
        }
        let found = key.clone();
        if stale {
            self.save(&file)?;
        }
        Ok(Some(found))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(name: &str) -> ApiKeyStore {
        let dir = std::env::temp_dir().join(format!("warden-api-keys-{name}-{}", now_millis()));
        ApiKeyStore::new(dir.join("api_keys.json"))
    }

    #[test]
    fn a_key_is_shown_once_and_only_its_hash_is_kept() {
        let store = store("create");
        let created = store.create("  n8n  ").unwrap();
        assert!(created.key.starts_with(KEY_PREFIX));
        assert_eq!(created.info.name, "n8n");
        assert!(created.key.starts_with(&created.info.shown));
        let on_disk = std::fs::read_to_string(store.path()).unwrap();
        assert!(!on_disk.contains(&created.key), "the key itself never reaches the disk");
        assert!(store.create("n8n").is_err(), "names are unique");
        assert!(store.create("   ").is_err());
        assert_eq!(store.list().unwrap(), vec![created.info]);
    }

    #[test]
    fn authenticate_takes_the_right_key_only_and_notes_the_use() {
        let store = store("auth");
        let created = store.create("script").unwrap();
        assert_eq!(store.authenticate("wdn_nope").unwrap(), None);
        assert_eq!(store.authenticate("no-prefix").unwrap(), None);
        let found = store.authenticate(&created.key).unwrap().expect("the right key");
        assert_eq!(found.id, created.info.id);
        assert!(found.last_used_at_ms.is_some());
        assert_eq!(store.list().unwrap()[0].last_used_at_ms, found.last_used_at_ms);

        assert!(store.revoke(&created.info.id).unwrap());
        assert!(!store.revoke(&created.info.id).unwrap());
        assert_eq!(store.authenticate(&created.key).unwrap(), None, "a revoked key stops working");
    }
}
