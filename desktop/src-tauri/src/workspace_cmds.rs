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

use serde::{Deserialize, Serialize};
use warden_bootstrap::HubPairingConfig;
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

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HubPairingConfigPayload {
    server_url: String,
    auth_key: String,
}

impl From<HubPairingConfig> for HubPairingConfigPayload {
    fn from(c: HubPairingConfig) -> Self {
        HubPairingConfigPayload { server_url: c.server_url, auth_key: c.auth_key }
    }
}

impl From<HubPairingConfigPayload> for HubPairingConfig {
    fn from(p: HubPairingConfigPayload) -> Self {
        HubPairingConfig { server_url: p.server_url, auth_key: p.auth_key }
    }
}

fn hub_pairing_config_path() -> Result<std::path::PathBuf, String> {
    warden_bootstrap::default_hub_pairing_config_path().ok_or_else(|| "could not determine the OS config directory".to_string())
}

/// What the Workspace screen's "Pareamento por QR" section pre-fills its Server URL/Auth key
/// fields from (Fase 9.7) — `None` until the operator has saved it at least once.
#[tauri::command]
pub fn get_hub_pairing_config() -> Result<Option<HubPairingConfigPayload>, String> {
    let path = hub_pairing_config_path()?;
    warden_bootstrap::load_hub_pairing_config(&path).map(|opt| opt.map(Into::into)).map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub fn save_hub_pairing_config(server_url: String, auth_key: String) -> Result<(), String> {
    let server_url = server_url.trim();
    let auth_key = auth_key.trim();
    if server_url.is_empty() || auth_key.is_empty() {
        return Err("Server URL e Auth key são obrigatórios".to_string());
    }
    let path = hub_pairing_config_path()?;
    let config = HubPairingConfig { server_url: server_url.to_string(), auth_key: auth_key.to_string() };
    warden_bootstrap::save_hub_pairing_config(&path, &config).map_err(|e| format!("{e:#}"))
}

/// Renders the pairing QR (Fase 9.7) from whatever was last saved via `save_hub_pairing_config` —
/// a new client (today, the mobile app's `ConnectionScreen`) scans this to skip typing the server
/// URL and auth key by hand. The payload is deliberately just those two fields: `device_id`/
/// `device_name` stay client-chosen (mobile already generates/persists its own), and approving the
/// device for `CallDeviceTool` routing (Fase 9.3/9.6) still happens separately, after it connects.
#[tauri::command]
pub fn hub_pairing_qr_svg() -> Result<String, String> {
    let path = hub_pairing_config_path()?;
    let config = warden_bootstrap::load_hub_pairing_config(&path)
        .map_err(|e| format!("{e:#}"))?
        .ok_or_else(|| "Preencha e salve o Server URL e a Auth key antes de gerar o QR".to_string())?;
    let payload = HubPairingConfigPayload::from(config);
    let json = serde_json::to_string(&payload).map_err(|e| format!("failed to serialize pairing payload: {e}"))?;
    crate::qr::render_qr_svg(&json)
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

    // Locks in the exact camelCase JSON shape embedded in the pairing QR (Fase 9.7) — this is
    // what `mobile/lib/services/hub_pairing_qr.dart::parseHubPairingQr` decodes on the other end,
    // so the two sides must agree on field names without either one importing the other's types.
    #[test]
    fn hub_pairing_config_payload_serializes_as_camel_case() {
        let payload = HubPairingConfigPayload::from(HubPairingConfig {
            server_url: "ws://192.168.1.10:7420".to_string(),
            auth_key: "secret".to_string(),
        });
        assert_eq!(serde_json::to_string(&payload).unwrap(), r#"{"serverUrl":"ws://192.168.1.10:7420","authKey":"secret"}"#);
    }
}
