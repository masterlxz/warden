//! Tauri commands backing the "Sync" screen (P37) — manual Send/Pull of the vault + `config.toml`
//! via Arweave (paid by the TruthID app's `pin()`), plus the code+LAN pairing flow that spreads
//! the vault-encryption key to another Warden install. Split out of `lib.rs` (already 500+ lines
//! before this) the same way `recording.rs` keeps the mic-capture logic out of it — `AppState`'s
//! fields stay accessible here since Rust's privacy rules extend a private item's visibility to
//! every descendant module of the one that declares it, not just that exact module.

use std::path::PathBuf;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use warden_bootstrap::load_config_from_path;
use warden_sync::{GitPullOutcome, GitPushOutcome, PullOutcome, SyncEngine, SyncStatus};

use crate::git_sync_cmds::build_git_sync_engine;
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

const AUTO_SYNC_INTERVAL: Duration = Duration::from_secs(5 * 60);

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AutoSyncPulledPayload {
    tx_id: Option<String>,
    files_written: usize,
    files_deleted: usize,
    config_updated: bool,
}

impl From<&PullOutcome> for AutoSyncPulledPayload {
    fn from(o: &PullOutcome) -> Self {
        Self { tx_id: o.tx_id.clone(), files_written: o.files_written, files_deleted: o.files_deleted, config_updated: o.config_updated }
    }
}

impl From<&GitPullOutcome> for AutoSyncPulledPayload {
    fn from(o: &GitPullOutcome) -> Self {
        Self { tx_id: None, files_written: o.files_written, files_deleted: o.files_deleted, config_updated: o.config_updated }
    }
}

/// Emitted only for the git backend (P71 follow-up) — Arweave never auto-pushes, see this module's
/// doc comment for why.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AutoSyncPushedPayload {
    commit_sha: String,
    files_changed: usize,
    config_changed: bool,
}

impl From<&GitPushOutcome> for AutoSyncPushedPayload {
    fn from(o: &GitPushOutcome) -> Self {
        Self { commit_sha: o.commit_sha.clone(), files_changed: o.files_changed, config_changed: o.config_changed }
    }
}

