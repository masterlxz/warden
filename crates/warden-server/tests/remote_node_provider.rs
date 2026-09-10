mod support;

use base64::Engine;
use serde_json::json;
use support::{spin_up_server, MockProvider};
use warden_core::storage::StorageProvider;
use warden_server::{ClientMessage, RemoteNodeProvider, ServerConnection, ServerMessage};

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// End-to-end proof of the calling half of P61's `RemoteNodeProvider`: it connects to the same
/// `warden-server` a plain (no `Hello.tools`) fake target is already connected to, issues
/// `read()`, and the real base64-decoded bytes from the scripted target reply come back.
#[tokio::test]
async fn read_round_trips_real_base64_decoded_content_from_the_target() {
    let addr = spin_up_server(MockProvider::replying("unused")).await;
    let mut target = ServerConnection::connect(&format!("ws://{addr}"), "dev-target", "Target Device", "test-key").await.unwrap();

    let provider = RemoteNodeProvider::connect(&format!("ws://{addr}"), "dev-caller", "Caller Device", "test-key", "dev-target").await.unwrap();

    let read = tokio::spawn(async move { provider.read("notes/a.md").await });

    match target.recv().await.unwrap() {
        Some(ServerMessage::ToolCallRequest { call_id, tool, arguments }) => {
            assert_eq!(tool, "vault_read");
            assert_eq!(arguments, json!({"path": "notes/a.md"}));
            target
                .send(&ClientMessage::ToolCallResult { call_id, result: json!({"content_base64": b64(b"buy milk")}) })
                .await
                .unwrap();
        }
        other => panic!("expected ToolCallRequest, got {other:?}"),
    }

    assert_eq!(read.await.unwrap().unwrap(), b"buy milk");
}

/// `write` sends the content base64-encoded in the args the target sees; a real fake target
/// stores it and a follow-up `read()` against the same path proves the encoding round-trips both
/// directions, not just that *some* request/response pair was exchanged.
#[tokio::test]
async fn write_then_read_round_trips_through_a_scripted_target() {
    let addr = spin_up_server(MockProvider::replying("unused")).await;
    let mut target = ServerConnection::connect(&format!("ws://{addr}"), "dev-target", "Target Device", "test-key").await.unwrap();

    let provider = RemoteNodeProvider::connect(&format!("ws://{addr}"), "dev-caller", "Caller Device", "test-key", "dev-target").await.unwrap();

    let write = tokio::spawn(async move {
        provider.write("notes/a.md", b"buy milk").await.unwrap();
        provider
    });

    let written_content_base64 = match target.recv().await.unwrap() {
        Some(ServerMessage::ToolCallRequest { call_id, tool, arguments }) => {
            assert_eq!(tool, "vault_write");
            assert_eq!(arguments["path"], json!("notes/a.md"));
            target.send(&ClientMessage::ToolCallResult { call_id, result: json!({}) }).await.unwrap();
            arguments["content_base64"].as_str().unwrap().to_string()
        }
        other => panic!("expected ToolCallRequest, got {other:?}"),
    };
    assert_eq!(written_content_base64, b64(b"buy milk"));

    let provider = write.await.unwrap();
    let read = tokio::spawn(async move { provider.read("notes/a.md").await });
    match target.recv().await.unwrap() {
        Some(ServerMessage::ToolCallRequest { call_id, tool, .. }) => {
            assert_eq!(tool, "vault_read");
            target
                .send(&ClientMessage::ToolCallResult { call_id, result: json!({"content_base64": written_content_base64}) })
                .await
                .unwrap();
        }
        other => panic!("expected ToolCallRequest, got {other:?}"),
    }
    assert_eq!(read.await.unwrap().unwrap(), b"buy milk");
}

