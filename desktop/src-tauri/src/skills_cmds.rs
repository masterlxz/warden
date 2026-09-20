//! Tauri commands backing the "Skills" screen (P16) — list/create/edit/delete the skills stored
//! under `skills/` in the vault (`warden_core::skill`), plus a "describe it and the AI drafts it"
//! command. Split out of `lib.rs` the same way `vault_cmds.rs` is. Skills live in the vault, not
//! `config.toml`, so nothing here touches `save_settings`.

use serde::{Deserialize, Serialize};
use tauri::State;
use warden_bootstrap::{build_model_provider, default_config_path, load_config_from_path};
use warden_core::skill::{Skill, SkillStore};

use crate::AppState;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SkillPayload {
    name: String,
    description: String,
    body: String,
    /// Agent ids the skill is restricted to (P72 c); empty = every agent. `default` so a caller
    /// that predates the field (or the mobile-style payload without it) still deserializes.
    #[serde(default)]
    agents: Vec<String>,
}

impl From<Skill> for SkillPayload {
    fn from(skill: Skill) -> Self {
        Self { name: skill.name, description: skill.description, body: skill.body, agents: skill.agents }
    }
}

impl From<SkillPayload> for Skill {
    fn from(payload: SkillPayload) -> Self {
        Self { name: payload.name.trim().to_string(), description: payload.description, body: payload.body, agents: payload.agents }
    }
}

fn store(state: &State<'_, AppState>) -> Result<SkillStore, String> {
    let orchestrator = { state.orchestrator.lock().unwrap().clone() }?;
    Ok(SkillStore::new(orchestrator.vault().clone()))
}

/// `overwrite: false` is "create" (refuses a name already taken, so the New form can't silently
/// replace another skill); `true` is "edit" (the frontend locks the name field then).
fn save_inner(store: &SkillStore, skill: Skill, overwrite: bool) -> Result<(), String> {
    skill.validate().map_err(|e| format!("{e:#}"))?;
    if !overwrite && store.exists(&skill.name) {
        return Err(format!("a skill named '{}' already exists", skill.name));
    }
    store.save(&skill).map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub fn list_skills(state: State<'_, AppState>) -> Result<Vec<SkillPayload>, String> {
    Ok(store(&state)?.list().into_iter().map(Into::into).collect())
}

#[tauri::command]
pub fn save_skill(state: State<'_, AppState>, skill: SkillPayload, overwrite: bool) -> Result<(), String> {
    save_inner(&store(&state)?, skill.into(), overwrite)
}

#[tauri::command]
pub fn delete_skill(state: State<'_, AppState>, name: String) -> Result<(), String> {
    store(&state)?.delete(&name).map_err(|e| format!("{e:#}"))
}

/// Asks the model to draft a skill from `prompt` and returns it *unsaved* — the frontend loads it
/// into the form for review. Uses the provider the user picked (`provider_id`, `null` = the active
/// one), same convention as `send_message`.
#[tauri::command]
pub async fn generate_skill_draft(
    state: State<'_, AppState>,
    prompt: String,
    provider_id: Option<String>,
) -> Result<SkillPayload, String> {
    let orchestrator = { state.orchestrator.lock().unwrap().clone() }?;
    let model = match provider_id {
        Some(id) => {
            let path = default_config_path().ok_or_else(|| "could not determine the OS config directory".to_string())?;
            let config = load_config_from_path(&path, false).map_err(|e| format!("{e:#}"))?;
            let provider =
                config.providers.iter().find(|p| p.id == id).ok_or_else(|| format!("model provider '{id}' not found"))?;
            build_model_provider(provider, None).map_err(|e| format!("{e:#}"))?
        }
        None => orchestrator.model().clone(),
    };
    let skill =
        warden_bootstrap::skill_gen::generate_skill_draft(model.as_ref(), &prompt).await.map_err(|e| format!("{e:#}"))?;
    Ok(skill.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use warden_core::memory::Vault;

    fn temp_store() -> SkillStore {
        SkillStore::new(Arc::new(Vault::new(std::env::temp_dir().join(format!(
            "warden-desktop-skills-test-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        )))))
    }

    fn skill(name: &str) -> Skill {
        Skill { name: name.into(), description: "d".into(), body: "b".into(), agents: Vec::new() }
    }

    #[test]
    fn agents_survive_the_payload_roundtrip_and_default_to_empty() {
        let restricted = Skill { agents: vec!["writer".into()], ..skill("x") };
        let payload: SkillPayload = restricted.clone().into();
        assert_eq!(Skill::from(payload), restricted);

        let legacy: SkillPayload = serde_json::from_str(r#"{"name":"x","description":"d","body":"b"}"#).unwrap();
        assert!(legacy.agents.is_empty());
    }

    #[test]
    fn create_refuses_an_existing_name_but_edit_overwrites_it() {
        let store = temp_store();
        save_inner(&store, skill("x"), false).unwrap();

        let err = save_inner(&store, skill("x"), false).unwrap_err();
        assert!(err.contains("already exists"));

        let edited = Skill { body: "new".into(), ..skill("x") };
        save_inner(&store, edited, true).unwrap();
        assert_eq!(store.get("x").unwrap().body, "new");
    }

    #[test]
    fn save_rejects_an_invalid_skill_before_touching_disk() {
        let store = temp_store();
        assert!(save_inner(&store, skill("../escape"), false).is_err());
        assert!(save_inner(&store, Skill { body: " ".into(), ..skill("ok") }, false).is_err());
        assert!(store.list().is_empty());
    }

    #[test]
    fn payload_serializes_camel_case_and_trims_the_name() {
        let json = serde_json::to_value(SkillPayload::from(skill("x"))).unwrap();
        assert_eq!(json, serde_json::json!({ "name": "x", "description": "d", "body": "b", "agents": [] }));

        let parsed: SkillPayload = serde_json::from_value(json).unwrap();
        let padded = SkillPayload { name: "  x  ".into(), ..parsed };
        assert_eq!(Skill::from(padded).name, "x");
    }
}