/// P71 — pulls (and, for the git backend, pushes) automatically every few minutes instead of the
/// operator having to remember to click Pull/Push. `tokio::time::interval`'s first tick resolves
/// immediately, so this also covers "just opened the app after being away" without a separate
/// startup call. Runs for the lifetime of the app — there's no stop handle, matching the
/// "always on while the app is open" scope of this slice.
///
/// Rereads `config.toml` fresh every tick (cheap — same posture as every other command in this
/// file) to decide which backend is active: Arweave (`SyncEngine`) and git (`GitSyncEngine`)
/// share the same `secrets_path`/`manifest_path` on disk (see `git_sync_cmds.rs`'s module docs) —
/// they're alternatives, not additive, so only one branch ever runs per tick. **git configured**:
/// pulls then pushes via `GitSyncEngine` — pull first so a stale local `main` never turns into an
/// avoidable non-fast-forward rejection. **git not configured**: exactly today's Arweave-only
/// auto-pull. Push stays entirely manual for Arweave regardless: `finish_push` blocks on a real
/// TruthID phone approval, an intentional safety gate this loop never bypasses.
pub fn spawn_auto_sync(app: AppHandle, vault_path: PathBuf, config_path: PathBuf, secrets_path: PathBuf, manifest_path: PathBuf) {
    tauri::async_runtime::spawn(async move {
        let mut ticker = tokio::time::interval(AUTO_SYNC_INTERVAL);
        loop {
            ticker.tick().await;

            let git_sync = load_config_from_path(&config_path, false).ok().and_then(|c| c.git_sync);
            match git_sync {
                Some(git_sync) => {
                    if !secrets_path.exists() {
                        continue; // sync never set up on this device — nothing to do, not an error
                    }
                    let git_repo_path = warden_sync::paths::default_git_sync_repo_path().unwrap_or_else(|| PathBuf::from("git-sync-repo"));
                    let engine = build_git_sync_engine(
                        vault_path.clone(),
                        config_path.clone(),
                        secrets_path.clone(),
                        manifest_path.clone(),
                        git_repo_path,
                        &git_sync,
                    );
                    match engine.pull().await {
                        Ok(outcome) if outcome.files_written > 0 || outcome.files_deleted > 0 || outcome.config_updated => {
                            let _ = app.emit("auto-sync-pulled", AutoSyncPulledPayload::from(&outcome));
                        }
                        Ok(_) => {}
                        Err(err) => {
                            eprintln!("desktop: auto-sync (git) pull failed: {err:#}");
                            continue; // don't attempt the push against a possibly-stale local state
                        }
                    }
                    match engine.push().await {
                        Ok(Some(outcome)) => {
                            let _ = app.emit("auto-sync-pushed", AutoSyncPushedPayload::from(&outcome));
                        }
                        Ok(None) => {} // nothing local to send — nothing worth telling the UI about
                        Err(err) => eprintln!("desktop: auto-sync (git) push failed: {err:#}"),
                    }
                }
                None => {
                    let engine = SyncEngine::new(vault_path.clone(), config_path.clone(), secrets_path.clone(), manifest_path.clone());
                    if !engine.is_initialized() {
                        continue; // sync never set up on this device — nothing to do, not an error
                    }
                    match engine.pull().await {
                        Ok(outcome) if outcome.files_written > 0 || outcome.files_deleted > 0 || outcome.config_updated => {
                            let _ = app.emit("auto-sync-pulled", AutoSyncPulledPayload::from(&outcome));
                        }
                        Ok(_) => {} // already up to date — nothing worth telling the UI about
                        Err(err) => eprintln!("desktop: auto-pull failed: {err:#}"),
                    }
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use warden_bootstrap::GitSyncConfig;

    fn temp_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "desktop-auto-sync-{name}-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ))
    }

    fn bare_remote(suffix: &str) -> PathBuf {
        let path = temp_dir(&format!("remote-{suffix}"));
        std::process::Command::new("git").args(["init", "--bare", &path.to_string_lossy()]).status().unwrap();
        path
    }

    // Locks in the exact camelCase JSON shape `desktop/src/types.ts` expects.
    #[test]
    fn auto_sync_pulled_payload_serializes_as_camel_case() {
        let outcome = PullOutcome { tx_id: Some("tx-1".to_string()), files_written: 2, files_deleted: 1, config_updated: true, warnings: vec![] };
        assert_eq!(
            serde_json::to_string(&AutoSyncPulledPayload::from(&outcome)).unwrap(),
            r#"{"txId":"tx-1","filesWritten":2,"filesDeleted":1,"configUpdated":true}"#
        );
    }

    /// Not mocked: a real `SyncEngine` that was never `init_fresh()`'d, pointed at a fake Arweave
    /// gateway that would fail loudly if ever contacted (proves the `is_initialized()` gate skips
    /// *before* any network call, not just that no error surfaced). Runs `spawn_auto_pull`'s inner
    /// loop body directly (not through the real 5-minute ticker) since the point under test is the
    /// gate itself, not the timer.
    #[tokio::test]
    async fn auto_pull_skips_silently_when_sync_was_never_set_up() {
        let temp_dir = std::env::temp_dir().join(format!(
            "desktop-auto-pull-uninitialized-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        let engine = SyncEngine::new(temp_dir.join("vault"), temp_dir.join("config.toml"), temp_dir.join("secrets.json"), temp_dir.join("manifest.json"))
            .with_arweave_client(warden_sync::ArweaveClient::new("http://127.0.0.1:1/graphql", "http://127.0.0.1:1"));

        assert!(!engine.is_initialized());
        // Mirrors `spawn_auto_pull`'s loop body for one tick: the gate must return before `pull()`
        // ever runs, or this would hang/error trying to reach the unroutable fake address.
        if engine.is_initialized() {
            engine.pull().await.unwrap();
        }
    }

    /// Not mocked: a real, already-*paired* `SyncEngine` (secrets + a manifest with a real
    /// `owner_address` written straight to disk — `pull()` bails early on an unpaired device
    /// regardless of `is_initialized()`, so `init_fresh()` alone wouldn't exercise this path)
    /// pulling against a real local HTTP server standing in for the Arweave gateway — same
    /// fake-gateway idiom `crates/warden-sync/tests/fake_arweave_gateway.rs` already uses, kept
    /// minimal here (answers "nothing published yet") since what's under test is that the auto-
    /// pull loop's real call reaches the network and completes without error, not the full
    /// push→pull content round trip (already covered end-to-end by `warden-sync`'s own
    /// `engine_lifecycle.rs`).
    #[tokio::test]
    async fn auto_pull_reaches_a_real_gateway_and_completes_when_already_paired() {
        let router = axum::Router::new().route(
            "/graphql",
            axum::routing::post(|| async { axum::Json(serde_json::json!({ "data": { "transactions": { "edges": [] } } })) }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let base_url = format!("http://{addr}");

        let temp_dir = std::env::temp_dir().join(format!(
            "desktop-auto-pull-paired-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        let secrets_path = temp_dir.join("secrets.json");
        let manifest_path = temp_dir.join("manifest.json");
        warden_sync::manifest::save_secrets(&secrets_path, &warden_sync::manifest::generate_secrets()).unwrap();
        warden_sync::manifest::save_manifest(
            &manifest_path,
            &warden_sync::SyncManifest { owner_address: Some("test-owner".to_string()), ..Default::default() },
        )
        .unwrap();

        let engine = SyncEngine::new(temp_dir.join("vault"), temp_dir.join("config.toml"), secrets_path, manifest_path)
            .with_arweave_client(warden_sync::ArweaveClient::new(format!("{base_url}/graphql"), base_url));

        assert!(engine.is_initialized());
        let outcome = engine.pull().await.unwrap();
        assert_eq!(outcome.files_written, 0);
    }

    /// Not mocked: mirrors `spawn_auto_sync`'s git branch for one tick — same "gate before any
    /// network call" proof as `auto_pull_skips_silently_when_sync_was_never_set_up`, but for the
    /// git backend. `remote_url` points at an address that would fail loudly if `git` ever shelled
    /// out to it, so this only passes if the `secrets_path.exists()` gate really does return first.
    #[tokio::test]
    async fn git_branch_skips_silently_when_sync_was_never_set_up() {
        let dir = temp_dir("git-uninitialized");
        let secrets_path = dir.join("secrets.json");
        assert!(!secrets_path.exists());

        let git_sync = GitSyncConfig { remote_url: "https://127.0.0.1:1/nope.git".to_string(), token: String::new() };
        if secrets_path.exists() {
            let engine = build_git_sync_engine(
                dir.join("vault"),
                dir.join("config.toml"),
                secrets_path.clone(),
                dir.join("manifest.json"),
                dir.join("git-sync-repo"),
                &git_sync,
            );
            engine.pull().await.unwrap();
        }
    }

    /// Not mocked: mirrors `spawn_auto_sync`'s git branch for one tick against a real local bare
    /// git repo (same hermetic-but-real idiom `crates/warden-sync/src/git.rs`'s own tests use) —
    /// proves the exact helper the production loop calls (`build_git_sync_engine`) really can
    /// pull-then-push a real change, not just that the wiring compiles.
    #[tokio::test]
    async fn git_branch_pull_then_push_reaches_a_real_bare_repo() {
        let remote = bare_remote("tick");
        let dir = temp_dir("git-tick");
        let secrets_path = dir.join("secrets.json");
        warden_sync::manifest::save_secrets(&secrets_path, &warden_sync::manifest::generate_secrets()).unwrap();

        let git_sync = GitSyncConfig { remote_url: remote.to_string_lossy().to_string(), token: String::new() };
        let vault_path = dir.join("vault");
        let config_path = dir.join("config.toml");
        let manifest_path = dir.join("manifest.json");
        let git_repo_path = dir.join("git-sync-repo");

        assert!(secrets_path.exists());
        let engine = build_git_sync_engine(vault_path.clone(), config_path.clone(), secrets_path.clone(), manifest_path, git_repo_path, &git_sync);

        // Pull first, same order `spawn_auto_sync` uses — the remote is empty, so this is a no-op.
        let pull_outcome = engine.pull().await.unwrap();
        assert_eq!(pull_outcome.commits_applied, 0);

        std::fs::create_dir_all(&vault_path).unwrap();
        std::fs::write(vault_path.join("a.md"), "hello").unwrap();
        let push_outcome = engine.push().await.unwrap();
        assert_eq!(push_outcome.unwrap().files_changed, 1);
    }
}
