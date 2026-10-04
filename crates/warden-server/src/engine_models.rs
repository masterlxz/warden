//! The code engine's route to the hub's model (P103 b): a listener on the loopback, for the opencode alone.
//!
//! The opencode needs a model, and the hub already has the providers, their keys, the fallback and the spending
//! limits, so it is pointed at the hub — through a door of its own (`openai_api::serve_engine`), not the Warden API:
//! - the API redirects to `https` on a hub with TLS, and the opencode wouldn't trust the certificate; this listener is
//!   plain HTTP and never leaves the machine (`127.0.0.1`);
//! - the API speaks as the Warden — its tools, its persona and the owner's notes as context — and here the model gets
//!   none of that, only the opencode's own tools and prompt;
//! - the credential is a random token made at start-up and held in memory, handed to the opencode in its environment:
//!   no key in the API's list for anyone to copy, nothing on disk.

use std::sync::Arc;

use serde_json::json;
use tokio::net::TcpListener;
use warden_core::memory::Vault;

use crate::openai_api::{serve_engine, EngineRoute};
use crate::settings::SharedOrchestrator;
use crate::web_ui::{read_request_head, HEAD_TIMEOUT};

/// Where the opencode finds the hub's model, and the key it must show.
pub struct EngineModels {
    /// `http://127.0.0.1:<port>/v1`.
    pub base_url: String,
    pub token: String,
}

