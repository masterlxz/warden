mod support;

use serde_json::json;
use support::{spin_up_server, MockProvider};
use warden_core::tool::ToolSpec;
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
