//! The target side of P61's `RemoteNodeProvider` (`crates/warden-server-protocol`) — a real
//! process (the `warden-node` binary on top of this module) that connects to a `warden-server`
//! hub as a client, advertises `vault_read`/`vault_write`/`vault_list`/`vault_delete`, and serves
//! them against its own local `Vault`. No new file-I/O logic here — every operation is a thin JSON
//! wrapper around `warden_core::storage::LocalFSProvider`, the same primitives
//! `RemoteNodeProvider` already documents as the wire contract.

use std::sync::Arc;

use base64::Engine;
use serde_json::{json, Value};
use warden_core::memory::Vault;
use warden_core::storage::{LocalFSProvider, StorageProvider};
use warden_core::tool::ToolSpec;
use warden_server_protocol::{ClientMessage, ServerConnection, ServerMessage};

/// The 4 tools this node advertises in `Hello` — schemas match exactly what `RemoteNodeProvider`
/// sends (see its own doc comment for the wire contract).
pub fn tool_specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "vault_read".to_string(),
            description: "Reads one file from this node's vault, base64-encoded.".to_string(),
            parameters: json!({"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}),
        },
        ToolSpec {
            name: "vault_write".to_string(),
            description: "Writes (creating or overwriting) one file in this node's vault, content base64-encoded.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {"path": {"type": "string"}, "content_base64": {"type": "string"}},
                "required": ["path", "content_base64"]
            }),
        },
        ToolSpec {
            name: "vault_list".to_string(),
            description: "Lists every file's relative path in this node's vault.".to_string(),
            parameters: json!({"type": "object", "properties": {}}),
        },
        ToolSpec {
            name: "vault_delete".to_string(),
            description: "Deletes one file from this node's vault.".to_string(),
            parameters: json!({"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}),
        },
    ]
}

/// Connects to `url` advertising this node's 4 vault tools — the Hello/HelloAck handshake is
/// `ServerConnection::connect_with_tools`'s job, unchanged from Fase 7.4's own use of it.
pub async fn connect(url: &str, device_id: &str, device_name: &str, auth_key: &str) -> anyhow::Result<ServerConnection> {
    ServerConnection::connect_with_tools(url, device_id, device_name, auth_key, tool_specs()).await
}

/// Serves `ToolCallRequest`s against `vault` until the connection closes (cleanly or on error) —
/// call this from its own task if the caller needs to do anything else concurrently. This
/// connection never sends `Chat`, so any message besides `ToolCallRequest` is unexpected and
/// ignored rather than treated as an error.
pub async fn serve(mut conn: ServerConnection, vault: Arc<Vault>) -> anyhow::Result<()> {
    let provider = LocalFSProvider::new(vault);

    loop {
        let Some(msg) = conn.recv().await? else { break };
        let ServerMessage::ToolCallRequest { call_id, tool, arguments } = msg else { continue };

        let reply = match handle_call(&provider, &tool, arguments).await {
            Ok(result) => ClientMessage::ToolCallResult { call_id, result },
            Err(err) => ClientMessage::ToolCallError { call_id, message: format!("{err:#}") },
        };
        conn.send(&reply).await?;
    }

    Ok(())
}

async fn handle_call(provider: &LocalFSProvider, tool: &str, arguments: Value) -> anyhow::Result<Value> {
    match tool {
        "vault_read" => {
            let path = require_path(&arguments)?;
            let bytes = provider.read(path).await?;
            Ok(json!({"content_base64": base64::engine::general_purpose::STANDARD.encode(bytes)}))
        }
        "vault_write" => {
            let path = require_path(&arguments)?;
            let content_b64 = arguments
                .get("content_base64")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("vault_write requires 'content_base64'"))?;
            let bytes = base64::engine::general_purpose::STANDARD.decode(content_b64)?;
            provider.write(path, &bytes).await?;
            Ok(json!({}))
        }
        "vault_list" => {
            let paths = provider.list().await?;
            Ok(json!({"paths": paths}))
        }
        "vault_delete" => {
            let path = require_path(&arguments)?;
            provider.delete(path).await?;
            Ok(json!({}))
        }
        other => anyhow::bail!("unknown tool '{other}'"),
    }
}

fn require_path(arguments: &Value) -> anyhow::Result<&str> {
    arguments.get("path").and_then(Value::as_str).ok_or_else(|| anyhow::anyhow!("this call requires a 'path'"))
}
