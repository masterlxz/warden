//! The credentials of the hub's incoming webhooks (P105): one per webhook, created with `warden-server webhooks token <id>`
//! (or from the web and desktop screens) and presented to `POST /hooks/<id>` (`webhooks.rs`) in one of two ways:
//!
//! - **a token** (`whk_…`), as `Authorization: Bearer <token>` (or `X-Warden-Token`). Like the Warden API's keys
//!   (`api_keys.rs`), only a SHA-256 of it is kept, so a leaked `webhook_tokens.json` hands out nothing that works, and the
//!   token is shown once, when it's created;
//! - **a signing secret** (`whsec_…`), for services that sign what they send (GitHub, Gitea, Stripe) and can't be given a
//!   header of our choosing. The secret is shown once too, **but it has to be kept in the clear**: checking an HMAC means
//!   making one. The file is `0600`, and anyone who can read it could sign a call, which is the price of accepting
//!   signatures at all.
//!
//! A credential opens **its own webhook only** — one that leaks (a CI log, a chat) fires one prompt, not the hub. A new
//! credential for the same webhook replaces the old one, which is how a leaked one is rotated. No cache: every call
//! rereads the file, so a rotation made from another process (the CLI) holds on the very next request.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use warden_bootstrap::webhooks::WebhookAuth;

use crate::settings::keys_match;

/// What every token starts with, so one is easy to recognize (and to find in a leaked log).
pub const TOKEN_PREFIX: &str = "whk_";

/// What every signing secret starts with.
pub const SECRET_PREFIX: &str = "whsec_";

/// How much of a credential the list shows, prefix included.
const SHOWN_CHARS: usize = 10;

/// `last_used_at_ms` is only rewritten when older than this, so a busy integration doesn't rewrite the file on every call.
const LAST_USED_GRANULARITY_MS: i64 = 60_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebhookToken {
    /// The webhook it opens.
    pub webhook: String,
    /// A token or a signing secret. Absent in a file from before secrets existed, so those are tokens.
    #[serde(default, skip_serializing_if = "WebhookAuth::is_token")]
    pub kind: WebhookAuth,
    /// The first characters of the credential, prefix included, for recognizing it in the list.
    pub shown: String,
    pub created_at_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used_at_ms: Option<i64>,
    /// SHA-256 of a token. Empty for a signing secret, which is kept in `secret`.
    #[serde(default)]
    hash: String,
    /// The signing secret, in the clear (see the module doc). `None` for a token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    secret: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct TokensFile {
    #[serde(default)]
    tokens: Vec<WebhookToken>,
}

/// A credential just created: `token` is the token, or the signing secret for `info.kind == Hmac` — the only time it is
/// shown outside the file, for a token not even there.
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

    /// Every webhook that has a credential, oldest first.
    pub fn list(&self) -> anyhow::Result<Vec<WebhookToken>> {
        Ok(self.load()?.tokens)
    }

    /// What kind of credential `webhook` has, or `None` when it has none.
    pub fn kind_of(&self, webhook: &str) -> anyhow::Result<Option<WebhookAuth>> {
        Ok(self.load()?.tokens.iter().find(|t| t.webhook == webhook).map(|t| t.kind))
    }

    /// A new token for `webhook`, replacing the credential it had. The store doesn't read the config: the caller checks
    /// that the webhook exists.
    pub fn create(&self, webhook: &str) -> anyhow::Result<CreatedToken> {
        self.create_credential(webhook, WebhookAuth::Token)
    }

    /// A new signing secret for `webhook` (`whsec_…`), replacing the credential it had.
    pub fn create_secret(&self, webhook: &str) -> anyhow::Result<CreatedToken> {
        self.create_credential(webhook, WebhookAuth::Hmac)
    }

    /// A new credential of `kind` for `webhook`.
    pub fn create_credential(&self, webhook: &str, kind: WebhookAuth) -> anyhow::Result<CreatedToken> {
        let prefix = match kind {
            WebhookAuth::Token => TOKEN_PREFIX,
            WebhookAuth::Hmac => SECRET_PREFIX,
        };
        let credential = format!("{prefix}{}", warden_bootstrap::generate_auth_key());
        let (hash, secret) = match kind {
            WebhookAuth::Token => (hash_token(&credential), None),
            WebhookAuth::Hmac => (String::new(), Some(credential.clone())),
        };
        let info = WebhookToken { webhook: webhook.to_string(), kind, shown: credential.chars().take(SHOWN_CHARS).collect(), created_at_ms: now_millis(), last_used_at_ms: None, hash, secret };
        let _guard = WRITES.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut file = self.load()?;
        file.tokens.retain(|t| t.webhook != webhook);
        file.tokens.push(info.clone());
        self.save(&file)?;
        Ok(CreatedToken { token: credential, info })
    }

    /// Moves `old`'s credential to `new` (the webhook was renamed): it keeps working, the way it did. `false` when `old`
    /// had none. A credential `new` already had is replaced, so it can never be two.
    pub fn rename(&self, old: &str, new: &str) -> anyhow::Result<bool> {
        let _guard = WRITES.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut file = self.load()?;
        if !file.tokens.iter().any(|t| t.webhook == old) {
            return Ok(false);
        }
        file.tokens.retain(|t| t.webhook != new || t.webhook == old);
        if let Some(token) = file.tokens.iter_mut().find(|t| t.webhook == old) {
            token.webhook = new.to_string();
        }
        self.save(&file)?;
        Ok(true)
    }

    /// `false` when the webhook has no credential.
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

    /// Whether `presented` is `webhook`'s token, noting that it was used. A webhook with no credential, one whose credential
    /// is a signing secret, a token of another webhook and a wrong one all say `false` — the caller can't tell them apart.
    pub fn authenticate(&self, webhook: &str, presented: &str) -> anyhow::Result<bool> {
        if !presented.starts_with(TOKEN_PREFIX) {
            return Ok(false);
        }
        let hash = hash_token(presented);
        let _guard = WRITES.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut file = self.load()?;
        let Some(token) = file.tokens.iter_mut().find(|t| t.webhook == webhook && t.kind == WebhookAuth::Token) else {
            return Ok(false);
        };
        if !keys_match(&hash, &token.hash) {
            return Ok(false);
        }
        if note_use(token) {
            self.save(&file)?;
        }
        Ok(true)
    }

    /// The signing secret of `webhook`, when its credential is one (`None` for a token or no credential).
    pub fn secret_of(&self, webhook: &str) -> anyhow::Result<Option<String>> {
        Ok(self.load()?.tokens.into_iter().find(|t| t.webhook == webhook && t.kind == WebhookAuth::Hmac).and_then(|t| t.secret))
    }

    /// Notes that `webhook`'s signature was just checked and good (a token notes its own use in `authenticate`).
    pub fn note_signature_used(&self, webhook: &str) -> anyhow::Result<()> {
        let _guard = WRITES.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut file = self.load()?;
        if let Some(token) = file.tokens.iter_mut().find(|t| t.webhook == webhook && t.kind == WebhookAuth::Hmac) {
            if note_use(token) {
                self.save(&file)?;
            }
        }
        Ok(())
    }
}

