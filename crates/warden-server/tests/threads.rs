//! P125 on the hub: a chat can be a thread of one message of another conversation. The history carries each message's id (what a thread
//! starts from), the list says which conversation is a thread and how many replies it has, a thread keeps its own messages, and deleting
//! the conversation takes its threads along. What the model sees inside a thread is `warden-bootstrap`'s test; here is what travels.

mod support;

use support::{spin_up_server, MockProvider};
use warden_server::{ClientMessage, ServerConnection, ServerMessage};
use warden_server_protocol::protocol::{ConversationSummary, HistoryMessage, ThreadParentDto};

fn chat(text: &str, conversation: &str, thread_of: Option<ThreadParentDto>) -> ClientMessage {
    ClientMessage::Chat { message: text.into(), conversation_id: Some(conversation.into()), attachments: Vec::new(), agent_id: None, project_id: None, workdir: None, thread_of }
}

fn link(conversation: &str, message: &str) -> Option<ThreadParentDto> {
    Some(ThreadParentDto { conversation_id: conversation.into(), message_id: message.into() })
}

async fn answer(conn: &mut ServerConnection) -> ServerMessage {
    loop {
        match conn.recv().await.unwrap().expect("connection closed") {
            reply @ (ServerMessage::ChatResponse { .. } | ServerMessage::ChatError { .. }) => return reply,
            _ => continue,
        }
    }
}

async fn history(conn: &mut ServerConnection, conversation: &str) -> Vec<HistoryMessage> {
    conn.send(&ClientMessage::RequestHistory { request_id: 1, limit: None, conversation_id: Some(conversation.into()) }).await.unwrap();
    loop {
        match conn.recv().await.unwrap().expect("connection closed") {
            ServerMessage::History { messages, .. } => return messages,
            _ => continue,
        }
    }
}

async fn list(conn: &mut ServerConnection) -> Vec<ConversationSummary> {
    conn.send(&ClientMessage::ListConversations { request_id: 2 }).await.unwrap();
    loop {
        match conn.recv().await.unwrap().expect("connection closed") {
            ServerMessage::ConversationList { conversations, .. } => return conversations,
            _ => continue,
        }
    }
}

#[tokio::test]
async fn a_thread_is_a_conversation_of_its_own_that_the_list_ties_to_its_message_and_the_history_gives_ids() {
    let addr = spin_up_server(MockProvider::replying("ahoy")).await;
    let mut conn = ServerConnection::connect(&format!("ws://{addr}"), "dev-1", "Test Device", "test-key").await.unwrap();

    conn.send(&chat("hello", "main", None)).await.unwrap();
    assert!(matches!(answer(&mut conn).await, ServerMessage::ChatResponse { .. }));
    let messages = history(&mut conn, "main").await;
    assert!(messages.iter().all(|m| !m.id.is_empty()), "every message has an id to start a thread from");
    let anchor = messages[1].id.clone();

    conn.send(&chat("what did you mean?", "side", link("main", &anchor))).await.unwrap();
    match answer(&mut conn).await {
        ServerMessage::ChatResponse { conversation_id, .. } => assert_eq!(conversation_id.as_deref(), Some("side")),
        other => panic!("expected the answer in the thread, got {other:?}"),
    }
    conn.send(&chat("and again?", "side", None)).await.unwrap();
    assert!(matches!(answer(&mut conn).await, ServerMessage::ChatResponse { .. }));

    // The thread holds only its own messages; the conversation it came from is as it was.
    assert_eq!(history(&mut conn, "side").await.len(), 4);
    assert_eq!(history(&mut conn, "main").await.len(), 2);

    let listed = list(&mut conn).await;
    let thread = listed.iter().find(|c| c.id == "side").unwrap();
    assert_eq!((thread.parent.clone(), thread.replies), (link("main", &anchor), 2));
    let main = listed.iter().find(|c| c.id == "main").unwrap();
    assert_eq!((main.parent.clone(), main.replies), (None, 0));
}

#[tokio::test]
async fn a_thread_that_cannot_exist_is_refused_with_a_chat_error_and_saves_nothing() {
    let addr = spin_up_server(MockProvider::replying("ahoy")).await;
    let mut conn = ServerConnection::connect(&format!("ws://{addr}"), "dev-1", "Test Device", "test-key").await.unwrap();
    conn.send(&chat("hello", "main", None)).await.unwrap();
    answer(&mut conn).await;
    let anchor = history(&mut conn, "main").await[0].id.clone();
    conn.send(&chat("first", "side", link("main", &anchor))).await.unwrap();
    answer(&mut conn).await;
    let in_thread = history(&mut conn, "side").await[0].id.clone();

    for (id, thread_of, why) in [
        ("t1", link("main", "no-such-message"), "a message that isn't there"),
        ("t2", link("ghost", &anchor), "a conversation that isn't there"),
        ("t3", link("side", &in_thread), "a thread of a thread"),
        ("t4", link("../escape", &anchor), "a path instead of a conversation id"),
        ("t5", link("main", " "), "no message id"),
    ] {
        conn.send(&chat("x", id, thread_of)).await.unwrap();
        assert!(matches!(answer(&mut conn).await, ServerMessage::ChatError { .. }), "{why}");
    }
    let ids: Vec<_> = list(&mut conn).await.into_iter().map(|c| c.id).collect();
    assert!(ids.iter().all(|id| id == "main" || id == "side"), "nothing was saved: {ids:?}");
}

#[tokio::test]
async fn deleting_a_conversation_takes_its_threads_with_it() {
    let addr = spin_up_server(MockProvider::replying("ahoy")).await;
    let mut conn = ServerConnection::connect(&format!("ws://{addr}"), "dev-1", "Test Device", "test-key").await.unwrap();
    conn.send(&chat("hello", "main", None)).await.unwrap();
    answer(&mut conn).await;
    let anchor = history(&mut conn, "main").await[0].id.clone();
    conn.send(&chat("in the thread", "side", link("main", &anchor))).await.unwrap();
    answer(&mut conn).await;
    conn.send(&chat("another conversation", "other", None)).await.unwrap();
    answer(&mut conn).await;

    conn.send(&ClientMessage::DeleteConversation { request_id: 3, conversation_id: "main".into() }).await.unwrap();
    loop {
        match conn.recv().await.unwrap().expect("connection closed") {
            ServerMessage::ConversationOk { request_id: 3 } => break,
            _ => continue,
        }
    }
    let ids: Vec<_> = list(&mut conn).await.into_iter().map(|c| c.id).collect();
    assert_eq!(ids, ["other"]);
}
