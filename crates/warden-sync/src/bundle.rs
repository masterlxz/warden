use std::collections::HashMap;
use std::path::Path;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use hkdf::Hkdf;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use warden_core::memory::Vault;
use warden_truthid::crypto::{decrypt_pin_content, encrypt_pin_content};

use crate::diff::{sha256_hex, VaultDiff};
use crate::manifest::{SyncDirection, SyncManifest};

/// Deliberately distinct from `warden_truthid::crypto`'s own `"TruthID Pin Content"` HKDF
/// context — this key never needs to relate to TruthID's, and reusing their salt/info by
/// accident would be a subtle way to weaken key separation between two unrelated protocols.
const SYNC_BUNDLE_HKDF_SALT: &[u8] = b"Warden Sync Bundle";
const SYNC_BUNDLE_HKDF_INFO: &[u8] = b"bundle-content-key-v1";

pub fn derive_bundle_key(vault_key: &[u8; 32]) -> anyhow::Result<[u8; 32]> {
    let hk = Hkdf::<Sha256>::new(Some(SYNC_BUNDLE_HKDF_SALT), vault_key);
    let mut key = [0u8; 32];
    hk.expand(SYNC_BUNDLE_HKDF_INFO, &mut key).map_err(|_| anyhow::anyhow!("HKDF expand failed"))?;
    Ok(key)
}

/// What a config change looks like as of this push — distinguishes "nothing changed" (bundle
/// carries neither field) from "config was deleted" (`config_deleted: true`, no content) from
/// "config was updated" (`config_toml: Some(...)`). `None` at the `build_bundle` call site means
/// "unchanged"; a config that's unchanged since the last sync is simply left out of the bundle.
pub enum ConfigChange {
    Updated(Vec<u8>),
    Deleted,
}

/// The plaintext envelope built once per Send, then serialized and encrypted as a single blob —
/// `pin()` has no batching (one physical phone approval per call), so every changed file for one
/// push has to become one bundle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncBundle {
    pub schema_version: u8,
    pub manifest_counter: u64,
    pub created_at_ms: i64,
    pub device_id: String,
    /// Relative vault path -> base64 of the file's raw bytes.
    pub vault_files: HashMap<String, String>,
    pub deleted_vault_files: Vec<String>,
    /// base64 of `config.toml`'s raw bytes, if it changed this push.
    pub config_toml: Option<String>,
    pub config_deleted: bool,
}

pub struct ApplyReport {
    pub files_written: usize,
    pub files_deleted: usize,
    pub config_updated: bool,
    pub config_deleted: bool,
}

pub fn build_bundle(
    vault: &Vault,
    config_change: Option<ConfigChange>,
    diff: &VaultDiff,
    manifest_counter: u64,
    device_id: &str,
) -> anyhow::Result<SyncBundle> {
    let mut vault_files = HashMap::new();
    for relative in &diff.added_or_modified {
        let bytes = std::fs::read(vault.root().join(relative))?;
        vault_files.insert(relative.to_string_lossy().to_string(), BASE64.encode(bytes));
    }
    let deleted_vault_files = diff.deleted.iter().map(|p| p.to_string_lossy().to_string()).collect();

    let (config_toml, config_deleted) = match config_change {
        Some(ConfigChange::Updated(bytes)) => (Some(BASE64.encode(bytes)), false),
        Some(ConfigChange::Deleted) => (None, true),
        None => (None, false),
    };

    Ok(SyncBundle {
        schema_version: 1,
        manifest_counter,
        created_at_ms: now_ms(),
        device_id: device_id.to_string(),
        vault_files,
        deleted_vault_files,
        config_toml,
        config_deleted,
    })
}

pub fn encrypt_bundle(bundle: &SyncBundle, vault_key: &[u8; 32]) -> anyhow::Result<Vec<u8>> {
    let key = derive_bundle_key(vault_key)?;
    let plaintext = serde_json::to_vec(bundle)?;
    encrypt_pin_content(&plaintext, &key)
}

pub fn decrypt_bundle(blob: &[u8], vault_key: &[u8; 32]) -> anyhow::Result<SyncBundle> {
    let key = derive_bundle_key(vault_key)?;
    let plaintext = decrypt_pin_content(blob, &key)?;
    Ok(serde_json::from_slice(&plaintext)?)
}

