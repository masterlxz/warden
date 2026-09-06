use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Ports both the phone (`RemoteSignerLanServer`) and the requester sweep — the QR payload
/// carries no IP/port, so both sides agree on this fixed block out of band (mirrors
/// `mobile/lib/services/remote_signer_lan_server.dart` / `sdk/dart/lib/src/internal/lan_sweep_client.dart`).
pub const LAN_PORTS: [u16; 5] = [48050, 48051, 48052, 48053, 48054];

/// Same default as the Dart SDK's `_defaultTimeout`.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(180);

/// The JSON payload rendered as a QR code for the `truthid-pin` action. Field names/casing and
/// value encodings mirror `sdk/dart/lib/src/requester.dart:263-270` exactly — the phone's QR
/// parser expects this shape verbatim.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QrPayload {
    pub action: &'static str,
    pub v: u8,
    pub session_id: String,
    pub ephemeral_pub_key: String,
    pub expires_at: i64,
    pub app_name: String,
}

/// What the phone reports back after the user approves or rejects the pin. Mirrors
/// `sdk/dart/lib/src/requester.dart:85-101` field for field.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PinResult {
    pub status: String,
    pub cid: Option<String>,
    pub content_hash: Option<String>,
    pub providers_ok: Option<Vec<String>>,
    pub providers_failed: Option<Vec<String>>,
    pub error: Option<String>,
}
