//! A device's conversations over the wire — `RequestHistory` (P40) and, since P78, several
//! conversations per device: `ListConversations`/`RenameConversation`/`DeleteConversation`, and
//! which one a `Chat` turn goes to. Pure functions over the conversations directory, kept out of
//! `server.rs` so they're testable without a socket — same split as `skills.rs`.
//!
//! On disk each device has its own directory, `<root>/<device dir>/<conversation id>.json`, read
//! and written with the same `warden_bootstrap` functions every other channel uses. Before P78 a
//! device had exactly one conversation, `<root>/<device_id>.json`; `device_conversations_dir`
//! moves that file into the device's directory as its `default` conversation, the one a client
//! that never names a conversation (mobile, extension and `warden-node` from before P78) keeps
//! talking to.

use std::path::{Path, PathBuf};

use warden_bootstrap::{
    delete_conversation, list_conversations, load_conversation, rename_conversation, save_conversation, ChatRole, Conversation,
    ConversationMessage,
};
use warden_server_protocol::protocol::{ConversationSummary, HistoryMessage, HistoryRole};
use warden_server_protocol::{ClientMessage, ServerMessage};

/// The conversation a `Chat`/`RequestHistory` without a `conversation_id` goes to.
pub const DEFAULT_CONVERSATION_ID: &str = "default";

const MAX_ID_LEN: usize = 64;

/// Whether `id` is safe to use as a file or directory name: 1-64 ASCII letters, digits, `-` or
/// `_`. Covers the UUIDs every client generates, and nothing that could climb out of a directory.
pub fn is_valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_ID_LEN && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// The `conversation_id` of a `Chat`/`RequestHistory`, with `None` meaning the default one. An
/// invalid id is refused with a message for the client instead of reaching the filesystem.
pub fn resolve_conversation_id(conversation_id: Option<String>) -> Result<String, String> {
    match conversation_id {
        None => Ok(DEFAULT_CONVERSATION_ID.to_string()),
        Some(id) if is_valid_id(&id) => Ok(id),
        Some(id) => Err(format!("invalid conversation id '{id}' (use 1-{MAX_ID_LEN} letters, digits, '-' or '_')")),
    }
}

/// `device_id`'s conversations directory under `root`, created on first write. A device id that
/// isn't a safe name (`warden-node --device-id` takes anything) gets a hash-derived directory
/// instead, so it can never point outside `root`.
///
/// Also migrates the device's pre-P78 single conversation (`<root>/<device_id>.json`) into it as
/// the `default` conversation, once — called when a device connects, so the move never races the
/// device's own requests. A legacy file that doesn't parse is moved as is, so `RequestHistory`
/// keeps reporting the parse error it always did instead of it disappearing.
pub fn device_conversations_dir(root: &Path, device_id: &str) -> anyhow::Result<PathBuf> {
    if !is_valid_id(device_id) {
        use sha2::{Digest, Sha256};
        let hash: String = Sha256::digest(device_id.as_bytes()).iter().take(16).map(|b| format!("{b:02x}")).collect();
        return Ok(root.join(format!("device-{hash}")));
    }
    let dir = root.join(device_id);
    let legacy = root.join(format!("{device_id}.json"));
    let migrated = dir.join(format!("{DEFAULT_CONVERSATION_ID}.json"));
    if legacy.is_file() && !migrated.exists() {
        match load_conversation(root, device_id) {
            Ok(Some(conversation)) => {
                // `save_conversation` names the file after `conversation.id`, so the id changes too.
                save_conversation(&dir, &Conversation { id: DEFAULT_CONVERSATION_ID.to_string(), ..conversation })?;
                std::fs::remove_file(&legacy)?;
            }
            Ok(None) => {}
            Err(_) => {
                std::fs::create_dir_all(&dir)?;
                std::fs::rename(&legacy, &migrated)?;
            }
        }
    }
    Ok(dir)
}

/// Answers a `RequestHistory`: that conversation's last `limit` messages (all of them when `None`),
/// oldest first. A conversation that doesn't exist yet is an empty `History`, not an error.
pub fn handle_history_request(device_dir: &Path, request_id: u64, limit: Option<u32>, conversation_id: Option<String>) -> ServerMessage {
    let id = match resolve_conversation_id(conversation_id) {
        Ok(id) => id,
        Err(message) => return ServerMessage::HistoryError { request_id, message },
    };
    match load_conversation(device_dir, &id) {
        Ok(conversation) => {
            let messages = conversation.map(|c| c.messages).unwrap_or_default();
            let skip = limit.map_or(0, |limit| messages.len().saturating_sub(limit as usize));
            ServerMessage::History { request_id, messages: messages.into_iter().skip(skip).map(to_history_message).collect() }
        }
        Err(err) => ServerMessage::HistoryError { request_id, message: format!("{err:#}") },
    }
}

