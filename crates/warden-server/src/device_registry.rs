//! Fase 9.3 — persistent device pairing, the piece 9.4/9.5's routing (`CallDeviceTool`) was
//! missing: knowing the shared `auth_key` is enough to `Hello` and chat, but routing a tool call
//! to (or from) a device now also requires that device to have been explicitly `approve`d by the
//! operator (via `warden-server devices approve <id>`, `main.rs`) — not just present and using the
//! right key. See `server.rs`'s `Hello`/`CallDeviceTool` handling and `project/PENDING.md` P61.
//!
//! `PairingStore` is deliberately stateless between calls — each method re-reads the JSON file
//! from disk, mutates if needed, and writes it back. The long-running `warden-server` process and
//! a one-shot `warden-server devices approve <id>` invocation are two separate processes sharing
//! this file as their only coordination; an approval made while the server is already up must take
//! effect on the *next* `CallDeviceTool` without a restart, which a cached in-memory copy
//! wouldn't give us. The file is tiny and touched at most once per `Hello`/`CallDeviceTool`, so the
//! read-mutate-write cost is a non-issue. Known limitation, accepted for this MVP: two writes
//! racing (e.g. an operator's `approve` landing at the exact instant a `Hello` updates
//! `last_seen_ms`) can lose one of them — a single-operator CLI tool at this call frequency doesn't
//! warrant real file locking yet.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PairingStatus {
    Pending,
    Approved,
    Revoked,
}

impl std::fmt::Display for PairingStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PairingStatus::Pending => write!(f, "pending"),
            PairingStatus::Approved => write!(f, "approved"),
            PairingStatus::Revoked => write!(f, "revoked"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairedDevice {
    pub device_name: String,
    pub status: PairingStatus,
    pub first_seen_ms: i64,
    pub last_seen_ms: i64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct RegistryFile {
    #[serde(default)]
    devices: HashMap<String, PairedDevice>,
}

fn now_millis() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as i64
}

fn load(path: &Path) -> anyhow::Result<RegistryFile> {
    match std::fs::read_to_string(path) {
        Ok(contents) => Ok(serde_json::from_str(&contents)?),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(RegistryFile::default()),
        Err(err) => Err(err.into()),
    }
}

fn save(path: &Path, file: &RegistryFile) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(file)?)?;
    Ok(())
}

/// Handle onto the on-disk registry at `path` — see module docs for why this holds no in-memory
/// state beyond the path itself.
pub struct PairingStore {
    path: PathBuf,
}

impl PairingStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Called once per successful `Hello` (`server.rs`) — records a never-seen `device_id` as
    /// `Pending` (visible to the operator via `devices list`), or just refreshes `device_name`/
    /// `last_seen_ms` for one already known, **without** touching an existing `Approved`/`Revoked`
    /// status. Returns the status the device has right after this call.
    pub fn record_seen(&self, device_id: &str, device_name: &str) -> anyhow::Result<PairingStatus> {
        let mut file = load(&self.path)?;
        let now = now_millis();
        let status = match file.devices.get_mut(device_id) {
            Some(existing) => {
                existing.device_name = device_name.to_string();
                existing.last_seen_ms = now;
                existing.status
            }
            None => {
                file.devices.insert(
                    device_id.to_string(),
                    PairedDevice { device_name: device_name.to_string(), status: PairingStatus::Pending, first_seen_ms: now, last_seen_ms: now },
                );
                PairingStatus::Pending
            }
        };
        save(&self.path, &file)?;
        Ok(status)
    }

    pub fn status(&self, device_id: &str) -> anyhow::Result<Option<PairingStatus>> {
        Ok(load(&self.path)?.devices.get(device_id).map(|d| d.status))
    }

    pub fn approve(&self, device_id: &str) -> anyhow::Result<()> {
        self.set_status(device_id, PairingStatus::Approved)
    }

    pub fn revoke(&self, device_id: &str) -> anyhow::Result<()> {
        self.set_status(device_id, PairingStatus::Revoked)
    }

    fn set_status(&self, device_id: &str, status: PairingStatus) -> anyhow::Result<()> {
        let mut file = load(&self.path)?;
        let device = file
            .devices
            .get_mut(device_id)
            .ok_or_else(|| anyhow::anyhow!("device '{device_id}' has never connected to this server — it needs to Hello at least once before it can be {status}"))?;
        device.status = status;
        save(&self.path, &file)
    }

    pub fn list(&self) -> anyhow::Result<Vec<(String, PairedDevice)>> {
        let mut devices: Vec<_> = load(&self.path)?.devices.into_iter().collect();
        devices.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(devices)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path() -> PathBuf {
        std::env::temp_dir().join(format!(
            "warden-device-registry-test-{}.json",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ))
    }

    #[test]
    fn a_never_seen_device_starts_pending() {
        let store = PairingStore::new(temp_path());
        let status = store.record_seen("dev-a", "Device A").unwrap();
        assert_eq!(status, PairingStatus::Pending);
        assert_eq!(store.status("dev-a").unwrap(), Some(PairingStatus::Pending));
    }

    #[test]
    fn reconnecting_refreshes_name_but_not_an_existing_approved_status() {
        let store = PairingStore::new(temp_path());
        store.record_seen("dev-a", "Old Name").unwrap();
        store.approve("dev-a").unwrap();

        let status = store.record_seen("dev-a", "New Name").unwrap();
        assert_eq!(status, PairingStatus::Approved);

        let devices = store.list().unwrap();
        assert_eq!(devices[0].1.device_name, "New Name");
        assert_eq!(devices[0].1.status, PairingStatus::Approved);
    }

    #[test]
    fn approve_and_revoke_change_status() {
        let store = PairingStore::new(temp_path());
        store.record_seen("dev-a", "Device A").unwrap();

        store.approve("dev-a").unwrap();
        assert_eq!(store.status("dev-a").unwrap(), Some(PairingStatus::Approved));

        store.revoke("dev-a").unwrap();
        assert_eq!(store.status("dev-a").unwrap(), Some(PairingStatus::Revoked));
    }

    #[test]
    fn approving_or_revoking_an_unknown_device_errors() {
        let store = PairingStore::new(temp_path());
        assert!(store.approve("dev-ghost").is_err());
        assert!(store.revoke("dev-ghost").is_err());
    }

    #[test]
    fn status_of_an_unknown_device_is_none() {
        let store = PairingStore::new(temp_path());
        assert_eq!(store.status("dev-ghost").unwrap(), None);
    }

    #[test]
    fn state_survives_a_reload_from_a_fresh_store_at_the_same_path() {
        let path = temp_path();
        PairingStore::new(&path).record_seen("dev-a", "Device A").unwrap();
        PairingStore::new(&path).approve("dev-a").unwrap();

        // A brand new `PairingStore` (simulating a separate `devices approve` CLI invocation, or
        // the server process restarting) sees the same state purely from disk.
        let reloaded = PairingStore::new(&path);
        assert_eq!(reloaded.status("dev-a").unwrap(), Some(PairingStatus::Approved));
    }

    #[test]
    fn list_is_sorted_by_device_id() {
        let store = PairingStore::new(temp_path());
        store.record_seen("dev-b", "B").unwrap();
        store.record_seen("dev-a", "A").unwrap();

        let ids: Vec<_> = store.list().unwrap().into_iter().map(|(id, _)| id).collect();
        assert_eq!(ids, vec!["dev-a".to_string(), "dev-b".to_string()]);
    }
}
