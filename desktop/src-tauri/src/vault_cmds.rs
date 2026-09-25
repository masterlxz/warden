//! Tauri commands backing the "Vault" screen (P52 part 2; editing since P78) — the vault's
//! folder/file tree, word search, and reading/saving/deleting notes. Thin wrappers over
//! `warden_core::memory`'s `browse_files`/`read_note`/`save_note`/`delete_note`, the same calls the
//! hub makes for the web UI, so both screens share one set of path rules and one version check.
//! Split out of `lib.rs` the same way `sync_cmds.rs` already is.

use serde::Serialize;
use tauri::State;
use warden_core::memory::NoteConflict;

use crate::AppState;

/// Most lines `search_vault` returns — same cap as the hub's `SearchVault`.
const MAX_SEARCH_HITS: usize = 50;

/// A failed vault command. `conflict` is set when the note changed since it was opened, so the
/// screen can offer to reload instead of only showing the text.
#[derive(Debug, Serialize)]
pub struct VaultCmdError {
    message: String,
    conflict: bool,
}

impl From<anyhow::Error> for VaultCmdError {
    fn from(err: anyhow::Error) -> Self {
        Self { conflict: err.downcast_ref::<NoteConflict>().is_some(), message: format!("{err:#}") }
    }
}

impl From<String> for VaultCmdError {
    fn from(message: String) -> Self {
        Self { message, conflict: false }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultNotePayload {
    content: String,
    version: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultSearchHitPayload {
    path: String,
    line_number: usize,
    line: String,
}

fn vault(state: &State<'_, AppState>) -> Result<std::sync::Arc<warden_core::memory::Vault>, VaultCmdError> {
    let orchestrator = { state.orchestrator.lock().unwrap().clone() }?;
    Ok(orchestrator.vault().clone())
}

/// Every vault file a person browses — not the 3 fixed files at the root (the screen shows those
/// in their own section), `skills/` (the Skills screen) or dotfiles.
#[tauri::command]
pub fn list_vault_files(state: State<'_, AppState>) -> Result<Vec<String>, VaultCmdError> {
    Ok(vault(&state)?.browse_files()?)
}

/// Opens a note, with the version `save_vault_note`/`delete_vault_note` expect back.
#[tauri::command]
pub fn read_vault_note(state: State<'_, AppState>, path: String) -> Result<VaultNotePayload, VaultCmdError> {
    let note = vault(&state)?.read_note(&path)?;
    Ok(VaultNotePayload { content: note.content, version: note.version })
}

/// Saves a note and returns its new version. No `expected_version` creates it.
#[tauri::command]
pub fn save_vault_note(state: State<'_, AppState>, path: String, content: String, expected_version: Option<String>) -> Result<String, VaultCmdError> {
    Ok(vault(&state)?.save_note(&path, &content, expected_version.as_deref())?)
}

#[tauri::command]
pub fn delete_vault_note(state: State<'_, AppState>, path: String, expected_version: String) -> Result<(), VaultCmdError> {
    Ok(vault(&state)?.delete_note(&path, &expected_version)?)
}

#[tauri::command]
pub fn search_vault(state: State<'_, AppState>, query: String) -> Result<Vec<VaultSearchHitPayload>, VaultCmdError> {
    let hits = vault(&state)?.search(&query, MAX_SEARCH_HITS)?;
    Ok(hits.into_iter().map(|h| VaultSearchHitPayload { path: h.path, line_number: h.line_number, line: h.line }).collect())
}
