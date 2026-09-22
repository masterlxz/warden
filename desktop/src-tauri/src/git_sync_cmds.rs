//! Tauri commands backing the "Sync" screen's Git section (P63/P71) — manual push/pull of the
//! vault + `config.toml` via a self-hosted/remote git repo, the fully-automatable alternative to
//! the Arweave/TruthID backend in `sync_cmds.rs` (that one always blocks push on a real phone
//! approval, by design — this one doesn't, see `sync_cmds::spawn_auto_sync`). Stateless, same
//! precedent as `workspace_cmds.rs`: no `AppState` needed, every command reads `config.toml` fresh
//! and builds its own `GitSyncEngine` per call.

use std::path::PathBuf;

use serde::Serialize;
use warden_bootstrap::{default_config_path, load_config_from_path, resolve_vault_path, GitSyncConfig, Overrides};
use warden_sync::{GitPullOutcome, GitPushOutcome, GitSyncEngine};

fn fresh_config() -> Result<(PathBuf, warden_bootstrap::FileConfig), String> {
    let path = default_config_path().ok_or_else(|| "could not determine the OS config directory".to_string())?;
    let config = load_config_from_path(&path, false).map_err(|e| format!("{e:#}"))?;
    Ok((path, config))
}

/// Whether `[git_sync]` is configured — the Sync screen uses this to decide between showing the
/// Push/Pull buttons or a hint pointing at Settings.
#[tauri::command]
pub fn git_sync_configured() -> Result<bool, String> {
    let (_, config) = fresh_config()?;
    Ok(config.git_sync.is_some())
}

/// Builds a `GitSyncEngine` — takes `secrets_path`/`manifest_path`/`git_repo_path` as explicit
/// parameters (rather than resolving `warden_sync::paths::default_*` internally) so a caller that
/// already has them at hand (`spawn_auto_sync`, whose args come straight from `lib.rs::run()`)
/// never re-derives them, and so tests can point this at a temp dir instead of this machine's real
/// `~/.config/warden/{sync_secrets.json,sync_manifest.json,git-sync-repo}` — pointing a test at the
/// real `git_repo_path` in particular would leave a stale local git clone behind on this machine's
/// actual config dir, and reusing it across separate test runs is exactly what caused a real
/// `git checkout --orphan main` failure ("a branch named 'main' already exists") the first time
/// this was tried with `git_repo_path` still resolved internally. Reused by this module's own
/// commands and by `sync_cmds::spawn_auto_sync`'s git branch, so the two never drift apart on how
/// the engine is assembled.
pub(crate) fn build_git_sync_engine(
    vault_path: PathBuf,
    config_path: PathBuf,
    secrets_path: PathBuf,
    manifest_path: PathBuf,
    git_repo_path: PathBuf,
    git_sync: &GitSyncConfig,
) -> GitSyncEngine {
    GitSyncEngine::new(vault_path, config_path, secrets_path, manifest_path, git_repo_path, git_sync.remote_url.clone(), git_sync.token.clone())
}

fn build_engine_from_fresh_config() -> Result<GitSyncEngine, String> {
    let (config_path, config) = fresh_config()?;
    let vault_path = resolve_vault_path(&Overrides::default(), &config, crate::desktop_default_vault_path());
    let git_sync = config.git_sync.ok_or_else(|| "configure a URL e o token do git sync em Settings antes de usar".to_string())?;
    let secrets_path = warden_sync::paths::default_sync_secrets_path().unwrap_or_else(|| PathBuf::from("sync_secrets.json"));
    let manifest_path = warden_sync::paths::default_sync_manifest_path().unwrap_or_else(|| PathBuf::from("sync_manifest.json"));
    let git_repo_path = warden_sync::paths::default_git_sync_repo_path().unwrap_or_else(|| PathBuf::from("git-sync-repo"));
    Ok(build_git_sync_engine(vault_path, config_path, secrets_path, manifest_path, git_repo_path, &git_sync))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitPushResultPayload {
    commit_sha: String,
    files_changed: usize,
    config_changed: bool,
}

impl From<GitPushOutcome> for GitPushResultPayload {
    fn from(o: GitPushOutcome) -> Self {
        Self { commit_sha: o.commit_sha, files_changed: o.files_changed, config_changed: o.config_changed }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitPullResultPayload {
    commits_applied: usize,
    files_written: usize,
    files_deleted: usize,
    files_ignored: usize,
    config_updated: bool,
    warnings: Vec<String>,
}

impl From<GitPullOutcome> for GitPullResultPayload {
    fn from(o: GitPullOutcome) -> Self {
        Self {
            commits_applied: o.commits_applied,
            files_written: o.files_written,
            files_deleted: o.files_deleted,
            files_ignored: o.files_ignored,
            config_updated: o.config_updated,
            warnings: o.warnings,
        }
    }
}

#[tauri::command]
pub async fn git_sync_push() -> Result<Option<GitPushResultPayload>, String> {
    let engine = build_engine_from_fresh_config()?;
    engine.push().await.map(|opt| opt.map(Into::into)).map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub async fn git_sync_pull() -> Result<GitPullResultPayload, String> {
    let engine = build_engine_from_fresh_config()?;
    engine.pull().await.map(Into::into).map_err(|e| format!("{e:#}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Locks in the exact camelCase JSON shape `desktop/src/types.ts` expects.
    #[test]
    fn git_push_result_payload_serializes_as_camel_case() {
        let outcome = GitPushOutcome { commit_sha: "abc123".to_string(), files_changed: 2, config_changed: true };
        assert_eq!(
            serde_json::to_string(&GitPushResultPayload::from(outcome)).unwrap(),
            r#"{"commitSha":"abc123","filesChanged":2,"configChanged":true}"#
        );
    }

    #[test]
    fn git_pull_result_payload_serializes_as_camel_case() {
        let outcome = GitPullOutcome {
            commits_applied: 3,
            files_written: 2,
            files_deleted: 1,
            files_ignored: 1,
            config_updated: false,
            warnings: vec!["a".to_string()],
        };
        assert_eq!(
            serde_json::to_string(&GitPullResultPayload::from(outcome)).unwrap(),
            r#"{"commitsApplied":3,"filesWritten":2,"filesDeleted":1,"filesIgnored":1,"configUpdated":false,"warnings":["a"]}"#
        );
    }
}
