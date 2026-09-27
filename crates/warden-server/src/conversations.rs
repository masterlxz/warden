//! A device's conversations over the wire — `RequestHistory` (P40) and, since P78, several
//! conversations per device: `ListConversations`/`RenameConversation`/`DeleteConversation`, and
//! which one a `Chat` turn goes to. Pure functions over the conversations directory, kept out of
//! `server.rs` so they're testable without a socket — same split as `skills.rs`.
//!
//! On disk each person has one directory (P84): the owner's `<root>/root/`, a member's
//! `<root>/users/<id>/`, each `<conversation id>.json`, read and written with the same
//! `warden_bootstrap` functions every other channel uses. Before P84 each device had its own
//! directory, and before P78 a single `<root>/<device_id>.json`; `people::migrate_device_conversations`
//! moves both into the owner's when the hub starts.
//!
//! Scheduled tasks (P92) keep their conversations in one directory of their own, shared by every
//! device: an id starting with `task-` goes there (`ConversationDirs`), and every device's list
//! shows them next to its own.

use std::path::{Path, PathBuf};

use warden_bootstrap::tasks::CONVERSATION_PREFIX as TASK_PREFIX;
use warden_bootstrap::{
    delete_conversation, list_conversations, load_conversation, rename_conversation, ChatRole, Conversation,
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

/// Where one device's requests find conversations: its own directory, and the hub's scheduled-task
/// conversations (P92) when it keeps any.
#[derive(Debug, Clone)]
pub struct ConversationDirs {
    pub device: PathBuf,
    pub tasks: Option<PathBuf>,
}

impl ConversationDirs {
    /// The directory conversation `id` lives in.
    pub fn dir_for(&self, id: &str) -> &Path {
        match &self.tasks {
            Some(tasks) if id.starts_with(TASK_PREFIX) => tasks,
            _ => &self.device,
        }
    }

    /// The device's conversations and the tasks', newest-updated first.
    fn list(&self) -> anyhow::Result<Vec<Conversation>> {
        let mut conversations = list_conversations(&self.device)?;
        if let Some(tasks) = &self.tasks {
            conversations.extend(list_conversations(tasks)?);
            conversations.sort_by_key(|c| std::cmp::Reverse(c.updated_at));
        }
        Ok(conversations)
    }
}

/// Answers a `RequestHistory`: that conversation's last `limit` messages (all of them when `None`),
/// oldest first. A conversation that doesn't exist yet is an empty `History`, not an error.
pub fn handle_history_request(dirs: &ConversationDirs, request_id: u64, limit: Option<u32>, conversation_id: Option<String>) -> ServerMessage {
    let id = match resolve_conversation_id(conversation_id) {
        Ok(id) => id,
        Err(message) => return ServerMessage::HistoryError { request_id, message },
    };
    match load_conversation(dirs.dir_for(&id), &id) {
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
pub fn handle_conversation_request(dirs: &ConversationDirs, message: ClientMessage) -> Option<ServerMessage> {
    let (request_id, result) = match message {
        ClientMessage::ListConversations { request_id } => {
            return Some(match dirs.list() {
                Ok(conversations) => ServerMessage::ConversationList {
                    request_id,
                    conversations: conversations.into_iter().map(to_summary).collect(),
                },
                Err(err) => ServerMessage::ConversationError { request_id, message: format!("{err:#}") },
            });
        }
        ClientMessage::RenameConversation { request_id, conversation_id, title } => {
            (request_id, existing(conversation_id).and_then(|id| found(rename_conversation(dirs.dir_for(&id), &id, &title), &id)))
        }
        ClientMessage::DeleteConversation { request_id, conversation_id } => {
            (request_id, existing(conversation_id).and_then(|id| found(delete_conversation(dirs.dir_for(&id), &id), &id)))
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
        agent_id: conversation.agent_id,
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
    use warden_bootstrap::save_conversation;

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

    fn only(dir: &Path) -> ConversationDirs {
        ConversationDirs { device: dir.to_path_buf(), tasks: None }
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
    fn a_conversation_that_never_existed_has_an_empty_history() {
        let reply = handle_history_request(&only(&temp_dir()), 3, None, Some("new-one".into()));
        assert_eq!(reply, ServerMessage::History { request_id: 3, messages: Vec::new() });
    }

    #[test]
    fn history_is_per_conversation_in_order_with_roles() {
        let dir = temp_dir();
        save(&dir, "c1", 1, vec![message(ChatRole::User, "hi", 1), message(ChatRole::Assistant, "hello", 2)]);
        save(&dir, "c2", 1, vec![message(ChatRole::User, "another topic", 1)]);

        match handle_history_request(&only(&dir), 4, None, Some("c1".into())) {
            ServerMessage::History { request_id, messages } => {
                assert_eq!(request_id, 4);
                assert_eq!(messages.iter().map(|m| m.role).collect::<Vec<_>>(), vec![HistoryRole::User, HistoryRole::Assistant]);
                assert_eq!(messages.iter().map(|m| m.content.as_str()).collect::<Vec<_>>(), vec!["hi", "hello"]);
                assert_eq!(messages[1].created_at, 2);
            }
            other => panic!("expected History, got {other:?}"),
        }
        assert_eq!(contents(handle_history_request(&only(&dir), 4, None, Some("c2".into()))), vec!["another topic"]);
    }

    #[test]
    fn limit_keeps_only_the_most_recent_messages() {
        let dir = temp_dir();
        save(&dir, "default", 1, (1..=5).map(|i| message(ChatRole::User, &format!("m{i}"), i)).collect());

        assert_eq!(contents(handle_history_request(&only(&dir), 1, Some(2), None)), vec!["m4", "m5"]);
        assert_eq!(contents(handle_history_request(&only(&dir), 1, Some(10), None)).len(), 5);
        assert!(contents(handle_history_request(&only(&dir), 1, Some(0), None)).is_empty());
    }

    #[test]
    fn an_invalid_conversation_id_is_a_history_error() {
        let reply = handle_history_request(&only(&temp_dir()), 6, None, Some("../x".into()));
        assert!(matches!(reply, ServerMessage::HistoryError { request_id: 6, .. }), "{reply:?}");
    }

    #[test]
    fn lists_newest_updated_first() {
        let dir = temp_dir();
        save(&dir, "old", 1, Vec::new());
        save(&dir, "new", 9, Vec::new());

        let reply = handle_conversation_request(&only(&dir), ClientMessage::ListConversations { request_id: 1 }).unwrap();
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
        let reply = handle_conversation_request(&only(&temp_dir()), ClientMessage::ListConversations { request_id: 2 }).unwrap();
        assert_eq!(reply, ServerMessage::ConversationList { request_id: 2, conversations: Vec::new() });
    }

    #[test]
    fn rename_and_delete_answer_ok_or_an_error() {
        let dir = temp_dir();
        save(&dir, "c1", 1, Vec::new());
        let rename = |id: &str, title: &str| {
            handle_conversation_request(&only(&dir), ClientMessage::RenameConversation { request_id: 3, conversation_id: id.into(), title: title.into() })
                .unwrap()
        };
        let delete = |id: &str| handle_conversation_request(&only(&dir), ClientMessage::DeleteConversation { request_id: 4, conversation_id: id.into() }).unwrap();

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
        assert_eq!(handle_conversation_request(&only(&temp_dir()), ClientMessage::Ping { nonce: 1 }), None);
    }

    #[test]
    fn task_conversations_are_shared_and_listed_with_the_device_ones() {
        let device = temp_dir();
        let tasks = temp_dir().join("tasks");
        let dirs = ConversationDirs { device: device.clone(), tasks: Some(tasks.clone()) };
        save(&device, "mine", 1, vec![message(ChatRole::User, "hi", 1)]);
        save(&tasks, "task-daily", 5, vec![message(ChatRole::Assistant, "summary", 5)]);

        assert_eq!(dirs.dir_for("task-daily"), tasks);
        assert_eq!(dirs.dir_for("mine"), device);
        match handle_conversation_request(&dirs, ClientMessage::ListConversations { request_id: 1 }).unwrap() {
            ServerMessage::ConversationList { conversations, .. } => {
                assert_eq!(conversations.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), vec!["task-daily", "mine"]);
            }
            other => panic!("expected ConversationList, got {other:?}"),
        }
        assert_eq!(contents(handle_history_request(&dirs, 1, None, Some("task-daily".into()))), vec!["summary"]);

        let rename = ClientMessage::RenameConversation { request_id: 2, conversation_id: "task-daily".into(), title: "Morning".into() };
        assert_eq!(handle_conversation_request(&dirs, rename).unwrap(), ServerMessage::ConversationOk { request_id: 2 });
        assert_eq!(load_conversation(&tasks, "task-daily").unwrap().unwrap().title, "Morning");
        let delete = ClientMessage::DeleteConversation { request_id: 3, conversation_id: "task-daily".into() };
        assert_eq!(handle_conversation_request(&dirs, delete).unwrap(), ServerMessage::ConversationOk { request_id: 3 });
        assert_eq!(load_conversation(&tasks, "task-daily").unwrap(), None);

        // Without a tasks directory, a `task-` id is just one of the device's own.
        assert_eq!(only(&device).dir_for("task-daily"), device);
    }
}