/// Answers a `ListConversations`/`RenameConversation`/`DeleteConversation`, or `None` for any
/// other message.
pub fn handle_conversation_request(device_dir: &Path, message: ClientMessage) -> Option<ServerMessage> {
    let (request_id, result) = match message {
        ClientMessage::ListConversations { request_id } => {
            return Some(match list_conversations(device_dir) {
                Ok(conversations) => ServerMessage::ConversationList {
                    request_id,
                    conversations: conversations.into_iter().map(to_summary).collect(),
                },
                Err(err) => ServerMessage::ConversationError { request_id, message: format!("{err:#}") },
            });
        }
        ClientMessage::RenameConversation { request_id, conversation_id, title } => {
            (request_id, existing(conversation_id).and_then(|id| found(rename_conversation(device_dir, &id, &title), &id)))
        }
        ClientMessage::DeleteConversation { request_id, conversation_id } => {
            (request_id, existing(conversation_id).and_then(|id| found(delete_conversation(device_dir, &id), &id)))
        }
        _ => return None,
    };
    Some(match result {
        Ok(()) => ServerMessage::ConversationOk { request_id },
        Err(message) => ServerMessage::ConversationError { request_id, message },
    })
}

/// Rename/delete always name an existing conversation — no default applies.
fn existing(conversation_id: String) -> Result<String, String> {
    resolve_conversation_id(Some(conversation_id))
}

fn found(result: anyhow::Result<bool>, id: &str) -> Result<(), String> {
    match result {
        Ok(true) => Ok(()),
        Ok(false) => Err(format!("no conversation with id '{id}'")),
        Err(err) => Err(format!("{err:#}")),
    }
}

fn to_summary(conversation: Conversation) -> ConversationSummary {
    ConversationSummary {
        id: conversation.id,
        title: conversation.title,
        created_at: conversation.created_at,
        updated_at: conversation.updated_at,
    }
}