/// Writes a decrypted bundle onto disk — used by `pull`. Caller is responsible for deciding
/// beforehand whether any of this would clobber an unpushed local change (`pull::pull` does this
/// by comparing current on-disk hashes against the *previous* manifest before calling this).
pub fn apply_bundle(bundle: &SyncBundle, vault: &Vault, config_path: &Path) -> anyhow::Result<ApplyReport> {
    let mut files_written = 0;
    for (relative, encoded) in &bundle.vault_files {
        let bytes = BASE64.decode(encoded.as_bytes())?;
        let path = vault.root().join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, bytes)?;
        files_written += 1;
    }

    let mut files_deleted = 0;
    for relative in &bundle.deleted_vault_files {
        let path = vault.root().join(relative);
        match std::fs::remove_file(path) {
            Ok(()) => files_deleted += 1,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err.into()),
        }
    }

    let mut config_updated = false;
    if let Some(encoded) = &bundle.config_toml {
        let bytes = BASE64.decode(encoded.as_bytes())?;
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(config_path, bytes)?;
        config_updated = true;
    }
    if bundle.config_deleted {
        match std::fs::remove_file(config_path) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err.into()),
        }
    }

    Ok(ApplyReport { files_written, files_deleted, config_updated, config_deleted: bundle.config_deleted })
}

/// Folds a decrypted bundle's changes into `manifest` — shared by `push::run_push` (after a
/// successful `PinResult`) and `pull::pull` (after a successful fetch+decrypt), since both need
/// the exact same bookkeeping: update per-file hashes, drop deleted entries, refresh the config
/// hash, and record which tx/direction/counter this manifest now reflects.
pub fn fold_into_manifest(
    mut manifest: SyncManifest,
    bundle: &SyncBundle,
    tx_id: String,
    direction: SyncDirection,
) -> anyhow::Result<SyncManifest> {
    for (path, encoded) in &bundle.vault_files {
        let bytes = BASE64.decode(encoded.as_bytes())?;
        manifest.vault_files.insert(path.clone(), sha256_hex(&bytes));
    }
    for path in &bundle.deleted_vault_files {
        manifest.vault_files.remove(path);
    }
    if let Some(encoded) = &bundle.config_toml {
        let bytes = BASE64.decode(encoded.as_bytes())?;
        manifest.config_hash = Some(sha256_hex(&bytes));
        manifest.config_present = true;
    }
    if bundle.config_deleted {
        manifest.config_hash = None;
        manifest.config_present = false;
    }

    manifest.last_tx_id = Some(tx_id);
    manifest.last_sync_direction = Some(direction);
    manifest.last_synced_at_ms = Some(now_ms());
    manifest.manifest_counter = bundle.manifest_counter;
    Ok(manifest)
}

pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::diff_vault;
    use crate::manifest::SyncManifest;
    use std::path::PathBuf;

    fn temp_vault(suffix: &str) -> Vault {
        let dir = std::env::temp_dir().join(format!(
            "warden-sync-bundle-test-{suffix}-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        Vault::new(dir)
    }

    #[test]
    fn derive_bundle_key_differs_from_pin_content_key_for_the_same_input() {
        let vault_key = [7u8; 32];
        let bundle_key = derive_bundle_key(&vault_key).unwrap();
        // `derive_pin_content_key` takes a hex session id, not a raw key — feed it the hex
        // encoding of the same 32 bytes so both derivations start from equivalent input material.
        let pin_key = warden_truthid::crypto::derive_pin_content_key(&hex::encode(vault_key)).unwrap();
        assert_ne!(bundle_key, pin_key);
    }

    #[test]
    fn bundle_round_trips_through_build_encrypt_decrypt_apply() {
        let source = temp_vault("source");
        source.write("notes/todo.md", "buy milk").unwrap();
        source.write("keep.md", "unchanged").unwrap();

        let mut manifest = SyncManifest::default();
        manifest.vault_files.insert("keep.md".to_string(), crate::diff::sha256_hex(b"unchanged"));
        manifest.vault_files.insert("old/gone.md".to_string(), crate::diff::sha256_hex(b"bye"));
        // "gone.md" isn't on disk in `source` — diff_vault will report it as deleted.

        let diff = diff_vault(&source, &manifest).unwrap();
        let bundle = build_bundle(
            &source,
            Some(ConfigChange::Updated(b"vault_path = \"vault\"".to_vec())),
            &diff,
            1,
            "device-a",
        )
        .unwrap();

        let vault_key = [3u8; 32];
        let encrypted = encrypt_bundle(&bundle, &vault_key).unwrap();
        let decrypted = decrypt_bundle(&encrypted, &vault_key).unwrap();
        assert_eq!(decrypted.manifest_counter, 1);
        assert_eq!(decrypted.deleted_vault_files, vec!["old/gone.md".to_string()]);

        let dest = temp_vault("dest");
        let config_path: PathBuf = dest.root().join("config.toml");
        // Pre-create the "deleted" file on the destination so we can prove apply_bundle removes it.
        dest.write("old/gone.md", "bye").unwrap();

        let report = apply_bundle(&decrypted, &dest, &config_path).unwrap();
        assert_eq!(report.files_written, 1);
        assert_eq!(report.files_deleted, 1);
        assert!(report.config_updated);
        assert!(!report.config_deleted);

        assert_eq!(dest.read("notes/todo.md").unwrap(), "buy milk");
        assert!(!dest.root().join("old/gone.md").exists());
        assert_eq!(std::fs::read_to_string(&config_path).unwrap(), "vault_path = \"vault\"");
    }
}
