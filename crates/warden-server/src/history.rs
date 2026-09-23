//! Conversation history over the wire (P40) — what `RequestHistory` answers with. Reads the same
//! per-device conversation file `warden_bootstrap::handle_turn` appends every `Chat` turn to, so a
//! client that reconnects (the mobile app after being closed, or just a dropped connection) can
//! show what was already said instead of starting from an empty transcript. A pure function over
//! the conversations directory, kept out of `server.rs` so it's testable without a socket — same
//! split as `skills.rs`.

use std::path::Path;

use warden_bootstrap::{load_conversation, ChatRole, ConversationMessage};
use warden_server_protocol::protocol::{HistoryMessage, HistoryRole};
use warden_server_protocol::ServerMessage;

/// Answers a `RequestHistory` from `device_id`: its conversation's last `limit` messages (all of
/// them when `None`), oldest first. No conversation yet is an empty `History`, not an error.
pub fn handle_history_request(conversations_dir: &Path, device_id: &str, request_id: u64, limit: Option<u32>) -> ServerMessage {
    match load_conversation(conversations_dir, device_id) {
        Ok(conversation) => {
            let messages = conversation.map(|c| c.messages).unwrap_or_default();
            let skip = limit.map_or(0, |limit| messages.len().saturating_sub(limit as usize));
            ServerMessage::History { request_id, messages: messages.into_iter().skip(skip).map(to_history_message).collect() }
        }
        Err(err) => ServerMessage::HistoryError { request_id, message: format!("{err:#}") },
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
    use std::path::PathBuf;
    use warden_bootstrap::{save_conversation, Conversation};

    fn temp_dir() -> PathBuf {
        std::env::temp_dir().join(format!(
            "warden-server-history-test-{}",
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

    fn save(dir: &Path, id: &str, messages: Vec<ConversationMessage>) {
        let conversation = Conversation {
            id: id.into(),
            title: "t".into(),
            messages,
            created_at: 0,
            updated_at: 0,
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
    fn a_device_that_never_chatted_gets_an_empty_history() {
        let reply = handle_history_request(&temp_dir(), "dev-1", 3, None);
        assert_eq!(reply, ServerMessage::History { request_id: 3, messages: Vec::new() });
    }

    #[test]
    fn returns_the_device_conversation_in_order_with_roles() {
        let dir = temp_dir();
        save(&dir, "dev-1", vec![message(ChatRole::User, "hi", 1), message(ChatRole::Assistant, "hello", 2)]);
        save(&dir, "dev-2", vec![message(ChatRole::User, "someone else", 1)]);

        match handle_history_request(&dir, "dev-1", 4, None) {
            ServerMessage::History { request_id, messages } => {
                assert_eq!(request_id, 4);
                assert_eq!(messages.iter().map(|m| m.role).collect::<Vec<_>>(), vec![HistoryRole::User, HistoryRole::Assistant]);
                assert_eq!(messages.iter().map(|m| m.content.as_str()).collect::<Vec<_>>(), vec!["hi", "hello"]);
                assert_eq!(messages[1].created_at, 2);
            }
            other => panic!("expected History, got {other:?}"),
        }
    }

    #[test]
    fn limit_keeps_only_the_most_recent_messages() {
        let dir = temp_dir();
        save(&dir, "dev-1", (1..=5).map(|i| message(ChatRole::User, &format!("m{i}"), i)).collect());

        assert_eq!(contents(handle_history_request(&dir, "dev-1", 1, Some(2))), vec!["m4", "m5"]);
        assert_eq!(contents(handle_history_request(&dir, "dev-1", 1, Some(10))).len(), 5);
        assert!(contents(handle_history_request(&dir, "dev-1", 1, Some(0))).is_empty());
    }

    #[test]
    fn an_unreadable_conversation_file_is_a_history_error() {
        let dir = temp_dir();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("dev-1.json"), "not json").unwrap();

        match handle_history_request(&dir, "dev-1", 5, None) {
            ServerMessage::HistoryError { request_id, message } => {
                assert_eq!(request_id, 5);
                assert!(message.contains("failed to parse"), "message was: {message}");
            }
            other => panic!("expected HistoryError, got {other:?}"),
        }
    }
}