/// Marks `token` as used now, unless it was a moment ago. Whether the file needs rewriting.
fn note_use(token: &mut WebhookToken) -> bool {
    let now = now_millis();
    if token.last_used_at_ms.is_none_or(|last| now - last >= LAST_USED_GRANULARITY_MS) {
        token.last_used_at_ms = Some(now);
        return true;
    }
    false
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

    #[test]
    fn a_signing_secret_is_kept_to_verify_with_and_is_never_a_token() {
        let store = store("secret");
        let created = store.create_secret("gh").unwrap();
        assert!(created.token.starts_with(SECRET_PREFIX));
        assert_eq!(created.info.kind, WebhookAuth::Hmac);
        assert_eq!(store.kind_of("gh").unwrap(), Some(WebhookAuth::Hmac));
        assert_eq!(store.secret_of("gh").unwrap().as_deref(), Some(created.token.as_str()), "the secret has to be there to verify with");
        assert!(std::fs::read_to_string(store.path()).unwrap().contains(&created.token), "kept in the clear: that is the price of HMAC");
        // The secret is not a bearer token, so a webhook that signs can't be opened with it as one.
        assert!(!store.authenticate("gh", &created.token).unwrap());
        assert!(!store.authenticate("gh", &format!("{TOKEN_PREFIX}x")).unwrap());

        // A token has no secret to give, and a webhook without credential has neither.
        let token = store.create("tok").unwrap();
        assert_eq!((store.secret_of("tok").unwrap(), store.kind_of("tok").unwrap()), (None, Some(WebhookAuth::Token)));
        assert!(store.authenticate("tok", &token.token).unwrap());
        assert_eq!((store.secret_of("nobody").unwrap(), store.kind_of("nobody").unwrap()), (None, None));

        // Replacing a credential by one of the other kind drops the first.
        let swapped = store.create("gh").unwrap();
        assert_eq!(store.kind_of("gh").unwrap(), Some(WebhookAuth::Token));
        assert_eq!(store.secret_of("gh").unwrap(), None, "the secret is gone with the replaced credential");
        assert!(store.authenticate("gh", &swapped.token).unwrap());
        assert!(!std::fs::read_to_string(store.path()).unwrap().contains(&created.token));
    }

    #[test]
    fn a_renamed_webhook_keeps_its_credential_and_never_ends_up_with_two() {
        let store = store("rename");
        let token = store.create("old").unwrap();
        store.create("taken").unwrap();
        assert!(store.rename("old", "taken").unwrap());
        assert!(store.authenticate("taken", &token.token).unwrap(), "the credential moved with the name");
        assert!(!store.authenticate("old", &token.token).unwrap());
        assert_eq!(store.list().unwrap().iter().filter(|t| t.webhook == "taken").count(), 1, "the one that was there is replaced");
        assert!(!store.rename("ghost", "other").unwrap(), "nothing to move");
        assert!(store.rename("taken", "taken").unwrap(), "a rename to itself changes nothing");
        assert!(store.authenticate("taken", &token.token).unwrap());
    }

    #[test]
    fn a_signature_use_is_noted_and_an_old_file_still_reads_as_tokens() {
        let store = store("used");
        store.create_secret("gh").unwrap();
        assert_eq!(store.list().unwrap()[0].last_used_at_ms, None);
        store.note_signature_used("gh").unwrap();
        assert!(store.list().unwrap()[0].last_used_at_ms.is_some());
        store.note_signature_used("ghost").unwrap();

        // A file from before secrets existed: no `kind`, no `secret`.
        let old = self::store("old");
        std::fs::create_dir_all(old.path().parent().unwrap()).unwrap();
        std::fs::write(old.path(), r#"{"tokens":[{"webhook":"build","shown":"whk_abc","created_at_ms":1,"hash":"h"}]}"#).unwrap();
        assert_eq!(old.kind_of("build").unwrap(), Some(WebhookAuth::Token));
        assert_eq!(old.secret_of("build").unwrap(), None);
    }
}
