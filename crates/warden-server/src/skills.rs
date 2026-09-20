//! Skills management over the wire (P72) — what `ListSkills`/`SaveSkill`/`DeleteSkill` do to the
//! vault this server hosts, so a client without a vault of its own (the browser extension) can
//! manage skills. A pure function over `SkillStore`, kept out of `server.rs` so it's testable
//! without a socket; the same create/edit rules as the desktop's `save_skill` and the mobile
//! bridge (create refuses a taken name, edit overwrites).

use warden_core::skill::{Skill, SkillStore};
use warden_server_protocol::protocol::SkillDto;
use warden_server_protocol::{ClientMessage, ServerMessage};

/// Answers a skills request, or `None` for any other message.
pub fn handle_skill_request(store: &SkillStore, message: ClientMessage) -> Option<ServerMessage> {
    Some(match message {
        ClientMessage::ListSkills { request_id } => {
            ServerMessage::SkillList { request_id, skills: store.list().into_iter().map(SkillDto::from).collect() }
        }
        ClientMessage::SaveSkill { request_id, skill, overwrite } => match save(store, skill, overwrite) {
            Ok(()) => ServerMessage::SkillOk { request_id },
            Err(message) => ServerMessage::SkillError { request_id, message },
        },
        ClientMessage::DeleteSkill { request_id, name } => match store.delete(&name) {
            Ok(()) => ServerMessage::SkillOk { request_id },
            Err(err) => ServerMessage::SkillError { request_id, message: format!("{err:#}") },
        },
        _ => return None,
    })
}

fn save(store: &SkillStore, dto: SkillDto, overwrite: bool) -> Result<(), String> {
    let name = dto.name.trim().to_string();
    // An edit that sends no agents keeps the restriction already on disk (see `SaveSkill`'s docs) —
    // the extension has no UI to change it, and must not make a restricted skill global by saving.
    let agents = if overwrite && dto.agents.is_empty() { store.get(&name).map(|s| s.agents).unwrap_or_default() } else { dto.agents };
    let skill = Skill { name, description: dto.description, body: dto.body, agents };
    skill.validate().map_err(|e| format!("{e:#}"))?;
    if !overwrite && store.exists(&skill.name) {
        return Err(format!("a skill named '{}' already exists", skill.name));
    }
    store.save(&skill).map_err(|e| format!("{e:#}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use warden_core::memory::Vault;

    fn temp_store() -> SkillStore {
        SkillStore::new(Arc::new(Vault::new(std::env::temp_dir().join(format!(
            "warden-server-skills-test-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        )))))
    }

    fn dto(name: &str) -> SkillDto {
        SkillDto { name: name.into(), description: "d".into(), body: "b".into(), agents: Vec::new() }
    }

    fn save_req(skill: SkillDto, overwrite: bool) -> ClientMessage {
        ClientMessage::SaveSkill { request_id: 9, skill, overwrite }
    }

    #[test]
    fn non_skill_messages_are_not_handled() {
        assert!(handle_skill_request(&temp_store(), ClientMessage::Ping { nonce: 1 }).is_none());
    }

    #[test]
    fn save_list_delete_roundtrip_echoes_the_request_id() {
        let store = temp_store();
        assert_eq!(handle_skill_request(&store, save_req(dto("x"), false)), Some(ServerMessage::SkillOk { request_id: 9 }));

        match handle_skill_request(&store, ClientMessage::ListSkills { request_id: 4 }) {
            Some(ServerMessage::SkillList { request_id: 4, skills }) => assert_eq!(skills, vec![dto("x")]),
            other => panic!("unexpected {other:?}"),
        }

        let deleted = handle_skill_request(&store, ClientMessage::DeleteSkill { request_id: 5, name: "x".into() });
        assert_eq!(deleted, Some(ServerMessage::SkillOk { request_id: 5 }));
        assert!(store.list().is_empty());
    }

    #[test]
    fn create_refuses_a_taken_name_but_edit_overwrites() {
        let store = temp_store();
        handle_skill_request(&store, save_req(dto("x"), false));

        match handle_skill_request(&store, save_req(dto("x"), false)) {
            Some(ServerMessage::SkillError { message, .. }) => assert!(message.contains("already exists")),
            other => panic!("unexpected {other:?}"),
        }
        let edited = SkillDto { body: "new".into(), ..dto("x") };
        assert_eq!(handle_skill_request(&store, save_req(edited, true)), Some(ServerMessage::SkillOk { request_id: 9 }));
        assert_eq!(store.get("x").unwrap().body, "new");
    }

    #[test]
    fn invalid_skills_and_unknown_deletes_come_back_as_errors() {
        let store = temp_store();
        assert!(matches!(handle_skill_request(&store, save_req(dto("../escape"), false)), Some(ServerMessage::SkillError { .. })));
        assert!(matches!(
            handle_skill_request(&store, ClientMessage::DeleteSkill { request_id: 1, name: "nope".into() }),
            Some(ServerMessage::SkillError { .. })
        ));
        assert!(store.list().is_empty());
    }

    #[test]
    fn an_edit_without_agents_keeps_the_stored_restriction_but_an_explicit_list_replaces_it() {
        let store = temp_store();
        handle_skill_request(&store, save_req(SkillDto { agents: vec!["writer".into()], ..dto("x") }, false));

        handle_skill_request(&store, save_req(SkillDto { body: "new".into(), ..dto("x") }, true));
        assert_eq!(store.get("x").unwrap().agents, vec!["writer"]);

        handle_skill_request(&store, save_req(SkillDto { agents: vec!["reviewer".into()], ..dto("x") }, true));
        assert_eq!(store.get("x").unwrap().agents, vec!["reviewer"]);
    }
}
