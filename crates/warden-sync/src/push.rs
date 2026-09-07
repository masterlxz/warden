use std::path::Path;

use warden_core::memory::Vault;
use warden_truthid::{PendingPin, PinResult};

use crate::arweave::ArweaveClient;
use crate::bundle::{self, ConfigChange, SyncBundle};
use crate::diff;
use crate::manifest::{SyncDirection, SyncManifest, SyncSecrets};

/// Everything needed to render a QR (`pending.qr_payload_json()`) *before* blocking on the
/// phone — the caller shows the QR, then awaits `run_push`.
pub struct BeginPushResult {
    pub pending: PendingPin,
    pub encrypted_bundle: Vec<u8>,
    pub bundle: SyncBundle,
}

#[derive(Debug)]
pub struct PushOutcome {
    pub tx_id: String,
    pub files_changed: usize,
    pub config_changed: bool,
}

/// Computes the diff, builds and encrypts the bundle, and starts a `PendingPin` session.
/// Returns `None` if neither the vault nor `config.toml` changed since the last successful sync
/// — nothing to send.
pub fn begin_push(
    vault: &Vault,
    config_path: &Path,
    secrets: &SyncSecrets,
    manifest: &SyncManifest,
) -> anyhow::Result<Option<BeginPushResult>> {
    let diff = diff::diff_vault(vault, manifest)?;
    let config_bytes = std::fs::read(config_path).ok();
    let config_changed = diff::config_changed(config_bytes.as_deref(), manifest);

    if diff.is_empty() && !config_changed {
        return Ok(None);
    }

    let config_change = if !config_changed {
        None
    } else {
        match config_bytes {
            Some(bytes) => Some(ConfigChange::Updated(bytes)),
            None => Some(ConfigChange::Deleted),
        }
    };

    let next_counter = manifest.manifest_counter + 1;
    let bundle = bundle::build_bundle(vault, config_change, &diff, next_counter, &secrets.device_id)?;
    let encrypted_bundle = bundle::encrypt_bundle(&bundle, &secrets.vault_key)?;
    let pending = PendingPin::begin_with_default_timeout("Warden")?;

    Ok(Some(BeginPushResult { pending, encrypted_bundle, bundle }))
}

/// Runs the actual TruthID exchange (`pending.run`, sweeping every real LAN host — see
/// `run_push_with_hosts` to target a known set instead, e.g. in tests), then folds the result
/// into `manifest` — returns the new manifest for the caller to persist (this crate never assumes
/// where the manifest file lives; `SyncEngine` owns that).
pub async fn run_push(
    begin: BeginPushResult,
    arweave: &ArweaveClient,
    manifest: SyncManifest,
) -> anyhow::Result<(PushOutcome, SyncManifest)> {
    let hosts = warden_truthid::lan::candidate_hosts()?;
    run_push_with_hosts(begin, arweave, manifest, hosts).await
}

/// Same as `run_push`, but sweeps only `hosts` — lets tests target a fake phone bound to
/// `127.0.0.1` (excluded from `candidate_hosts()`'s real-LAN sweep) instead of the real network.
pub async fn run_push_with_hosts(
    begin: BeginPushResult,
    arweave: &ArweaveClient,
    manifest: SyncManifest,
    hosts: Vec<std::net::Ipv4Addr>,
) -> anyhow::Result<(PushOutcome, SyncManifest)> {
    let BeginPushResult { pending, encrypted_bundle, bundle } = begin;
    let pin_result = pending.run_with_hosts(&encrypted_bundle, hosts).await?;
    let mut new_manifest = apply_push_result(manifest, &bundle, &pin_result)?;

    if new_manifest.owner_address.is_none() {
        if let Some(tx_id) = new_manifest.last_tx_id.clone() {
            if let Ok(Some(owner)) = arweave.owner_of_tx(&tx_id).await {
                new_manifest.owner_address = Some(owner);
            }
        }
    }

    let outcome = PushOutcome {
        tx_id: new_manifest.last_tx_id.clone().unwrap_or_default(),
        files_changed: bundle.vault_files.len() + bundle.deleted_vault_files.len(),
        config_changed: bundle.config_toml.is_some() || bundle.config_deleted,
    };
    Ok((outcome, new_manifest))
}

