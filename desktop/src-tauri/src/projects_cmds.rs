//! Tauri commands backing the "Projects" screen (P103) — list/create/edit/remove the projects stored under `projects/`
//! in the vault (`warden_core::project`). Split out of `lib.rs` like `skills_cmds.rs`. A project's files are ordinary
//! notes of the vault, so they are read and written with the Vault screen's commands; nothing here touches them.

use serde::{Deserialize, Serialize};
use tauri::State;
use warden_core::project::{Project, ProjectStore};

use crate::AppState;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectPayload {
    id: String,
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    instructions: String,
    /// A code project's working folder (P103 b); empty or absent for an ordinary project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    workdir: Option<String>,
}

impl From<Project> for ProjectPayload {
    fn from(project: Project) -> Self {
        Self { id: project.id, name: project.name, description: project.description, instructions: project.instructions, workdir: project.workdir }
    }
}

impl From<ProjectPayload> for Project {
    fn from(payload: ProjectPayload) -> Self {
        Self { id: payload.id.trim().to_string(), name: payload.name.trim().to_string(), description: payload.description, instructions: payload.instructions, workdir: payload.workdir.map(|dir| dir.trim().to_string()).filter(|dir| !dir.is_empty()) }
    }
}

fn store(state: &State<'_, AppState>) -> Result<ProjectStore, String> {
    let orchestrator = { state.orchestrator.lock().unwrap().clone() }?;
    Ok(ProjectStore::new(orchestrator.vault().clone()))
}

/// `overwrite: false` is "create" (refuses an id already taken, so the New form can't silently replace another
/// project); `true` is "edit" (the form locks the id then, since conversations point at it).
fn save_inner(store: &ProjectStore, project: Project, overwrite: bool) -> Result<(), String> {
    project.validate().map_err(|e| format!("{e:#}"))?;
    if !overwrite && store.exists(&project.id) {
        return Err(format!("a project '{}' already exists", project.id));
    }
    store.save(&project).map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub fn list_projects(state: State<'_, AppState>) -> Result<Vec<ProjectPayload>, String> {
    Ok(store(&state)?.list().into_iter().map(Into::into).collect())
}

#[tauri::command]
pub fn save_project(state: State<'_, AppState>, project: ProjectPayload, overwrite: bool) -> Result<(), String> {
    save_inner(&store(&state)?, project.into(), overwrite)
}

/// Removes only the project's `PROJECT.md`: its files stay in the vault, and the conversations that were in it go on as
/// conversations without a project.
#[tauri::command]
pub fn delete_project(state: State<'_, AppState>, id: String) -> Result<(), String> {
    store(&state)?.delete(id.trim()).map_err(|e| format!("{e:#}"))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use warden_core::memory::Vault;

    use super::*;

    fn temp_store() -> ProjectStore {
        ProjectStore::new(Arc::new(Vault::new(std::env::temp_dir().join(format!(
            "warden-desktop-projects-test-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        )))))
    }

    fn payload(id: &str) -> ProjectPayload {
        ProjectPayload { id: id.into(), name: "Tax return".into(), description: "d".into(), instructions: "Be brief.".into(), workdir: None }
    }

    #[test]
    fn create_refuses_a_taken_id_and_edit_overwrites_keeping_the_files() {
        let store = temp_store();
        save_inner(&store, payload("tax").into(), false).unwrap();
        assert!(save_inner(&store, payload("tax").into(), false).unwrap_err().contains("already exists"));

        store.scope("tax").unwrap().write("jan.md", "receipts").unwrap();
        save_inner(&store, ProjectPayload { name: "Tax 2026".into(), ..payload("tax") }.into(), true).unwrap();
        assert_eq!(store.get("tax").unwrap().name, "Tax 2026");
        assert_eq!(store.scope("tax").unwrap().read("jan.md").unwrap(), "receipts");
    }

    #[test]
    fn a_bad_id_or_name_is_refused_and_the_payload_is_trimmed() {
        let store = temp_store();
        for bad in [payload("../x"), payload(""), ProjectPayload { name: "  ".into(), ..payload("ok") }] {
            assert!(save_inner(&store, bad.into(), false).is_err());
        }
        save_inner(&store, ProjectPayload { id: " tax ".into(), name: " Tax ".into(), ..payload("x") }.into(), false).unwrap();
        assert_eq!(store.list().iter().map(|p| (p.id.as_str(), p.name.as_str())).collect::<Vec<_>>(), [("tax", "Tax")]);
    }
}
