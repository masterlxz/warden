//! Tauri commands backing the "Workspace" screen (Fase 9.6) — lists/approves/revokes devices
//! paired with a `warden-server` hub (Fase 9.3, `warden_server::PairingStore`). No `AppState`
//! needed: `PairingStore` is stateless by design (just a `PathBuf`, rereads the JSON file on every
//! call — see `device_registry.rs`'s module docs), so a fresh one is built per command.
//!
//! **Scope decision**: this assumes the desktop app runs on the *same machine* as the
//! `warden-server` hub whose registry it's reading — it opens `devices.json` straight off disk via
//! `default_server_devices_path()`, the exact path `warden-server devices list/approve/revoke`
//! (the CLI) already reads/writes. A hub running on a different machine needs a real admin
//! surface over the WS protocol, which doesn't exist yet (no `ClientMessage`/`ServerMessage`
//! variant for it, and no answer yet to "which credential is allowed to approve a device" beyond
//! the one shared `auth_key`) — deliberately out of scope for this slice.

use serde::Serialize;
use warden_server::{PairedDevice, PairingStore};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairedDeviceInfo {
    device_id: String,
    device_name: String,
    status: String,
    first_seen_ms: i64,
    last_seen_ms: i64,
}

fn to_info(device_id: String, device: PairedDevice) -> PairedDeviceInfo {
    PairedDeviceInfo {
        device_id,
        device_name: device.device_name,
        status: device.status.to_string(),
        first_seen_ms: device.first_seen_ms,
        last_seen_ms: device.last_seen_ms,
    }
}

fn store() -> Result<PairingStore, String> {
    warden_bootstrap::default_server_devices_path()
        .map(PairingStore::new)
        .ok_or_else(|| "could not determine the OS config directory".to_string())
}

#[tauri::command]
pub fn list_paired_devices() -> Result<Vec<PairedDeviceInfo>, String> {
    Ok(store()?.list().map_err(|e| format!("{e:#}"))?.into_iter().map(|(id, device)| to_info(id, device)).collect())
}

#[tauri::command]
pub fn approve_paired_device(device_id: String) -> Result<(), String> {
    store()?.approve(&device_id).map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub fn revoke_paired_device(device_id: String) -> Result<(), String> {
    store()?.revoke(&device_id).map_err(|e| format!("{e:#}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use warden_server::PairingStatus;

    // Locks in the exact camelCase JSON shape `desktop/src/types.ts`'s `PairedDevice` expects —
    // a mismatch here (e.g. someone reverting the `rename_all`) would silently break the frontend
    // without any Rust-side error, since these command results aren't otherwise typechecked
    // against the TS side.
    #[test]
    fn paired_device_info_serializes_as_camel_case() {
        let device = PairedDevice { device_name: "Desktop A".to_string(), status: PairingStatus::Approved, first_seen_ms: 1, last_seen_ms: 2 };
        let info = to_info("dev-a".to_string(), device);
        assert_eq!(
            serde_json::to_string(&info).unwrap(),
            r#"{"deviceId":"dev-a","deviceName":"Desktop A","status":"approved","firstSeenMs":1,"lastSeenMs":2}"#
        );
    }
}
