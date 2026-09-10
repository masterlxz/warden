//! `warden_core::storage::AuthProvider` face for TruthID (P61).
//!
//! **`is_subscription_active` here is a pairing check, not a real subscription check** — there is
//! no accounts/billing system anywhere in this codebase yet (confirmed by grepping the whole repo
//! before writing this; see `project/PENDING.md` P61). TruthID's `pin()` is pay-per-publish through
//! the paired phone's own Arweave wallet, not a recurring charge Warden itself could verify. Until a
//! real subscription system exists, "does this device have a paired `owner_address`" is the closest
//! honest proxy — documented explicitly here so nobody mistakes it for the real thing later.

use std::path::PathBuf;

use async_trait::async_trait;
use warden_core::storage::AuthProvider;

use crate::manifest;

/// Reads TruthID's actual on-disk pairing state (`SyncManifest`) to answer `AuthProvider` — doesn't
/// own a `SyncEngine` (that also needs a vault/config path this trait has no use for) and doesn't
/// drive pairing itself. `login`/`logout` return an explicit error: the trait's no-argument
/// signature can't carry a pairing code, and the real flow (`SyncEngine::pairing_host`/
/// `pairing_join`) is inherently asynchronous and QR-mediated, not a fire-and-forget call — callers
/// that need to actually pair or unpair a device should keep driving `SyncEngine` directly and treat
/// this `AuthProvider` as read-only status.
pub struct TruthIdAuthProvider {
    manifest_path: PathBuf,
}

impl TruthIdAuthProvider {
    pub fn new(manifest_path: PathBuf) -> Self {
        Self { manifest_path }
    }
}

#[async_trait]
impl AuthProvider for TruthIdAuthProvider {
    async fn get_user_id(&self) -> anyhow::Result<Option<String>> {
        Ok(manifest::load_manifest(&self.manifest_path)?.owner_address)
    }

    async fn is_subscription_active(&self) -> anyhow::Result<bool> {
        Ok(manifest::load_manifest(&self.manifest_path)?.is_paired())
    }

    async fn login(&self) -> anyhow::Result<()> {
        anyhow::bail!(
            "TruthIdAuthProvider doesn't support login() directly — pairing is QR-mediated and asynchronous; \
             drive it through SyncEngine::pairing_host/pairing_join instead"
        )
    }

    async fn logout(&self) -> anyhow::Result<()> {
        anyhow::bail!(
            "TruthIdAuthProvider doesn't support logout() directly — there's no session to tear down, only a \
             paired device's secrets/manifest on disk"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_manifest_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "warden-auth-provider-test-{name}-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ))
    }

    #[tokio::test]
    async fn reports_no_user_and_inactive_before_any_manifest_exists() {
        let provider = TruthIdAuthProvider::new(temp_manifest_path("missing"));
        assert_eq!(provider.get_user_id().await.unwrap(), None);
        assert!(!provider.is_subscription_active().await.unwrap());
    }

    #[tokio::test]
    async fn reports_the_owner_address_and_active_once_paired() {
        let path = temp_manifest_path("paired");
        let manifest = manifest::SyncManifest { version: 1, owner_address: Some("wallet-abc".to_string()), ..Default::default() };
        manifest::save_manifest(&path, &manifest).unwrap();

        let provider = TruthIdAuthProvider::new(path);
        assert_eq!(provider.get_user_id().await.unwrap(), Some("wallet-abc".to_string()));
        assert!(provider.is_subscription_active().await.unwrap());
    }

    #[tokio::test]
    async fn login_and_logout_error_clearly_instead_of_pretending_to_work() {
        let provider = TruthIdAuthProvider::new(temp_manifest_path("no-op"));
        assert!(provider.login().await.is_err());
        assert!(provider.logout().await.is_err());
    }
}