/// Pure fold of a `PinResult` into `manifest` — factored out of `run_push` specifically so it's
/// unit-testable without any network/`PendingPin` involvement.
fn apply_push_result(manifest: SyncManifest, bundle: &SyncBundle, pin_result: &PinResult) -> anyhow::Result<SyncManifest> {
    if pin_result.status != "pinned" {
        anyhow::bail!(pin_result
            .error
            .clone()
            .unwrap_or_else(|| format!("TruthID pin failed with status {:?}", pin_result.status)));
    }
    let tx_id = pin_result
        .cid
        .as_deref()
        .and_then(|cid| cid.strip_prefix("ar://"))
        .ok_or_else(|| anyhow::anyhow!("TruthID pin succeeded but returned no cid"))?
        .to_string();

    bundle::fold_into_manifest(manifest, bundle, tx_id, SyncDirection::Push)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::STANDARD as BASE64;
    use base64::Engine;
    use diff::sha256_hex;

    fn temp_vault(suffix: &str) -> Vault {
        let dir = std::env::temp_dir().join(format!(
            "warden-sync-push-test-{suffix}-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        Vault::new(dir)
    }

    #[test]
    fn begin_push_is_none_when_nothing_changed() {
        let vault = temp_vault("nochange");
        vault.write("a.md", "hello").unwrap();
        let mut manifest = SyncManifest::default();
        manifest.vault_files.insert("a.md".to_string(), sha256_hex(b"hello"));
        let secrets = crate::manifest::generate_secrets();
        let missing_config = std::env::temp_dir().join("warden-sync-push-test-no-config.toml");

        let result = begin_push(&vault, &missing_config, &secrets, &manifest).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn begin_push_is_some_with_the_right_file_list_when_something_changed() {
        let vault = temp_vault("change");
        vault.write("a.md", "hello").unwrap();
        let manifest = SyncManifest::default();
        let secrets = crate::manifest::generate_secrets();
        let missing_config = std::env::temp_dir().join("warden-sync-push-test-no-config-2.toml");

        let result = begin_push(&vault, &missing_config, &secrets, &manifest).unwrap().unwrap();
        assert_eq!(result.bundle.vault_files.len(), 1);
        assert!(result.bundle.vault_files.contains_key("a.md"));
        assert_eq!(result.bundle.manifest_counter, 1);
    }

    fn sample_bundle() -> SyncBundle {
        SyncBundle {
            schema_version: 1,
            manifest_counter: 3,
            created_at_ms: 0,
            device_id: "device-a".to_string(),
            vault_files: [("a.md".to_string(), BASE64.encode(b"hello"))].into_iter().collect(),
            deleted_vault_files: vec!["old.md".to_string()],
            config_toml: Some(BASE64.encode(b"vault_path = \"vault\"")),
            config_deleted: false,
        }
    }

    #[test]
    fn apply_push_result_updates_the_manifest_on_success() {
        let mut manifest = SyncManifest::default();
        manifest.vault_files.insert("old.md".to_string(), sha256_hex(b"bye"));
        let bundle = sample_bundle();
        let pin_result = PinResult {
            status: "pinned".to_string(),
            cid: Some("ar://test-tx-id".to_string()),
            content_hash: None,
            providers_ok: None,
            providers_failed: None,
            error: None,
        };

        let updated = apply_push_result(manifest, &bundle, &pin_result).unwrap();
        assert_eq!(updated.last_tx_id.as_deref(), Some("test-tx-id"));
        assert_eq!(updated.manifest_counter, 3);
        assert_eq!(updated.vault_files.get("a.md"), Some(&sha256_hex(b"hello")));
        assert!(!updated.vault_files.contains_key("old.md"));
        assert!(updated.config_present);
        assert_eq!(updated.last_sync_direction, Some(SyncDirection::Push));
    }

    #[test]
    fn apply_push_result_errors_on_a_non_pinned_status() {
        let manifest = SyncManifest::default();
        let bundle = sample_bundle();
        let pin_result = PinResult {
            status: "rejected".to_string(),
            cid: None,
            content_hash: None,
            providers_ok: None,
            providers_failed: None,
            error: Some("user declined".to_string()),
        };

        let err = apply_push_result(manifest, &bundle, &pin_result).unwrap_err();
        assert_eq!(err.to_string(), "user declined");
    }
}
