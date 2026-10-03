//! Projects management over the wire (P103) — what `ListProjects`/`SaveProject`/`DeleteProject` do to the vault of the
//! person asking (the owner's, or a member's own), so a client without a vault of its own (the browser) can manage them.
//! A pure function over `ProjectStore`, kept out of `server.rs` so it's testable without a socket, with the same
//! create/edit rules as `skills.rs` (create refuses a taken id, edit overwrites) and the desktop's commands.

use warden_core::project::{Project, ProjectStore};
use warden_server_protocol::protocol::ProjectDto;
use warden_server_protocol::{ClientMessage, ServerMessage};

/// Answers a projects request, or `None` for any other message.
pub fn handle_project_request(store: &ProjectStore, message: ClientMessage) -> Option<ServerMessage> {
    Some(match message {
        ClientMessage::ListProjects { request_id } => ServerMessage::ProjectList { request_id, projects: store.list().into_iter().map(ProjectDto::from).collect() },
        ClientMessage::SaveProject { request_id, project, overwrite } => match save(store, project, overwrite) {
            Ok(()) => ServerMessage::ProjectOk { request_id },
            Err(message) => ServerMessage::ProjectError { request_id, message },
        },
        ClientMessage::DeleteProject { request_id, id } => match store.delete(id.trim()) {
            Ok(()) => ServerMessage::ProjectOk { request_id },
            Err(err) => ServerMessage::ProjectError { request_id, message: format!("{err:#}") },
        },
        _ => return None,
    })
}

fn save(store: &ProjectStore, dto: ProjectDto, overwrite: bool) -> Result<(), String> {
    let project = Project { id: dto.id.trim().to_string(), name: dto.name.trim().to_string(), description: dto.description, instructions: dto.instructions, workdir: dto.workdir.map(|dir| dir.trim().to_string()).filter(|dir| !dir.is_empty()) };
    project.validate().map_err(|e| format!("{e:#}"))?;
    if !overwrite && store.exists(&project.id) {
        return Err(format!("a project '{}' already exists", project.id));
    }
    store.save(&project).map_err(|e| format!("{e:#}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use warden_core::memory::Vault;

    fn store() -> ProjectStore {
        ProjectStore::new(Arc::new(Vault::new(std::env::temp_dir().join(format!(
            "warden-server-projects-test-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        )))))
    }

    fn dto(id: &str) -> ProjectDto {
        ProjectDto { id: id.into(), name: "Tax".into(), description: "d".into(), instructions: "i".into(), workdir: None }
    }

    fn save_req(project: ProjectDto, overwrite: bool) -> ClientMessage {
        ClientMessage::SaveProject { request_id: 9, project, overwrite }
    }

    #[test]
    fn other_messages_are_not_handled() {
        assert!(handle_project_request(&store(), ClientMessage::Ping { nonce: 1 }).is_none());
    }

    #[test]
    fn save_list_delete_roundtrip_echoes_the_request_id() {
        let store = store();
        assert_eq!(handle_project_request(&store, save_req(dto("tax"), false)), Some(ServerMessage::ProjectOk { request_id: 9 }));
        match handle_project_request(&store, ClientMessage::ListProjects { request_id: 4 }) {
            Some(ServerMessage::ProjectList { request_id: 4, projects }) => assert_eq!(projects, vec![dto("tax")]),
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(handle_project_request(&store, ClientMessage::DeleteProject { request_id: 5, id: "tax".into() }), Some(ServerMessage::ProjectOk { request_id: 5 }));
        assert!(matches!(handle_project_request(&store, ClientMessage::DeleteProject { request_id: 6, id: "tax".into() }), Some(ServerMessage::ProjectError { request_id: 6, .. })), "already gone");
    }

    #[test]
    fn create_refuses_a_taken_id_and_edit_overwrites_keeping_the_files() {
        let store = store();
        handle_project_request(&store, save_req(dto("tax"), false));
        let taken = handle_project_request(&store, save_req(dto("tax"), false));
        assert!(matches!(taken, Some(ServerMessage::ProjectError { message, .. }) if message.contains("already exists")));

        let vault_file = "projects/tax/jan.md";
        let scoped_root = store.scope("tax").unwrap();
        scoped_root.write("jan.md", "receipts").unwrap();
        let edited = ProjectDto { name: "Tax 2026".into(), instructions: "new".into(), ..dto("tax") };
        assert_eq!(handle_project_request(&store, save_req(edited, true)), Some(ServerMessage::ProjectOk { request_id: 9 }));
        assert_eq!(store.get("tax").unwrap().name, "Tax 2026");
        assert_eq!(scoped_root.read("jan.md").unwrap(), "receipts", "editing the project doesn't touch its files ({vault_file})");
    }

    #[test]
    fn what_cannot_be_kept_is_refused_with_a_message() {
        let store = store();
        for bad in [dto("../escape"), dto("a b"), dto(""), ProjectDto { name: "  ".into(), ..dto("ok") }, ProjectDto { instructions: "x".repeat(20_000), ..dto("ok") }] {
            assert!(matches!(handle_project_request(&store, save_req(bad, false)), Some(ServerMessage::ProjectError { .. })));
        }
        assert!(store.list().is_empty());
        assert!(matches!(handle_project_request(&store, ClientMessage::DeleteProject { request_id: 1, id: "../x".into() }), Some(ServerMessage::ProjectError { .. })));
    }
}
