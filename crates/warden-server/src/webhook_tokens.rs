//! The tokens of the hub's incoming webhooks (P105): one per webhook, created with `warden-server webhooks token <id>`
//! and presented as `Authorization: Bearer <token>` (or `X-Warden-Token`) to `POST /hooks/<id>` (`webhooks.rs`). Like the
//! Warden API's keys (`api_keys.rs`), only a SHA-256 of each token is kept, so a leaked `webhook_tokens.json` hands out
//! nothing that works, and the token is shown once, when it's created.
//!
//! A token opens **its own webhook only** — one that leaks (a CI log, a chat) fires one prompt, not the hub. A new token
//! for the same webhook replaces the old one, which is how a leaked one is rotated. No cache: every call rereads the
//! file, so a rotation made from another process (the CLI) holds on the very next request.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::settings::keys_match;

/// What every token starts with, so one is easy to recognize (and to find in a leaked log).
pub const TOKEN_PREFIX: &str = "whk_";

/// How much of a token the list shows, prefix included.
const SHOWN_CHARS: usize = 10;

/// `last_used_at_ms` is only rewritten when older than this, so a busy integration doesn't rewrite the file on every call.
const LAST_USED_GRANULARITY_MS: i64 = 60_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebhookToken {
    /// The webhook it opens.
    pub webhook: String,
    /// The first characters of the token, `whk_` included, for recognizing it in the list.
    pub shown: String,
    pub created_at_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used_at_ms: Option<i64>,
    hash: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct TokensFile {
    #[serde(default)]
    tokens: Vec<WebhookToken>,
}

/// A token just created: `token` is the only time it exists outside the client that asked for it.
#[derive(Debug, Clone)]
pub struct CreatedToken {
    pub token: String,
    pub info: WebhookToken,
}

fn hash_token(token: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(token.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

fn now_millis() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as i64
}

/// Serializes this process's read-modify-writes of the file (several calls at once, plus a rotation).
static WRITES: Mutex<()> = Mutex::new(());

pub struct WebhookTokenStore {
    path: PathBuf,
}

impl WebhookTokenStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn load(&self) -> anyhow::Result<TokensFile> {
        match std::fs::read_to_string(&self.path) {
            Ok(contents) => Ok(serde_json::from_str(&contents)?),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(TokensFile::default()),
            Err(err) => Err(err.into()),
        }
    }

    fn save(&self, file: &TokensFile) -> anyhow::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&self.path, serde_json::to_string_pretty(file)?)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }

    /// Every webhook that has a token, oldest first.
    pub fn list(&self) -> anyhow::Result<Vec<WebhookToken>> {
        Ok(self.load()?.tokens)
    }

    /// A new token for `webhook`, replacing the one it had. The store doesn't read the config: the caller checks that the
    /// webhook exists.
    pub fn create(&self, webhook: &str) -> anyhow::Result<CreatedToken> {
        let token = format!("{TOKEN_PREFIX}{}", warden_bootstrap::generate_auth_key());
        let info = WebhookToken { webhook: webhook.to_string(), shown: token.chars().take(SHOWN_CHARS).collect(), created_at_ms: now_millis(), last_used_at_ms: None, hash: hash_token(&token) };
        let _guard = WRITES.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut file = self.load()?;
        file.tokens.retain(|t| t.webhook != webhook);
        file.tokens.push(info.clone());
        self.save(&file)?;
        Ok(CreatedToken { token, info })
    }

    /// `false` when the webhook has no token.
    pub fn revoke(&self, webhook: &str) -> anyhow::Result<bool> {
        let _guard = WRITES.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut file = self.load()?;
        let before = file.tokens.len();
        file.tokens.retain(|t| t.webhook != webhook);
        if file.tokens.len() == before {
            return Ok(false);
        }
        self.save(&file)?;
        Ok(true)
    }

    /// Whether `presented` is `webhook`'s token, noting that it was used. A webhook with no token, a token of another
    /// webhook and a wrong one all say `false` — the caller can't tell them apart.
    pub fn authenticate(&self, webhook: &str, presented: &str) -> anyhow::Result<bool> {
        if !presented.starts_with(TOKEN_PREFIX) {
            return Ok(false);
        }
        let hash = hash_token(presented);
        let _guard = WRITES.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut file = self.load()?;
        let Some(token) = file.tokens.iter_mut().find(|t| t.webhook == webhook) else {
            return Ok(false);
        };
        if !keys_match(&hash, &token.hash) {
            return Ok(false);
        }
        let now = now_millis();
        if token.last_used_at_ms.is_none_or(|last| now - last >= LAST_USED_GRANULARITY_MS) {
            token.last_used_at_ms = Some(now);
            self.save(&file)?;
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(name: &str) -> WebhookTokenStore {
        let dir = std::env::temp_dir().join(format!("warden-webhook-tokens-{name}-{}", now_millis()));
        WebhookTokenStore::new(dir.join("webhook_tokens.json"))
    }

    #[test]
    fn a_token_is_shown_once_and_only_its_hash_is_kept() {
        let store = store("create");
        let created = store.create("build").unwrap();
        assert!(created.token.starts_with(TOKEN_PREFIX));
        assert!(created.token.starts_with(&created.info.shown) && created.info.shown.len() < created.token.len());
        let on_disk = std::fs::read_to_string(store.path()).unwrap();
        assert!(!on_disk.contains(&created.token), "the token itself never reaches the disk");
        assert_eq!(store.list().unwrap(), vec![created.info]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(store.path()).unwrap().permissions().mode() & 0o777, 0o600, "not readable by other users");
        }
    }

    #[test]
    fn a_token_opens_its_own_webhook_only() {
        let store = store("own");
        let build = store.create("build").unwrap();
        let deploy = store.create("deploy").unwrap();
        assert!(store.authenticate("build", &build.token).unwrap());
        assert!(store.authenticate("deploy", &deploy.token).unwrap());
        assert!(!store.authenticate("build", &deploy.token).unwrap(), "another webhook's token");
        assert!(!store.authenticate("ghost", &build.token).unwrap(), "a webhook with no token");
        assert!(!store.authenticate("build", "whk_nope").unwrap());
        assert!(!store.authenticate("build", "no-prefix").unwrap());
        assert!(!store.authenticate("build", "").unwrap());
        assert!(store.list().unwrap().iter().find(|t| t.webhook == "build").unwrap().last_used_at_ms.is_some(), "the use is noted");
    }

    #[test]
    fn a_new_token_replaces_the_old_one_and_a_revoked_one_stops_working() {
        let store = store("rotate");
        let old = store.create("build").unwrap();
        let new = store.create("build").unwrap();
        assert_ne!(old.token, new.token);
        assert_eq!(store.list().unwrap().len(), 1, "one token per webhook");
        assert!(!store.authenticate("build", &old.token).unwrap(), "the old token is gone");
        assert!(store.authenticate("build", &new.token).unwrap());

        assert!(store.revoke("build").unwrap());
        assert!(!store.revoke("build").unwrap());
        assert!(!store.authenticate("build", &new.token).unwrap(), "a revoked token stops working");
    }
}