impl EngineModels {
    /// Starts listening. The orchestrator is the hub's shared one, so a change in its settings (another provider) reaches
    /// the opencode's next request.
    pub async fn start(orchestrator: SharedOrchestrator) -> anyhow::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let base_url = format!("http://{}/v1", listener.local_addr()?);
        let token = warden_bootstrap::generate_auth_key();
        let empty_vault = std::env::temp_dir().join(format!("warden-engine-no-notes-{}", std::process::id()));
        let route = EngineRoute { orchestrator, token: Arc::from(token.as_str()), empty_vault: Arc::new(Vault::new(empty_vault)) };
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else { return };
                let route = route.clone();
                tokio::spawn(async move {
                    let Ok(Ok(Some(head))) = tokio::time::timeout(HEAD_TIMEOUT, read_request_head(&mut stream)).await else { return };
                    let _ = serve_engine(&mut stream, &head, &route).await;
                });
            }
        });
        Ok(Self { base_url, token })
    }

    /// The opencode's own settings (`OPENCODE_CONFIG_CONTENT`): the hub's model as its only provider, and for everything
    /// it would otherwise pick for itself — titles included — so nothing the person types goes to a provider they didn't
    /// give it. No sharing, no self-update.
    pub fn opencode_config(&self) -> String {
        json!({
            "$schema": "https://opencode.ai/config.json",
            "autoupdate": false,
            "share": "disabled",
            "enabled_providers": ["warden"],
            "model": "warden/warden",
            "small_model": "warden/warden",
            "provider": {
                "warden": {
                    "npm": "@ai-sdk/openai-compatible",
                    "name": "Warden",
                    "options": { "baseURL": self.base_url, "apiKey": self.token },
                    "models": { "warden": { "name": "Warden" } }
                }
            }
        })
        .to_string()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::time::Duration;

    use async_trait::async_trait;
    use serde_json::{json, Value};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use warden_core::model::{response_stream, ChatStream, Message, ModelProvider, Response, ToolCall};
    use warden_core::orchestrator::Orchestrator;
    use warden_core::tool::file_tools::WriteFileTool;
    use warden_core::tool::ToolSpec;

    use super::*;

    /// What the model was offered and told, request by request: the tool names, then the messages' text.
    type Seen = Arc<Mutex<Vec<(Vec<String>, Vec<String>)>>>;

    /// Remembers what it was offered and told, and asks for the client's tool `edit` when the request says so.
    struct Spy {
        seen: Seen,
    }
    #[async_trait]
    impl ModelProvider for Spy {
        async fn chat_stream(&self, messages: Vec<Message>, tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
            let told: Vec<String> = messages.iter().map(|m| m.content.clone()).collect();
            self.seen.lock().unwrap().push((tools.iter().map(|t| t.name.clone()).collect(), told));
            let last = messages.last().unwrap();
            if last.content.contains("EDIT") && last.role != warden_core::model::Role::Tool {
                let call = ToolCall { id: "c1".into(), name: "edit".into(), arguments: json!({"file": "a.rs"}), thought_signature: None };
                return Ok(response_stream(Response { content: String::new(), tool_calls: vec![call], usage: None }));
            }
            Ok(response_stream(Response { content: "from the hub's model".into(), tool_calls: Vec::new(), usage: None }))
        }
    }

    async fn post(base: &str, token: Option<&str>, path: &str, body: &Value) -> (u16, Value) {
        let addr = base.trim_start_matches("http://").split('/').next().unwrap();
        let mut socket = tokio::net::TcpStream::connect(addr).await.unwrap();
        let body = body.to_string();
        let auth = token.map(|t| format!("Authorization: Bearer {t}\r\n")).unwrap_or_default();
        let method = if body == "null" { "GET" } else { "POST" };
        let request = format!("{method} {path} HTTP/1.1\r\nHost: x\r\n{auth}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", if method == "GET" { 0 } else { body.len() }, if method == "GET" { "" } else { &body });
        socket.write_all(request.as_bytes()).await.unwrap();
        let mut raw = String::new();
        tokio::time::timeout(Duration::from_secs(10), socket.read_to_string(&mut raw)).await.unwrap().unwrap();
        let status = raw.split(' ').nth(1).unwrap().parse().unwrap();
        let json = raw.split_once("\r\n\r\n").map(|(_, b)| b).and_then(|b| serde_json::from_str(b.trim()).ok()).unwrap_or(Value::Null);
        (status, json)
    }

    async fn start() -> (EngineModels, Seen, std::path::PathBuf) {
        let seen = Arc::default();
        let dir = std::env::temp_dir().join(format!("warden-engine-models-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        // The owner's vault has a note, and the hub's orchestrator has a tool that writes to it: neither may reach the engine.
        let vault = Arc::new(Vault::new(dir.join("vault")));
        vault.write("secrets.md", "the owner's secret plans for EDIT").unwrap();
        let mut orchestrator = Orchestrator::new(Arc::new(Spy { seen: Arc::clone(&seen) }), vault.clone());
        orchestrator.register_tool(Arc::new(WriteFileTool::new(vault)));
        (EngineModels::start(SharedOrchestrator::new(orchestrator)).await.unwrap(), seen, dir)
    }

    fn ask(text: &str) -> Value {
        json!({"model": "warden/warden", "messages": [{"role": "system", "content": "You are the opencode."}, {"role": "user", "content": text}],
               "tools": [{"type": "function", "function": {"name": "edit", "description": "edit a file", "parameters": {"type": "object"}}}]})
    }

    #[tokio::test]
    async fn the_engine_gets_the_hubs_model_alone_with_its_own_tools_and_no_notes_and_nobody_else_gets_in() {
        let (models, seen, _dir) = start().await;
        assert!(models.base_url.starts_with("http://127.0.0.1:") && models.base_url.ends_with("/v1"));

        // Without the token, or with another, nothing is answered.
        assert_eq!(post(&models.base_url, None, "/v1/chat/completions", &ask("hi")).await.0, 401);
        assert_eq!(post(&models.base_url, Some("not-it"), "/v1/chat/completions", &ask("hi")).await.0, 401);
        assert!(seen.lock().unwrap().is_empty(), "the model was never reached");

        let (status, answer) = post(&models.base_url, Some(&models.token), "/v1/chat/completions", &ask("hello")).await;
        assert_eq!(status, 200);
        assert_eq!(answer["choices"][0]["message"]["content"], "from the hub's model");
        let (tools, told) = seen.lock().unwrap().last().unwrap().clone();
        assert_eq!(tools, ["edit"], "only the engine's own tool: the hub's write_file is not offered");
        assert_eq!(told, ["You are the opencode.", "hello"], "the engine's prompt and nothing else: no persona, no notes");

        // A tool call of the engine's goes back to the engine to run, as the OpenAI format has it.
        let (_, call) = post(&models.base_url, Some(&models.token), "/v1/chat/completions", &ask("please EDIT it")).await;
        assert_eq!(call["choices"][0]["finish_reason"], "tool_calls");
        assert_eq!(call["choices"][0]["message"]["tool_calls"][0]["function"]["name"], "edit");
        // This question matches the owner's note word for word, and the model still wasn't shown it.
        let (_, told) = seen.lock().unwrap().last().unwrap().clone();
        assert!(!told.iter().any(|m| m.contains("secret")), "{told:?}");

        let (status, models_list) = post(&models.base_url, Some(&models.token), "/v1/models", &Value::Null).await;
        assert_eq!((status, models_list["data"][0]["id"].as_str()), (200, Some("warden")));
        assert_eq!(post(&models.base_url, Some(&models.token), "/v1/other", &json!({})).await.0, 404);
    }

    #[tokio::test]
    async fn the_opencodes_settings_make_the_hub_its_only_model_and_switch_off_what_would_leave_the_machine() {
        let (models, _, _) = start().await;
        let config: Value = serde_json::from_str(&models.opencode_config()).unwrap();
        assert_eq!(config["model"], "warden/warden");
        assert_eq!(config["small_model"], "warden/warden", "titles and the like go to the hub too, not to a provider of its choosing");
        assert_eq!(config["enabled_providers"], json!(["warden"]));
        assert_eq!((config["share"].as_str(), config["autoupdate"].as_bool()), (Some("disabled"), Some(false)));
        let provider = &config["provider"]["warden"];
        assert_eq!(provider["options"]["baseURL"], models.base_url.as_str());
        assert_eq!(provider["options"]["apiKey"], models.token.as_str());
        assert_eq!(provider["npm"], "@ai-sdk/openai-compatible");
    }
}
