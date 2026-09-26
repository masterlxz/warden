//! Tauri commands backing the "Sync" screen (P37) — manual Send/Pull of the vault + `config.toml`
//! via Arweave (paid by the TruthID app's `pin()`), plus the code+LAN pairing flow that spreads
//! the vault-encryption key to another Warden install. Split out of `lib.rs` (already 500+ lines
//! before this) the same way `recording.rs` keeps the mic-capture logic out of it — `AppState`'s
//! fields stay accessible here since Rust's privacy rules extend a private item's visibility to
//! every descendant module of the one that declares it, not just that exact module.

use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use warden_bootstrap::auto_sync::{PulledSummary, PushedSummary, SyncRunner, AUTO_SYNC_INTERVAL};
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
    syncignore_pattern_count: usize,
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
            syncignore_pattern_count: s.syncignore_pattern_count,
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
    let qr_svg = crate::qr::render_qr_svg(&qr_json)?;
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
    files_ignored: usize,
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
        files_ignored: outcome.files_ignored,
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

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AutoSyncPulledPayload {
    tx_id: Option<String>,
    files_written: usize,
    files_deleted: usize,
    config_updated: bool,
}

impl From<&PulledSummary> for AutoSyncPulledPayload {
    fn from(p: &PulledSummary) -> Self {
        Self { tx_id: p.tx_id.clone(), files_written: p.files_written, files_deleted: p.files_deleted, config_updated: p.config_updated }
    }
}

/// Emitted only for the git backend (P71 follow-up) — Arweave never auto-pushes, see
/// `warden_bootstrap::auto_sync` for why.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AutoSyncPushedPayload {
    commit_sha: String,
    files_changed: usize,
    config_changed: bool,
}

impl From<&PushedSummary> for AutoSyncPushedPayload {
    fn from(p: &PushedSummary) -> Self {
        Self { commit_sha: p.commit_sha.clone(), files_changed: p.files_changed, config_changed: p.config_changed }
    }
}

/// P71 — pulls (and, for the git backend, pushes) automatically every few minutes for as long as
/// the app is open, through the same `SyncRunner` the embedded hub answers the web's Sync screen
/// with (P61), so a round from here and a "sync now" from the web never overlap. The rounds
/// themselves live in `warden_bootstrap::auto_sync`, shared with the standalone hub. A round that
/// brings another device's `config.toml` rebuilds the orchestrator, as a Settings save does.
pub fn spawn_auto_sync(app: AppHandle, runner: Arc<SyncRunner>) {
    tauri::async_runtime::spawn(runner.run_loop(AUTO_SYNC_INTERVAL, move |report| {
        if let Some(pulled) = &report.pulled {
            let _ = app.emit("auto-sync-pulled", AutoSyncPulledPayload::from(pulled));
        }
        if let Some(pushed) = &report.pushed {
            let _ = app.emit("auto-sync-pushed", AutoSyncPushedPayload::from(pushed));
        }
        let reload = report.config_updated().then(|| app.clone());
        async move {
            if let Some(app) = reload {
                crate::reload_orchestrator(&app.state::<AppState>()).await;
            }
        }
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    // Locks in the exact camelCase JSON shapes `desktop/src/types.ts` expects. The rounds
    // themselves are tested in `warden_bootstrap::auto_sync`.
    #[test]
    fn auto_sync_payloads_serialize_as_camel_case() {
        let pulled = PulledSummary { tx_id: Some("tx-1".to_string()), files_written: 2, files_deleted: 1, config_updated: true };
        assert_eq!(
            serde_json::to_string(&AutoSyncPulledPayload::from(&pulled)).unwrap(),
            r#"{"txId":"tx-1","filesWritten":2,"filesDeleted":1,"configUpdated":true}"#
        );
        let pushed = PushedSummary { commit_sha: "abc".to_string(), files_changed: 3, config_changed: false };
        assert_eq!(serde_json::to_string(&AutoSyncPushedPayload::from(&pushed)).unwrap(), r#"{"commitSha":"abc","filesChanged":3,"configChanged":false}"#);
    }
}
