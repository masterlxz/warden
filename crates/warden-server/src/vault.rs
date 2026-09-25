//! The vault over the wire (P78) — what `ListVaultFiles`/`ReadVaultNote`/`SaveVaultNote`/
//! `DeleteVaultNote`/`SearchVault` do to the vault this server hosts, so the web UI can browse and
//! edit it. A pure function over `Vault`, like `skills.rs`; the path rules and the version check
//! live in `warden_core::memory` (`Vault::save_note` and friends), shared with the desktop.

use warden_core::memory::{NoteConflict, Vault};
use warden_server_protocol::protocol::VaultSearchHit;
use warden_server_protocol::{ClientMessage, ServerMessage};

/// Most lines a `SearchVault` answers with — a list someone scrolls, not the model's context.
pub const MAX_SEARCH_HITS: usize = 50;

/// Answers a vault request, or `None` for any other message.
pub fn handle_vault_request(vault: &Vault, message: ClientMessage) -> Option<ServerMessage> {
    let (request_id, result) = match message {
        ClientMessage::ListVaultFiles { request_id } => {
            (request_id, vault.browse_files().map(|files| ServerMessage::VaultFileList { request_id, files }))
        }
        ClientMessage::ReadVaultNote { request_id, path } => (
            request_id,
            vault.read_note(&path).map(|note| ServerMessage::VaultNote { request_id, path, content: note.content, version: note.version }),
        ),
        ClientMessage::SaveVaultNote { request_id, path, content, expected_version } => (
            request_id,
            vault.save_note(&path, &content, expected_version.as_deref()).map(|version| ServerMessage::VaultSaved { request_id, version }),
        ),
        ClientMessage::DeleteVaultNote { request_id, path, expected_version } => {
            (request_id, vault.delete_note(&path, &expected_version).map(|()| ServerMessage::VaultOk { request_id }))
        }
        ClientMessage::SearchVault { request_id, query } => (
            request_id,
            vault.search(&query, MAX_SEARCH_HITS).map(|hits| ServerMessage::VaultSearchResults {
                request_id,
                hits: hits.into_iter().map(|h| VaultSearchHit { path: h.path, line_number: h.line_number, line: h.line }).collect(),
            }),
        ),
        _ => return None,
    };
    Some(result.unwrap_or_else(|err| ServerMessage::VaultError {
        request_id,
        conflict: err.downcast_ref::<NoteConflict>().is_some(),
        message: format!("{err:#}"),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_vault() -> Vault {
        Vault::new(std::env::temp_dir().join(format!(
            "warden-server-vault-test-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        )))
    }

    fn save(path: &str, content: &str, expected_version: Option<&str>) -> ClientMessage {
        ClientMessage::SaveVaultNote { request_id: 3, path: path.into(), content: content.into(), expected_version: expected_version.map(Into::into) }
    }

    fn saved_version(reply: Option<ServerMessage>) -> String {
        match reply {
            Some(ServerMessage::VaultSaved { request_id: 3, version }) => version,
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn non_vault_messages_are_not_handled() {
        assert!(handle_vault_request(&temp_vault(), ClientMessage::ListSkills { request_id: 1 }).is_none());
    }

    #[test]
    fn create_list_read_edit_and_delete_echo_the_request_id() {
        let vault = temp_vault();
        vault.write("_profile.md", "fixed").unwrap();
        let v1 = saved_version(handle_vault_request(&vault, save("notes/a.md", "one", None)));

        assert_eq!(
            handle_vault_request(&vault, ClientMessage::ListVaultFiles { request_id: 1 }),
            Some(ServerMessage::VaultFileList { request_id: 1, files: vec!["notes/a.md".into()] })
        );
        assert_eq!(
            handle_vault_request(&vault, ClientMessage::ReadVaultNote { request_id: 2, path: "notes/a.md".into() }),
            Some(ServerMessage::VaultNote { request_id: 2, path: "notes/a.md".into(), content: "one".into(), version: v1.clone() })
        );

        let v2 = saved_version(handle_vault_request(&vault, save("notes/a.md", "two", Some(&v1))));
        let delete = ClientMessage::DeleteVaultNote { request_id: 4, path: "notes/a.md".into(), expected_version: v2 };
        assert_eq!(handle_vault_request(&vault, delete), Some(ServerMessage::VaultOk { request_id: 4 }));
        assert!(vault.read("notes/a.md").is_err());
    }

    #[test]
    fn a_stale_save_is_a_conflict_and_a_bad_path_is_a_plain_error() {
        let vault = temp_vault();
        let v1 = saved_version(handle_vault_request(&vault, save("a.md", "one", None)));
        vault.write("a.md", "changed by the model").unwrap();

        match handle_vault_request(&vault, save("a.md", "mine", Some(&v1))) {
            Some(ServerMessage::VaultError { request_id: 3, conflict: true, message }) => assert!(message.contains("changed")),
            other => panic!("unexpected {other:?}"),
        }
        match handle_vault_request(&vault, ClientMessage::ReadVaultNote { request_id: 7, path: "../../etc/passwd".into() }) {
            Some(ServerMessage::VaultError { request_id: 7, conflict: false, .. }) => {}
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(vault.read("a.md").unwrap(), "changed by the model");
    }

    #[test]
    fn search_returns_matching_lines() {
        let vault = temp_vault();
        vault.write("notes/shopping.md", "eggs\nbuy milk").unwrap();
        vault.write("other.md", "nothing here").unwrap();
        assert_eq!(
            handle_vault_request(&vault, ClientMessage::SearchVault { request_id: 5, query: "milk".into() }),
            Some(ServerMessage::VaultSearchResults {
                request_id: 5,
                hits: vec![VaultSearchHit { path: "notes/shopping.md".into(), line_number: 2, line: "buy milk".into() }],
            })
        );
    }
}
