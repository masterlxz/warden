mod support;

use std::time::Duration;

use support::{spin_up_server, MockProvider};
use warden_server::{ClientMessage, ServerConnection, ServerMessage};
use warden_server_protocol::protocol::HistoryRole;

#[tokio::test]
async fn chat_message_gets_answered_by_the_hosted_orchestrator() {
    let addr = spin_up_server(MockProvider::replying("ahoy")).await;
    let mut conn = ServerConnection::connect(&format!("ws://{addr}"), "dev-1", "Test Device", "test-key")
        .await
        .unwrap();

    conn.send(&ClientMessage::Chat { message: "hello".to_string() }).await.unwrap();

    match conn.recv().await.unwrap() {
        Some(ServerMessage::ChatResponse { content, .. }) => assert_eq!(content, "ahoy"),
        other => panic!("expected ChatResponse, got {other:?}"),
    }
}

#[tokio::test]
async fn a_failing_model_call_comes_back_as_a_chat_error_not_a_dropped_connection() {
    let addr = spin_up_server(MockProvider::failing()).await;
    let mut conn = ServerConnection::connect(&format!("ws://{addr}"), "dev-1", "Test Device", "test-key")
        .await
        .unwrap();

    conn.send(&ClientMessage::Chat { message: "hello".to_string() }).await.unwrap();

    match conn.recv().await.unwrap() {
        Some(ServerMessage::ChatError { message }) => assert!(message.contains("mock provider failure"), "message was: {message}"),
        other => panic!("expected ChatError, got {other:?}"),
    }

    // The connection itself must still be usable after an error.
    conn.ping(7).await.unwrap();
    assert!(matches!(conn.recv().await.unwrap(), Some(ServerMessage::Pong { nonce: 7 })));
}

/// P40: a device that reconnects gets back the turns it already had, from the same file `Chat`
/// writes to — and only its own conversation, not another device's.
#[tokio::test]
async fn a_reconnecting_device_can_fetch_its_conversation_history() {
    let addr = spin_up_server(MockProvider::replying("ahoy")).await;
    let url = format!("ws://{addr}");

    let mut first = ServerConnection::connect(&url, "dev-1", "Test Device", "test-key").await.unwrap();
    first.send(&ClientMessage::Chat { message: "hello".to_string() }).await.unwrap();
    assert!(matches!(first.recv().await.unwrap(), Some(ServerMessage::ChatResponse { .. })));
    drop(first);

    let mut again = ServerConnection::connect(&url, "dev-1", "Test Device", "test-key").await.unwrap();
    again.send(&ClientMessage::RequestHistory { request_id: 1, limit: None }).await.unwrap();
    match again.recv().await.unwrap() {
        Some(ServerMessage::History { request_id, messages }) => {
            assert_eq!(request_id, 1);
            let turns: Vec<_> = messages.iter().map(|m| (m.role, m.content.as_str())).collect();
            assert_eq!(turns, vec![(HistoryRole::User, "hello"), (HistoryRole::Assistant, "ahoy")]);
        }
        other => panic!("expected History, got {other:?}"),
    }

    let mut other_device = ServerConnection::connect(&url, "dev-2", "Other Device", "test-key").await.unwrap();
    other_device.send(&ClientMessage::RequestHistory { request_id: 2, limit: None }).await.unwrap();
    assert_eq!(other_device.recv().await.unwrap(), Some(ServerMessage::History { request_id: 2, messages: Vec::new() }));
}

/// Regression test for the reader/writer-task split in `server.rs`: a slow `Chat` call used to
/// block the whole connection's read loop, so a `Ping` sent while it was in flight wouldn't get
/// a `Pong` until the chat call finished — long enough to trip the client's real heartbeat
/// timeout. This proves a `Ping` sent mid-`Chat` is answered well before the slow reply lands.
#[tokio::test]
async fn a_ping_sent_during_a_slow_chat_call_is_answered_immediately() {
    let addr = spin_up_server(MockProvider::replying_after("slow reply", Duration::from_secs(2))).await;
    let mut conn = ServerConnection::connect(&format!("ws://{addr}"), "dev-1", "Test Device", "test-key")
        .await
        .unwrap();

    conn.send(&ClientMessage::Chat { message: "hello".to_string() }).await.unwrap();
    // Give the server a moment to have actually started the (slow) chat call before pinging.
    tokio::time::sleep(Duration::from_millis(100)).await;
    conn.ping(99).await.unwrap();

    let first = tokio::time::timeout(Duration::from_millis(500), conn.recv()).await.expect("Pong took too long — the reader loop was blocked by the in-flight Chat call").unwrap();
    assert!(matches!(first, Some(ServerMessage::Pong { nonce: 99 })), "expected Pong first, got {first:?}");

    let second = conn.recv().await.unwrap();
    assert!(matches!(second, Some(ServerMessage::ChatResponse { .. })), "expected ChatResponse second, got {second:?}");
}
