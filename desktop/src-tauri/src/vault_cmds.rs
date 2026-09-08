//! Tauri commands backing the "Vault" screen (P52, part 2) — a read-only browser over the vault's
//! folder/file tree, with the 3 "fixed" files (P52 part 1, `warden_core::memory::FIXED_VAULT_FILES`)
//! filtered out here so the frontend can show them in their own highlighted section instead of
//! mixed into the rest of the tree. Split out of `lib.rs` the same way `sync_cmds.rs` already is.

use std::path::Path;

use tauri::State;
use warden_core::memory::FIXED_VAULT_FILES;

use crate::AppState;

/// Every vault file, relative to its root, excluding the 3 fixed files at vault root (the
/// frontend renders those in their own section) — a same-named file nested under a subdirectory
/// is a regular note and stays in this list, mirroring `warden_core::memory`'s own
/// root-only exclusion for `search`/`search_semantic`.
#[tauri::command]
pub fn list_vault_files(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    let orchestrator = { state.orchestrator.lock().unwrap().clone() }?;
    let files = orchestrator.vault().list_all_files().map_err(|e| format!("{e:#}"))?;

    Ok(files
        .into_iter()
        .filter(|path| path.parent() != Some(Path::new("")) || !FIXED_VAULT_FILES.contains(&path.to_string_lossy().as_ref()))
        .map(|path| path.to_string_lossy().to_string())
        .collect())
}

/// Reads one vault file's raw content by relative path — used for both the 3 fixed files and any
/// entry from `list_vault_files`'s tree, so the frontend never needs to branch on which kind of
/// file it's showing. Paths only ever come from what this same vault already listed, never typed
/// by a user, so no extra path-traversal guard is needed beyond what `Vault::read` already does.
#[tauri::command]
pub fn read_vault_file(state: State<'_, AppState>, relative_path: String) -> Result<String, String> {
    let orchestrator = { state.orchestrator.lock().unwrap().clone() }?;
    orchestrator.vault().read(&relative_path).map_err(|e| format!("{e:#}"))
}
