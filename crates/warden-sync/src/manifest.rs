use std::collections::HashMap;
use std::path::Path;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};

/// The vault-encryption key, generated once on the first device and shared with every other
/// Warden install via pairing (`crate::pairing`) — never touches the TruthID wallet or Arweave.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncSecrets {
    pub version: u8,
    /// 16 random bytes, hex-encoded — same "opaque unique-enough string" posture as the mobile
    /// client's own `generateDeviceId()` (`mobile/lib/services/device_id.dart`), deliberately not
    /// a `uuid`-crate v4 UUID (buys nothing over 128 bits of raw entropy for an id nothing else
    /// ever needs to parse).
    pub device_id: String,
    #[serde(with = "base64_32")]
    pub vault_key: [u8; 32],
}

/// Non-secret sync tracking state, rewritten after every successful push/pull.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct SyncManifest {
    pub version: u8,
    /// Arweave wallet address of the TruthID wallet that has been paying for our pins — learned
    /// once (either from this device's own first push, or from whichever device we paired with)
    /// and reused from then on to discover "latest" via `ArweaveClient::latest_tx_by_owner`.
    pub owner_address: Option<String>,
    /// Most recently applied/pushed Arweave tx id (no `ar://` prefix).
    pub last_tx_id: Option<String>,
    pub last_sync_direction: Option<SyncDirection>,
    pub last_synced_at_ms: Option<i64>,
    /// Monotonic counter carried inside every bundle (`bundle::SyncBundle::manifest_counter`) —
    /// lets `pull` refuse to apply a snapshot older than or equal to what's already applied.
    pub manifest_counter: u64,
    /// Relative vault path -> sha256 hex of its content, as of the last successful push/pull.
    pub vault_files: HashMap<String, String>,
    pub config_present: bool,
    pub config_hash: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncDirection {
    Push,
    Pull,
}

impl SyncManifest {
    pub fn is_paired(&self) -> bool {
        self.owner_address.is_some()
    }
}

/// 16 random bytes, hex-encoded — same "opaque unique-enough string" posture as the mobile
/// client's `generateDeviceId()` (`mobile/lib/services/device_id.dart`), deliberately not a
/// `uuid`-crate v4 UUID.
pub fn generate_device_id() -> String {
    let mut bytes = [0u8; 16];
    OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

pub fn generate_secrets() -> SyncSecrets {
    let mut vault_key = [0u8; 32];
    OsRng.fill_bytes(&mut vault_key);
    SyncSecrets { version: 1, device_id: generate_device_id(), vault_key }
}

pub fn load_secrets(path: &Path) -> anyhow::Result<Option<SyncSecrets>> {
    match std::fs::read_to_string(path) {
        Ok(contents) => Ok(Some(serde_json::from_str(&contents)?)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err.into()),
    }
}

pub fn save_secrets(path: &Path, secrets: &SyncSecrets) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(secrets)?)?;
    Ok(())
}

pub fn load_manifest(path: &Path) -> anyhow::Result<SyncManifest> {
    match std::fs::read_to_string(path) {
        Ok(contents) => Ok(serde_json::from_str(&contents)?),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(SyncManifest { version: 1, ..Default::default() }),
        Err(err) => Err(err.into()),
    }
}

pub fn save_manifest(path: &Path, manifest: &SyncManifest) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(manifest)?)?;
    Ok(())
}

mod base64_32 {
    use super::{Engine, BASE64};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8; 32], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&BASE64.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<[u8; 32], D::Error> {
        let encoded = String::deserialize(deserializer)?;
        let decoded = BASE64.decode(encoded.as_bytes()).map_err(serde::de::Error::custom)?;
        decoded.try_into().map_err(|v: Vec<u8>| serde::de::Error::custom(format!("expected 32 bytes, got {}", v.len())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "warden-sync-test-{name}-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ))
    }

    #[test]
    fn secrets_round_trip_through_json() {
        let secrets = generate_secrets();
        let path = temp_path("secrets");
        save_secrets(&path, &secrets).unwrap();
        let loaded = load_secrets(&path).unwrap().unwrap();
        assert_eq!(loaded.device_id, secrets.device_id);
        assert_eq!(loaded.vault_key, secrets.vault_key);
    }

    #[test]
    fn load_secrets_returns_none_when_missing() {
        assert!(load_secrets(&temp_path("missing")).unwrap().is_none());
    }

    #[test]
    fn load_manifest_defaults_when_missing() {
        let manifest = load_manifest(&temp_path("missing-manifest")).unwrap();
        assert!(!manifest.is_paired());
        assert_eq!(manifest.manifest_counter, 0);
    }

    #[test]
    fn manifest_round_trips_through_json() {
        let mut manifest = SyncManifest { version: 1, ..Default::default() };
        manifest.owner_address = Some("abc123".to_string());
        manifest.vault_files.insert("notes/todo.md".to_string(), "deadbeef".to_string());
        let path = temp_path("manifest");
        save_manifest(&path, &manifest).unwrap();
        let loaded = load_manifest(&path).unwrap();
        assert_eq!(loaded, manifest);
    }
}
