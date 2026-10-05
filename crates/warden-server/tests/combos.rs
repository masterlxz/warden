//! P90 on the hub: an agent whose default model is a combo — the combo's first provider answers
//! HTTP 503, the second answers — gets the second one's reply and the fallback notice. The
//! providers are real `openai_compatible` ones pointed at two tiny local HTTP servers, so this goes
//! through `build_model_for`, the real OpenAI client and the typed `ProviderHttpError`.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use warden_bootstrap::{save_config, AgentConfig, ComboConfig, FileConfig, Provider, ProviderConfig};
use warden_core::memory::Vault;
use warden_core::model::{ChatStream, Message, ModelProvider};
use warden_core::orchestrator::Orchestrator;
use warden_core::tool::ToolSpec;
use warden_server::{ClientMessage, Server, ServerConnection, ServerMessage, SettingsHost};

/// Answers every request with `response` (a whole HTTP/1.1 response), then closes.
async fn fake_http(response: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else { return };
            tokio::spawn(async move {
                let mut buf = vec![0u8; 64 * 1024];
                let _ = stream.read(&mut buf).await;
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.shutdown().await;
            });
        }
    });
    format!("http://{addr}/v1")
}

const BUSY: &str = "HTTP/1.1 503 Service Unavailable\r\ncontent-type: application/json\r\ncontent-length: 22\r\nconnection: close\r\n\r\n{\"error\":\"overloaded\"}";
const OK: &str = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\ndata: {\"choices\":[{\"delta\":{\"content\":\"from the spare\"}}]}\n\ndata: [DONE]\n\n";

/// The chat itself must never reach this: the agent's own model (the combo) answers.
struct Unused;

#[async_trait]
impl ModelProvider for Unused {
    async fn chat_stream(&self, _messages: Vec<Message>, _tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
        anyhow::bail!("the hub's default model was used instead of the agent's combo")
    }
}

struct TestHost {
    path: PathBuf,
}

#[async_trait]
impl SettingsHost for TestHost {
    fn config_path(&self) -> PathBuf {
        self.path.clone()
    }

    async fn build(&self) -> anyhow::Result<Orchestrator> {
        anyhow::bail!("not used by this test")
    }
}

fn provider(id: &str, base_url: String) -> ProviderConfig {
    ProviderConfig { id: id.into(), kind: Provider::OpenaiCompatible, api_key: Some("x".into()), base_url: Some(base_url), model: Some(format!("{id}-model")), node: None }
}

#[tokio::test]
async fn an_agent_on_a_combo_gets_the_next_provider_when_the_first_is_down() {
    let dir = std::env::temp_dir().join(format!(
        "warden-server-combos-{}",
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let config_path = dir.join("config.toml");
    let config = FileConfig {
        providers: vec![provider("busy", fake_http(BUSY).await), provider("spare", fake_http(OK).await)],
        combos: vec![ComboConfig { id: "fast".into(), providers: vec!["busy".into(), "spare".into()] }],
        agents: vec![AgentConfig {
            id: "helper".into(),
            persona: "You help.".into(),
            provider_id: Some("fast".into()),
            can_delegate_to_agents: false,
            can_manage_agents: false,
            can_message_agents: false,
            can_manage_tasks: false,
            allowed_tools: None,
            autonomy: warden_bootstrap::default_autonomy(),
            approval_required: Vec::new(),
            role: None,
            reports_to: None,
            owner: None,
            shared_with: Vec::new(),
        }],
        ..FileConfig::default()
    };
    save_config(&config_path, &config).unwrap();

    let orchestrator = Orchestrator::new(Arc::new(Unused), Arc::new(Vault::new(dir.join("vault"))));
    let server = Server::bind("127.0.0.1:0".parse().unwrap(), "test-key", "Test Hub", Arc::new(orchestrator), dir.join("conversations"), dir.join("devices.json"))
        .await
        .unwrap()
        .with_settings(Arc::new(TestHost { path: config_path }));
    let addr = server.local_addr().unwrap();
    tokio::spawn(server.serve());

    let mut conn = ServerConnection::connect(&format!("ws://{addr}"), "web-1", "Browser", "test-key").await.unwrap();
    conn.send(&ClientMessage::Chat { message: "hi".into(), conversation_id: Some("c1".into()), attachments: Vec::new(), agent_id: Some("helper".into()), project_id: None, workdir: None })
        .await
        .unwrap();
    match conn.recv().await.unwrap() {
        Some(ServerMessage::ChatResponse { content, fallbacks, .. }) => {
            assert_eq!(content, "from the spare");
            assert_eq!(fallbacks.len(), 1);
            assert_eq!((fallbacks[0].from.as_str(), fallbacks[0].to.as_str(), fallbacks[0].model.as_str()), ("busy", "spare", "spare-model"));
            assert!(fallbacks[0].reason.starts_with("503"), "{}", fallbacks[0].reason);
        }
        other => panic!("expected ChatResponse, got {other:?}"),
    }
}