fn to_history_message(message: ConversationMessage) -> HistoryMessage {
    HistoryMessage {
        role: match message.role {
            ChatRole::User => HistoryRole::User,
            ChatRole::Assistant => HistoryRole::Assistant,
        },
        content: message.content,
        created_at: message.created_at,
        attachments: message.attachments,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> PathBuf {
        std::env::temp_dir().join(format!(
            "warden-server-conversations-test-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ))
    }

    fn message(role: ChatRole, content: &str, created_at: i64) -> ConversationMessage {
        ConversationMessage {
            id: format!("m{created_at}"),
            role,
            content: content.into(),
            created_at,
            usage: None,
            attachments: Vec::new(),
            generated_files: Vec::new(),
        }
    }

    fn save(dir: &Path, id: &str, updated_at: i64, messages: Vec<ConversationMessage>) {
        let conversation = Conversation {
            id: id.into(),
            title: format!("title {id}"),
            messages,
            created_at: 0,
            updated_at,
            agent_id: None,
            provider_id: None,
        };
        save_conversation(dir, &conversation).unwrap();
    }

    fn contents(reply: ServerMessage) -> Vec<String> {
        match reply {
            ServerMessage::History { messages, .. } => messages.into_iter().map(|m| m.content).collect(),
            other => panic!("expected History, got {other:?}"),
        }
    }

    #[test]
    fn ids_are_limited_to_safe_file_names() {
        for ok in ["default", "3f2b-9c_a", &"a".repeat(64)] {
            assert!(is_valid_id(ok), "{ok}");
        }
        for bad in ["", "..", "../x", "a/b", "a.json", "a b", &"a".repeat(65)] {
            assert!(!is_valid_id(bad), "{bad}");
        }
        assert_eq!(resolve_conversation_id(None).unwrap(), DEFAULT_CONVERSATION_ID);
        assert!(resolve_conversation_id(Some("../etc".into())).is_err());
    }

    #[test]
    fn an_unsafe_device_id_gets_a_hashed_directory_inside_the_root() {
        let root = temp_dir();
        let dir = device_conversations_dir(&root, "../../escape").unwrap();
        assert_eq!(dir.parent().unwrap(), root);
        assert!(dir.file_name().unwrap().to_str().unwrap().starts_with("device-"));
        assert_eq!(device_conversations_dir(&root, "../../escape").unwrap(), dir);
    }

    #[test]
    fn the_pre_p78_conversation_becomes_the_default_one() {
        let root = temp_dir();
        save(&root, "dev-1", 5, vec![message(ChatRole::User, "hi", 1)]);

        let dir = device_conversations_dir(&root, "dev-1").unwrap();

        assert!(!root.join("dev-1.json").exists());
        let migrated = load_conversation(&dir, DEFAULT_CONVERSATION_ID).unwrap().unwrap();
        assert_eq!(migrated.id, DEFAULT_CONVERSATION_ID);
        assert_eq!(migrated.title, "title dev-1");
        assert_eq!(contents(handle_history_request(&dir, 1, None, None)), vec!["hi"]);
        // Running again (the next connection) is a no-op.
        assert_eq!(device_conversations_dir(&root, "dev-1").unwrap(), dir);
    }

    #[test]
    fn a_corrupt_pre_p78_conversation_is_moved_and_still_reports_its_error() {
        let root = temp_dir();
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("dev-1.json"), "not json").unwrap();

        let dir = device_conversations_dir(&root, "dev-1").unwrap();

        assert!(!root.join("dev-1.json").exists());
        match handle_history_request(&dir, 5, None, None) {
            ServerMessage::HistoryError { request_id, message } => {
                assert_eq!(request_id, 5);
                assert!(message.contains("failed to parse"), "message was: {message}");
            }
            other => panic!("expected HistoryError, got {other:?}"),
        }
    }

    #[test]
    fn a_conversation_that_never_existed_has_an_empty_history() {
        let reply = handle_history_request(&temp_dir(), 3, None, Some("new-one".into()));
        assert_eq!(reply, ServerMessage::History { request_id: 3, messages: Vec::new() });
    }

    #[test]
    fn history_is_per_conversation_in_order_with_roles() {
        let dir = temp_dir();
        save(&dir, "c1", 1, vec![message(ChatRole::User, "hi", 1), message(ChatRole::Assistant, "hello", 2)]);
        save(&dir, "c2", 1, vec![message(ChatRole::User, "another topic", 1)]);

        match handle_history_request(&dir, 4, None, Some("c1".into())) {
            ServerMessage::History { request_id, messages } => {
                assert_eq!(request_id, 4);
                assert_eq!(messages.iter().map(|m| m.role).collect::<Vec<_>>(), vec![HistoryRole::User, HistoryRole::Assistant]);
                assert_eq!(messages.iter().map(|m| m.content.as_str()).collect::<Vec<_>>(), vec!["hi", "hello"]);
                assert_eq!(messages[1].created_at, 2);
            }
            other => panic!("expected History, got {other:?}"),
        }
        assert_eq!(contents(handle_history_request(&dir, 4, None, Some("c2".into()))), vec!["another topic"]);
    }

    #[test]
    fn limit_keeps_only_the_most_recent_messages() {
        let dir = temp_dir();
        save(&dir, "default", 1, (1..=5).map(|i| message(ChatRole::User, &format!("m{i}"), i)).collect());

        assert_eq!(contents(handle_history_request(&dir, 1, Some(2), None)), vec!["m4", "m5"]);
        assert_eq!(contents(handle_history_request(&dir, 1, Some(10), None)).len(), 5);
        assert!(contents(handle_history_request(&dir, 1, Some(0), None)).is_empty());
    }

    #[test]
    fn an_invalid_conversation_id_is_a_history_error() {
        let reply = handle_history_request(&temp_dir(), 6, None, Some("../x".into()));
        assert!(matches!(reply, ServerMessage::HistoryError { request_id: 6, .. }), "{reply:?}");
    }

    #[test]
    fn lists_newest_updated_first() {
        let dir = temp_dir();
        save(&dir, "old", 1, Vec::new());
        save(&dir, "new", 9, Vec::new());

        let reply = handle_conversation_request(&dir, ClientMessage::ListConversations { request_id: 1 }).unwrap();
        match reply {
            ServerMessage::ConversationList { request_id: 1, conversations } => {
                assert_eq!(conversations.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), vec!["new", "old"]);
                assert_eq!(conversations[0].title, "title new");
                assert_eq!(conversations[0].updated_at, 9);
            }
            other => panic!("expected ConversationList, got {other:?}"),
        }
    }

    #[test]
    fn a_device_without_conversations_lists_none() {
        let reply = handle_conversation_request(&temp_dir(), ClientMessage::ListConversations { request_id: 2 }).unwrap();
        assert_eq!(reply, ServerMessage::ConversationList { request_id: 2, conversations: Vec::new() });
    }

    #[test]
    fn rename_and_delete_answer_ok_or_an_error() {
        let dir = temp_dir();
        save(&dir, "c1", 1, Vec::new());
        let rename = |id: &str, title: &str| {
            handle_conversation_request(&dir, ClientMessage::RenameConversation { request_id: 3, conversation_id: id.into(), title: title.into() })
                .unwrap()
        };
        let delete = |id: &str| handle_conversation_request(&dir, ClientMessage::DeleteConversation { request_id: 4, conversation_id: id.into() }).unwrap();

        assert_eq!(rename("c1", "Trip"), ServerMessage::ConversationOk { request_id: 3 });
        assert_eq!(load_conversation(&dir, "c1").unwrap().unwrap().title, "Trip");
        assert!(matches!(rename("c1", " "), ServerMessage::ConversationError { request_id: 3, .. }));
        assert!(matches!(rename("missing", "x"), ServerMessage::ConversationError { request_id: 3, message } if message.contains("no conversation")));
        assert!(matches!(rename("../c1", "x"), ServerMessage::ConversationError { request_id: 3, message } if message.contains("invalid")));

        assert_eq!(delete("c1"), ServerMessage::ConversationOk { request_id: 4 });
        assert_eq!(load_conversation(&dir, "c1").unwrap(), None);
        assert!(matches!(delete("c1"), ServerMessage::ConversationError { request_id: 4, .. }));
    }

    #[test]
    fn other_messages_are_not_conversation_requests() {
        assert_eq!(handle_conversation_request(&temp_dir(), ClientMessage::Ping { nonce: 1 }), None);
    }
}
