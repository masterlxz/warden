mod support;

use serde_json::json;
use support::{spin_up_server, MockProvider};
use warden_server::{ClientMessage, ServerConnection, ServerMessage};

/// End-to-end proof of Fase 9.3/9.4's device registry + cross-connection routing: device "a"
/// asks the server to run a tool on device "b" (a plain `connect`, no `Hello.tools` advertised —
/// any connected device is a valid routing target), "b" sees the request arrive as the same
/// `ToolCallRequest` shape Fase 7.4 already uses, answers for real, and "a" gets the result back
/// as `DeviceToolResult` correlated by its own `call_id`.
#[tokio::test]
async fn routes_a_call_to_the_correct_target_device_and_gets_the_real_result_back() {
    let addr = spin_up_server(MockProvider::replying("unused")).await;

    let mut a = ServerConnection::connect(&format!("ws://{addr}"), "dev-a", "Device A", "test-key").await.unwrap();
    let mut b = ServerConnection::connect(&format!("ws://{addr}"), "dev-b", "Device B", "test-key").await.unwrap();

    a.send(&ClientMessage::CallDeviceTool {
        call_id: 1,
        target_device_id: "dev-b".to_string(),
        tool: "vault_read".to_string(),
        arguments: json!({"path": "notes/a.md"}),
    })
    .await
    .unwrap();

    let request_call_id = match b.recv().await.unwrap() {
        Some(ServerMessage::ToolCallRequest { call_id, tool, arguments }) => {
            assert_eq!(tool, "vault_read");
            assert_eq!(arguments, json!({"path": "notes/a.md"}));
            call_id
        }
        other => panic!("expected ToolCallRequest on b's connection, got {other:?}"),
    };
    b.send(&ClientMessage::ToolCallResult { call_id: request_call_id, result: json!({"content": "buy milk"}) }).await.unwrap();

    match a.recv().await.unwrap() {
        Some(ServerMessage::DeviceToolResult { call_id, result }) => {
            assert_eq!(call_id, 1);
            assert_eq!(result, json!({"content": "buy milk"}));
        }
        other => panic!("expected DeviceToolResult on a's connection, got {other:?}"),
    }
}

/// The target device answering with `ToolCallError` (its side genuinely failed, e.g. file not
/// found) surfaces to the caller as `DeviceToolError` with the real message — same "carry the
/// real error text" posture as every other error variant in this protocol.
#[tokio::test]
async fn the_target_devices_own_failure_surfaces_back_to_the_caller() {
    let addr = spin_up_server(MockProvider::replying("unused")).await;

    let mut a = ServerConnection::connect(&format!("ws://{addr}"), "dev-a", "Device A", "test-key").await.unwrap();
    let mut b = ServerConnection::connect(&format!("ws://{addr}"), "dev-b", "Device B", "test-key").await.unwrap();

    a.send(&ClientMessage::CallDeviceTool {
        call_id: 1,
        target_device_id: "dev-b".to_string(),
        tool: "vault_read".to_string(),
        arguments: json!({"path": "missing.md"}),
    })
    .await
    .unwrap();

    let request_call_id = match b.recv().await.unwrap() {
        Some(ServerMessage::ToolCallRequest { call_id, .. }) => call_id,
        other => panic!("expected ToolCallRequest on b's connection, got {other:?}"),
    };
    b.send(&ClientMessage::ToolCallError { call_id: request_call_id, message: "file not found".to_string() }).await.unwrap();

    match a.recv().await.unwrap() {
        Some(ServerMessage::DeviceToolError { call_id, message }) => {
            assert_eq!(call_id, 1);
            assert!(message.contains("file not found"), "message was: {message}");
        }
        other => panic!("expected DeviceToolError on a's connection, got {other:?}"),
    }
}

/// Routing to a `target_device_id` that never connected (typo, or the node is simply offline)
/// fails immediately with a clear message instead of hanging until some timeout.
#[tokio::test]
async fn routing_to_a_device_that_isnt_connected_errors_immediately() {
    let addr = spin_up_server(MockProvider::replying("unused")).await;
    let mut a = ServerConnection::connect(&format!("ws://{addr}"), "dev-a", "Device A", "test-key").await.unwrap();

    a.send(&ClientMessage::CallDeviceTool {
        call_id: 1,
        target_device_id: "dev-ghost".to_string(),
        tool: "vault_read".to_string(),
        arguments: json!({}),
    })
    .await
    .unwrap();

    match a.recv().await.unwrap() {
        Some(ServerMessage::DeviceToolError { call_id, message }) => {
            assert_eq!(call_id, 1);
            assert!(message.contains("dev-ghost") && message.contains("not connected"), "message was: {message}");
        }
        other => panic!("expected DeviceToolError, got {other:?}"),
    }
}

/// A device that disconnects (`Goodbye`) stops being a valid routing target — a call issued
/// after that gets the same "not connected" error as one that was never online.
#[tokio::test]
async fn a_device_that_says_goodbye_is_deregistered_and_stops_being_routable() {
    let addr = spin_up_server(MockProvider::replying("unused")).await;

    let mut a = ServerConnection::connect(&format!("ws://{addr}"), "dev-a", "Device A", "test-key").await.unwrap();
    let mut b = ServerConnection::connect(&format!("ws://{addr}"), "dev-b", "Device B", "test-key").await.unwrap();
    b.send(&ClientMessage::Goodbye { reason: None }).await.unwrap();
    // Give the server a moment to process Goodbye and deregister "dev-b" before routing to it.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    a.send(&ClientMessage::CallDeviceTool {
        call_id: 1,
        target_device_id: "dev-b".to_string(),
        tool: "vault_read".to_string(),
        arguments: json!({}),
    })
    .await
    .unwrap();

    match a.recv().await.unwrap() {
        Some(ServerMessage::DeviceToolError { message, .. }) => {
            assert!(message.contains("not connected"), "message was: {message}");
        }
        other => panic!("expected DeviceToolError, got {other:?}"),
    }
}
