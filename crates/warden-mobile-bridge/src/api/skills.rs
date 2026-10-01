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
    /// An AI suggestion (P104) still waiting for the person to accept it. Saving a DTO with this
    /// `true` keeps it a suggestion; accepting is an explicit save with `false`.
    pub proposed: bool,
    pub source: Option<String>,
    pub proposed_at: Option<i64>,
}

impl From<Skill> for SkillDto {
    fn from(skill: Skill) -> Self {
        Self { name: skill.name, description: skill.description, body: skill.body, proposed: skill.proposed, source: skill.source, proposed_at: skill.proposed_at }
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
    let store = store(&vault_root);
    // The mobile UI doesn't edit the agent restriction (P72 c), so an edit keeps whatever the
    // desktop/CLI set — otherwise saving here would silently make the skill global again.
    let agents = if overwrite { store.get(skill.name.trim()).map(|s| s.agents).unwrap_or_default() } else { Vec::new() };
    let name = skill.name.trim().to_string();
    // A suggestion that's edited but not accepted keeps when it was made (same rule as the hub's save).
    let proposed_at = if skill.proposed { skill.proposed_at.or_else(|| store.get(&name).ok().and_then(|s| s.proposed_at)) } else { None };
    let skill = Skill { name, description: skill.description, body: skill.body, agents, proposed: skill.proposed, source: if skill.proposed { skill.source } else { None }, proposed_at };
    skill.validate().map_err(|e| format!("{e:#}"))?;
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
        SkillDto { name: name.into(), description: "Reviews a PR".into(), body: "Step 1.\nStep 2.".into(), proposed: false, source: None, proposed_at: None }
    }

    #[test]
    fn a_suggestion_stays_pending_until_saved_as_accepted() {
        let vault = temp_vault();
        let suggested = SkillDto { proposed: true, source: Some("conversa".into()), proposed_at: Some(42), ..dto("tip") };
        bridge_save_skill(vault.clone(), suggested.clone(), false).unwrap();
        assert!(bridge_list_skills(vault.clone())[0].proposed);

        // Editing without accepting keeps it a suggestion and keeps when it was made.
        let edited = SkillDto { body: "Changed.".into(), proposed_at: None, ..suggested.clone() };
        bridge_save_skill(vault.clone(), edited, true).unwrap();
        let after = bridge_list_skills(vault.clone()).remove(0);
        assert!(after.proposed);
        assert_eq!(after.proposed_at, Some(42));

        // Accepting is an explicit save with `proposed: false`.
        bridge_save_skill(vault.clone(), SkillDto { proposed: false, ..suggested }, true).unwrap();
        let accepted = bridge_list_skills(vault).remove(0);
        assert!(!accepted.proposed && accepted.source.is_none() && accepted.proposed_at.is_none());
    }

    #[test]
    fn editing_keeps_the_agent_restriction_set_elsewhere() {
        let vault = temp_vault();
        bridge_save_skill(vault.clone(), dto("review-pr"), false).unwrap();
        let store = store(&vault);
        let mut restricted = store.get("review-pr").unwrap();
        restricted.agents = vec!["writer".into()];
        store.save(&restricted).unwrap();

        let edited = SkillDto { body: "New body.".into(), ..dto("review-pr") };
        bridge_save_skill(vault.clone(), edited, true).unwrap();

        let after = store.get("review-pr").unwrap();
        assert_eq!(after.body, "New body.");
        assert_eq!(after.agents, vec!["writer"]);
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
