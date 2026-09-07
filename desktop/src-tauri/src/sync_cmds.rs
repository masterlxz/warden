//! Tauri commands backing the "Sync" screen (P37) — manual Send/Pull of the vault + `config.toml`
//! via Arweave (paid by the TruthID app's `pin()`), plus the code+LAN pairing flow that spreads
//! the vault-encryption key to another Warden install. Split out of `lib.rs` (already 500+ lines
//! before this) the same way `recording.rs` keeps the mic-capture logic out of it — `AppState`'s
//! fields stay accessible here since Rust's privacy rules extend a private item's visibility to
//! every descendant module of the one that declares it, not just that exact module.

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use warden_sync::SyncStatus;

use crate::AppState;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatusPayload {
    paired: bool,
    device_id: Option<String>,
    owner_address: Option<String>,
    last_tx_id: Option<String>,
    last_synced_at_ms: Option<i64>,
    pending_vault_changes: usize,
    pending_config_changed: bool,
}

impl From<SyncStatus> for SyncStatusPayload {
    fn from(s: SyncStatus) -> Self {
        Self {
            paired: s.paired,
            device_id: s.device_id,
            owner_address: s.owner_address,
            last_tx_id: s.last_tx_id,
            last_synced_at_ms: s.last_synced_at_ms,
            pending_vault_changes: s.pending_vault_changes,
            pending_config_changed: s.pending_config_changed,
        }
    }
}

#[tauri::command]
pub fn sync_status(state: State<'_, AppState>) -> Result<SyncStatusPayload, String> {
    state.sync.status().map(Into::into).map_err(|e| format!("{e:#}"))
}

/// Generates a fresh vault key on this device — the first device in a sync group calls this;
/// every other one instead pairs (`pairing_join`) with an already-initialized device.
#[tauri::command]
pub fn sync_init(state: State<'_, AppState>) -> Result<(), String> {
    state.sync.init_fresh().map_err(|e| format!("{e:#}"))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PushBeginPayload {
    qr_svg: String,
    files_changed: usize,
    config_changed: bool,
}

/// Step 1 of Send: computes the diff, builds the QR for the TruthID app to scan, and stashes the
/// in-flight `PendingPin` in `AppState` for `sync_push_await` to pick up. Showing the QR and
/// blocking on the phone are deliberately separate IPC calls, so the frontend can render the QR
/// before the (potentially minutes-long, human-paced) wait begins.
#[tauri::command]
pub fn sync_push_begin(state: State<'_, AppState>) -> Result<PushBeginPayload, String> {
    let begin = state
        .sync
        .begin_push()
        .map_err(|e| format!("{e:#}"))?
        .ok_or_else(|| "Nada para enviar — vault e config já batem com o último Enviar".to_string())?;

    let qr_json = begin.pending.qr_payload_json().map_err(|e| format!("{e:#}"))?;
    let qr_svg = render_qr_svg(&qr_json)?;
    let files_changed = begin.bundle.vault_files.len() + begin.bundle.deleted_vault_files.len();
    let config_changed = begin.bundle.config_toml.is_some() || begin.bundle.config_deleted;

    *state.pending_push.lock().unwrap() = Some(begin);
    Ok(PushBeginPayload { qr_svg, files_changed, config_changed })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PushResultPayload {
    tx_id: String,
    files_changed: usize,
    config_changed: bool,
}

/// Step 2 of Send: waits for the TruthID phone to pin the bundle prepared by `sync_push_begin`.
#[tauri::command]
pub async fn sync_push_await(state: State<'_, AppState>) -> Result<PushResultPayload, String> {
    let begin = state
        .pending_push
        .lock()
        .unwrap()
        .take()
        .ok_or_else(|| "Nenhum envio em andamento — chame sync_push_begin primeiro".to_string())?;

    let outcome = state.sync.finish_push(begin).await.map_err(|e| format!("{e:#}"))?;
    Ok(PushResultPayload { tx_id: outcome.tx_id, files_changed: outcome.files_changed, config_changed: outcome.config_changed })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PullResultPayload {
    tx_id: Option<String>,
    files_written: usize,
    files_deleted: usize,
    config_updated: bool,
    warnings: Vec<String>,
}

#[tauri::command]
pub async fn sync_pull(state: State<'_, AppState>) -> Result<PullResultPayload, String> {
    let outcome = state.sync.pull().await.map_err(|e| format!("{e:#}"))?;
    Ok(PullResultPayload {
        tx_id: outcome.tx_id,
        files_written: outcome.files_written,
        files_deleted: outcome.files_deleted,
        config_updated: outcome.config_updated,
        warnings: outcome.warnings,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingStartPayload {
    code: String,
}

/// Starts showing a pairing code and, in the background, waits for another device to join —
/// emits `pairing-completed`/`pairing-failed` on `app` when that resolves, since the wait can take
/// up to `PAIRING_TIMEOUT` (5 minutes) and must not block this IPC call.
#[tauri::command]
pub async fn pairing_start(app: AppHandle, state: State<'_, AppState>) -> Result<PairingStartPayload, String> {
    let host = state.sync.pairing_host().await.map_err(|e| format!("{e:#}"))?;
    let code = host.code().to_string();

    tauri::async_runtime::spawn(async move {
        match host.wait_for_join().await {
            Ok(()) => {
                let _ = app.emit("pairing-completed", ());
            }
            Err(err) => {
                let _ = app.emit("pairing-failed", format!("{err:#}"));
            }
        }
    });

    Ok(PairingStartPayload { code })
}

/// Sweeps the LAN for a device showing `code` and adopts the vault key it hands over.
#[tauri::command]
pub async fn pairing_join(state: State<'_, AppState>, code: String) -> Result<(), String> {
    state.sync.pairing_join(&code).await.map_err(|e| format!("{e:#}"))
}

fn render_qr_svg(data: &str) -> Result<String, String> {
    let code = qrcode::QrCode::new(data.as_bytes()).map_err(|e| format!("failed to build QR code: {e}"))?;
    Ok(code
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(256, 256)
        .dark_color(qrcode::render::svg::Color("#000000"))
        .light_color(qrcode::render::svg::Color("#ffffff"))
        .build())
}
