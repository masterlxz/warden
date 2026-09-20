//! Bridge functions for the mobile "Skills" screen (P72 b) — the same `warden_core::skill` store the
//! desktop's `skills_cmds.rs` and the CLI's `/skills` use, so the name/description/body rules live
//! in one place instead of a Dart re-implementation of the frontmatter format. Skills are plain
//! `skills/<name>.md` files in the local vault, so they ride the existing sync
//! (`Vault::list_all_files`) to the desktop/server with no change here.
//!
//! Plain blocking `pub fn`s, like `api::sync` — pure file I/O, no Tokio needed.

use std::sync::Arc;

use warden_core::memory::Vault;
use warden_core::skill::{Skill, SkillStore};

#[derive(Debug, Clone)]
pub struct SkillDto {
    pub name: String,
    pub description: String,
    pub body: String,
}

impl From<Skill> for SkillDto {
    fn from(skill: Skill) -> Self {
        Self { name: skill.name, description: skill.description, body: skill.body }
    }
}

fn store(vault_root: &str) -> SkillStore {
    SkillStore::new(Arc::new(Vault::new(vault_root)))
}

pub fn bridge_list_skills(vault_root: String) -> Vec<SkillDto> {
    store(&vault_root).list().into_iter().map(Into::into).collect()
}

/// `overwrite: false` is "create" (refuses a name already taken, so the New form can't silently
/// replace another skill); `true` is "edit" (the Dart side locks the name field then). Same
/// contract as the desktop's `save_skill`.
pub fn bridge_save_skill(vault_root: String, skill: SkillDto, overwrite: bool) -> Result<(), String> {
    let skill = Skill { name: skill.name.trim().to_string(), description: skill.description, body: skill.body };
    skill.validate().map_err(|e| format!("{e:#}"))?;
    let store = store(&vault_root);
    if !overwrite && store.exists(&skill.name) {
        return Err(format!("a skill named '{}' already exists", skill.name));
    }
    store.save(&skill).map_err(|e| format!("{e:#}"))
}

pub fn bridge_delete_skill(vault_root: String, name: String) -> Result<(), String> {
    store(&vault_root).delete(&name).map_err(|e| format!("{e:#}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_vault() -> String {
        let dir = std::env::temp_dir().join(format!(
            "warden-bridge-skills-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        dir.to_string_lossy().into_owned()
    }

    fn dto(name: &str) -> SkillDto {
        SkillDto { name: name.into(), description: "Reviews a PR".into(), body: "Step 1.\nStep 2.".into() }
    }

    #[test]
    fn save_list_and_delete_roundtrip() {
        let vault = temp_vault();
        assert!(bridge_list_skills(vault.clone()).is_empty());

        bridge_save_skill(vault.clone(), dto("review-pr"), false).unwrap();
        let skills = bridge_list_skills(vault.clone());
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "review-pr");
        assert_eq!(skills[0].body, "Step 1.\nStep 2.");

        bridge_delete_skill(vault.clone(), "review-pr".into()).unwrap();
        assert!(bridge_list_skills(vault).is_empty());
    }

    #[test]
    fn create_refuses_an_existing_name_but_edit_overwrites_it() {
        let vault = temp_vault();
        bridge_save_skill(vault.clone(), dto("a"), false).unwrap();
        assert!(bridge_save_skill(vault.clone(), dto("a"), false).is_err());

        let edited = SkillDto { description: "New".into(), ..dto("a") };
        bridge_save_skill(vault.clone(), edited, true).unwrap();
        assert_eq!(bridge_list_skills(vault)[0].description, "New");
    }

    #[test]
    fn invalid_skills_and_unknown_deletes_are_errors() {
        let vault = temp_vault();
        assert!(bridge_save_skill(vault.clone(), dto("../x"), false).is_err());
        assert!(bridge_save_skill(vault.clone(), SkillDto { body: "  ".into(), ..dto("ok") }, false).is_err());
        assert!(bridge_delete_skill(vault, "missing".into()).is_err());
    }
}