#[tokio::test]
async fn list_decodes_the_paths_array_from_the_target() {
    let addr = spin_up_server(MockProvider::replying("unused")).await;
    let mut target = ServerConnection::connect(&format!("ws://{addr}"), "dev-target", "Target Device", "test-key").await.unwrap();

    let provider = RemoteNodeProvider::connect(&format!("ws://{addr}"), "dev-caller", "Caller Device", "test-key", "dev-target").await.unwrap();
    let list = tokio::spawn(async move { provider.list().await });

    match target.recv().await.unwrap() {
        Some(ServerMessage::ToolCallRequest { call_id, tool, arguments }) => {
            assert_eq!(tool, "vault_list");
            assert_eq!(arguments, json!({}));
            target
                .send(&ClientMessage::ToolCallResult { call_id, result: json!({"paths": ["a.md", "nested/b.md"]}) })
                .await
                .unwrap();
        }
        other => panic!("expected ToolCallRequest, got {other:?}"),
    }

    assert_eq!(list.await.unwrap().unwrap(), vec!["a.md".to_string(), "nested/b.md".to_string()]);
}

#[tokio::test]
async fn delete_sends_the_right_path_and_resolves_on_an_empty_result() {
    let addr = spin_up_server(MockProvider::replying("unused")).await;
    let mut target = ServerConnection::connect(&format!("ws://{addr}"), "dev-target", "Target Device", "test-key").await.unwrap();

    let provider = RemoteNodeProvider::connect(&format!("ws://{addr}"), "dev-caller", "Caller Device", "test-key", "dev-target").await.unwrap();
    let delete = tokio::spawn(async move { provider.delete("notes/a.md").await });

    match target.recv().await.unwrap() {
        Some(ServerMessage::ToolCallRequest { call_id, tool, arguments }) => {
            assert_eq!(tool, "vault_delete");
            assert_eq!(arguments, json!({"path": "notes/a.md"}));
            target.send(&ClientMessage::ToolCallResult { call_id, result: json!({}) }).await.unwrap();
        }
        other => panic!("expected ToolCallRequest, got {other:?}"),
    }

    delete.await.unwrap().unwrap();
}

/// The target's own `ToolCallError` reply surfaces as a real error carrying its message — same
/// wire path `device_routing.rs` already proves at the protocol level, exercised here through the
/// `StorageProvider` trait instead.
#[tokio::test]
async fn the_targets_own_failure_surfaces_as_a_real_error() {
    let addr = spin_up_server(MockProvider::replying("unused")).await;
    let mut target = ServerConnection::connect(&format!("ws://{addr}"), "dev-target", "Target Device", "test-key").await.unwrap();

    let provider = RemoteNodeProvider::connect(&format!("ws://{addr}"), "dev-caller", "Caller Device", "test-key", "dev-target").await.unwrap();
    let read = tokio::spawn(async move { provider.read("missing.md").await });

    match target.recv().await.unwrap() {
        Some(ServerMessage::ToolCallRequest { call_id, .. }) => {
            target.send(&ClientMessage::ToolCallError { call_id, message: "file not found".to_string() }).await.unwrap();
        }
        other => panic!("expected ToolCallRequest, got {other:?}"),
    }

    let err = read.await.unwrap().unwrap_err();
    assert!(err.to_string().contains("file not found"), "error was: {err}");
}

/// A target that replies with a shape that doesn't match the documented contract (missing
/// `content_base64`) fails clearly instead of panicking.
#[tokio::test]
async fn a_malformed_reply_errors_clearly_instead_of_panicking() {
    let addr = spin_up_server(MockProvider::replying("unused")).await;
    let mut target = ServerConnection::connect(&format!("ws://{addr}"), "dev-target", "Target Device", "test-key").await.unwrap();

    let provider = RemoteNodeProvider::connect(&format!("ws://{addr}"), "dev-caller", "Caller Device", "test-key", "dev-target").await.unwrap();
    let read = tokio::spawn(async move { provider.read("notes/a.md").await });

    match target.recv().await.unwrap() {
        Some(ServerMessage::ToolCallRequest { call_id, .. }) => {
            target.send(&ClientMessage::ToolCallResult { call_id, result: json!({}) }).await.unwrap();
        }
        other => panic!("expected ToolCallRequest, got {other:?}"),
    }

    let err = read.await.unwrap().unwrap_err();
    assert!(err.to_string().contains("content_base64"), "error was: {err}");
}
