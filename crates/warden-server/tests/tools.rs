mod support;

use std::sync::Arc;

use serde_json::json;
use support::{spin_up_server, spin_up_server_with_base_tool, MockProvider};
use warden_core::tool::{Tool, ToolSpec};
use warden_server::{ClientMessage, ServerConnection, ServerMessage};

fn list_files_spec() -> ToolSpec {
    ToolSpec {
        name: "list_files".to_string(),
        description: "Lists files under the configured root folder".to_string(),
        parameters: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
    }
}

/// End-to-end proof of the Fase 7.4 wiring: a client advertises a tool in `Hello`, the server
/// registers a `RemoteTool` proxy for it on this connection's `Orchestrator`, the (mocked) model
/// calls it mid-turn, the request reaches the client as `ToolCallRequest`, the client's real
/// answer comes back as `ToolCallResult`, and the final `ChatResponse` reflects it. `RemoteTool`'s
/// own mechanics (timeouts, errors, dropped connections) are already covered by
/// `src/remote_tool.rs`'s unit tests — this test is only about the connection wiring around it.
#[tokio::test]
async fn a_chat_that_calls_an_advertised_client_tool_gets_the_real_result_back() {
    let provider = MockProvider::calling_tool_then_replying("list_files", json!({"path": ""}));
    let addr = spin_up_server(provider).await;

    let mut conn = ServerConnection::connect_with_tools(
        &format!("ws://{addr}"),
        "dev-1",
        "Test Device",
        "test-key",
        vec![list_files_spec()],
    )
    .await
    .unwrap();

    conn.send(&ClientMessage::Chat { message: "list my files".to_string() }).await.unwrap();

    match conn.recv().await.unwrap() {
        Some(ServerMessage::ToolCallRequest { call_id, tool, arguments }) => {
            assert_eq!(tool, "list_files");
            assert_eq!(arguments, json!({"path": ""}));
            conn.send(&ClientMessage::ToolCallResult { call_id, result: json!({"files": ["a.txt", "b.txt"]}) })
                .await
                .unwrap();
        }
        other => panic!("expected ToolCallRequest, got {other:?}"),
    }

    match conn.recv().await.unwrap() {
        Some(ServerMessage::ChatResponse { content, .. }) => {
            assert!(content.contains(r#""files":["a.txt","b.txt"]"#), "content was: {content}");
        }
        other => panic!("expected ChatResponse, got {other:?}"),
    }
}

/// A client that connects without advertising any tools (Fase 7.2/7.3's plain `connect`) must
/// keep working exactly as before — the per-connection `Orchestrator` branch in `server.rs` is
/// skipped entirely when `Hello.tools` is empty.
#[tokio::test]
async fn a_client_with_no_advertised_tools_still_gets_a_plain_chat_response() {
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

/// A tool already registered on the shared `Orchestrator` (stands in for the vault's own
/// `read_file`, the tool a real phone client once collided with in the case that motivated P42) —
/// used to prove a client's `Hello.tools` entry of the same name gets namespaced, not dropped or
/// left to shadow/be shadowed silently.
struct FakeServerSideTool;
#[async_trait::async_trait]
impl Tool for FakeServerSideTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec { name: "read_file".to_string(), description: "the server's own tool".to_string(), parameters: json!({"type": "object"}) }
    }
    async fn call(&self, _args: serde_json::Value) -> anyhow::Result<serde_json::Value> {
        Ok(json!({"content": "server-side content — should never be reached by this test"}))
    }
}

/// P42: a client that advertises a tool whose name collides with one the shared `Orchestrator`
/// already has gets namespaced (`{device_id}__{tool}`), not dropped or left to silently shadow the
/// server's own tool — and the wire request that reaches the client still names the tool the way
/// the client itself advertised it, because `RemoteTool` sends from its own internal spec,
/// untouched by the rename wrapper.
#[tokio::test]
async fn a_client_tool_colliding_with_a_server_tool_is_renamed_not_dropped() {
    let provider = MockProvider::calling_tool_then_replying("dev-1__read_file", json!({"path": "notes.md"}));
    let addr = spin_up_server_with_base_tool(provider, Arc::new(FakeServerSideTool)).await;

    let mut conn = ServerConnection::connect_with_tools(
        &format!("ws://{addr}"),
        "dev-1",
        "Test Device",
        "test-key",
        vec![ToolSpec {
            name: "read_file".to_string(),
            description: "Reads a file from the phone's chosen root folder".to_string(),
            parameters: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
        }],
    )
    .await
    .unwrap();

    conn.send(&ClientMessage::Chat { message: "read my notes".to_string() }).await.unwrap();

    match conn.recv().await.unwrap() {
        Some(ServerMessage::ToolCallRequest { call_id, tool, arguments }) => {
            // The client only ever advertised "read_file" — it must never be asked for the
            // namespaced name, which only exists inside the server's own dispatch.
            assert_eq!(tool, "read_file");
            assert_eq!(arguments, json!({"path": "notes.md"}));
            conn.send(&ClientMessage::ToolCallResult { call_id, result: json!({"content": "the real phone content"}) })
                .await
                .unwrap();
        }
        other => panic!("expected ToolCallRequest, got {other:?}"),
    }

    match conn.recv().await.unwrap() {
        Some(ServerMessage::ChatResponse { content, .. }) => {
            assert!(content.contains("the real phone content"), "content was: {content}");
        }
        other => panic!("expected ChatResponse, got {other:?}"),
    }
}

/// The companion regression case: a client tool that does *not* collide with anything keeps its
/// bare name (`dedupe_tool_name` only renames on a real collision) — same shape as the already
///-existing `a_chat_that_calls_an_advertised_client_tool_gets_the_real_result_back`, just against a
/// server that also has a base tool of a *different* name, to prove that alone doesn't trigger a
/// rename.
#[tokio::test]
async fn a_client_tool_with_no_collision_keeps_its_bare_name() {
    let provider = MockProvider::calling_tool_then_replying("list_files", json!({"path": ""}));
    let addr = spin_up_server_with_base_tool(provider, Arc::new(FakeServerSideTool)).await;

    let mut conn = ServerConnection::connect_with_tools(
        &format!("ws://{addr}"),
        "dev-1",
        "Test Device",
        "test-key",
        vec![list_files_spec()],
    )
    .await
    .unwrap();

    conn.send(&ClientMessage::Chat { message: "list my files".to_string() }).await.unwrap();

    match conn.recv().await.unwrap() {
        Some(ServerMessage::ToolCallRequest { call_id, tool, arguments }) => {
            assert_eq!(tool, "list_files");
            assert_eq!(arguments, json!({"path": ""}));
            conn.send(&ClientMessage::ToolCallResult { call_id, result: json!({"files": ["a.txt"]}) }).await.unwrap();
        }
        other => panic!("expected ToolCallRequest, got {other:?}"),
    }

    match conn.recv().await.unwrap() {
        Some(ServerMessage::ChatResponse { content, .. }) => {
            assert!(content.contains(r#""files":["a.txt"]"#), "content was: {content}");
        }
        other => panic!("expected ChatResponse, got {other:?}"),
    }
}
