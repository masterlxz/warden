//! P120 on the hub: `EditAgentOrg` changes the organization of the agents from the tree, with the pairing key, and answers like a
//! settings save — the new settings — after the hub's orchestrator was started again with the change in place.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use warden_bootstrap::{load_config_from_path, save_config, AgentConfig, FileConfig};
use warden_core::memory::Vault;
use warden_core::model::{ChatStream, Message, ModelProvider};
use warden_core::orchestrator::Orchestrator;
use warden_core::tool::ToolSpec;
use warden_server::{ClientMessage, Server, ServerConnection, ServerMessage, SettingsHost};
use warden_server_protocol::protocol::AgentOrgEdit;

struct NoModel;

#[async_trait]
impl ModelProvider for NoModel {
    async fn chat_stream(&self, _messages: Vec<Message>, _tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
        anyhow::bail!("these tests never run a turn")
    }
}

/// Builds a fresh orchestrator from nothing, and counts how many times the hub asked for one.
struct Host {
    path: PathBuf,
    dir: PathBuf,
    builds: Arc<AtomicUsize>,
}

#[async_trait]
impl SettingsHost for Host {
    fn config_path(&self) -> PathBuf {
        self.path.clone()
    }

    async fn build(&self) -> anyhow::Result<Orchestrator> {
        self.builds.fetch_add(1, Ordering::SeqCst);
        Ok(Orchestrator::new(Arc::new(NoModel), Arc::new(Vault::new(self.dir.join("vault")))))
    }
}

fn agent(id: &str, boss: Option<&str>) -> AgentConfig {
    AgentConfig {
        id: id.into(),
        persona: format!("{id} persona"),
        provider_id: None,
        can_delegate_to_agents: false,
        can_manage_agents: false,
        can_message_agents: false,
        can_manage_tasks: false,
        allowed_tools: None,
        autonomy: warden_bootstrap::default_autonomy(),
        approval_required: Vec::new(),
        role: None,
        reports_to: boss.map(str::to_string),
        owner: None,
        shared_with: Vec::new(),
        delegation_models: Vec::new(),
        can_start_tasks: true,
        can_create_workers: true,
        can_message_user: true,
        can_choose_models: true,
    }
}

async fn spin_up() -> (String, PathBuf, Arc<AtomicUsize>) {
    spin_up_with(FileConfig { agents: vec![agent("chief", None), agent("lead", Some("chief")), agent("dev", Some("lead"))], ..FileConfig::default() }).await
}

async fn spin_up_with(config: FileConfig) -> (String, PathBuf, Arc<AtomicUsize>) {
    let dir = std::env::temp_dir().join(format!("warden-server-org-edit-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    save_config(&path, &config).unwrap();
    let builds = Arc::new(AtomicUsize::new(0));
    let orchestrator = Orchestrator::new(Arc::new(NoModel), Arc::new(Vault::new(dir.join("vault"))));
    let server = Server::bind("127.0.0.1:0".parse().unwrap(), "test-key", "Test Hub", Arc::new(orchestrator), dir.join("conversations"), dir.join("devices.json"))
        .await
        .unwrap()
        .with_settings(Arc::new(Host { path: path.clone(), dir, builds: builds.clone() }));
    let addr = server.local_addr().unwrap();
    tokio::spawn(server.serve());
    (format!("ws://{addr}"), path, builds)
}

async fn edit(conn: &mut ServerConnection, key: &str, edit: AgentOrgEdit) -> ServerMessage {
    conn.send(&ClientMessage::EditAgentOrg { request_id: 1, pairing_key: key.into(), edit }).await.unwrap();
    loop {
        match conn.recv().await.unwrap().expect("connection closed") {
            reply @ (ServerMessage::SettingsSaved { .. } | ServerMessage::SettingsError { .. }) => return reply,
            _ => continue,
        }
    }
}

#[tokio::test]
async fn the_owner_edits_the_organization_from_the_tree_and_the_hub_starts_again_with_it() {
    let (url, path, builds) = spin_up().await;
    let mut conn = ServerConnection::connect(&url, "web-1", "Browser", "test-key").await.unwrap();

    // A move and a role: the answer is the new settings, and the file has it.
    let reply = edit(&mut conn, "test-key", AgentOrgEdit::SetPosition { id: "dev".into(), role: Some("Backend".into()), reports_to: Some("chief".into()) }).await;
    match reply {
        ServerMessage::SettingsSaved { settings, .. } => {
            let dev = settings.agents.iter().find(|a| a.id == "dev").unwrap();
            assert_eq!((dev.role.as_deref(), dev.reports_to.as_deref()), (Some("Backend"), Some("chief")));
        }
        other => panic!("expected the new settings, got {other:?}"),
    }
    assert_eq!(builds.load(Ordering::SeqCst), 1, "the orchestrator was built again");
    let config = load_config_from_path(&path, false).unwrap();
    assert_eq!(config.agents.iter().find(|a| a.id == "dev").unwrap().reports_to.as_deref(), Some("chief"));

    // A new report, careful by default, then its removal.
    let added = edit(&mut conn, "test-key", AgentOrgEdit::AddReport { id: "reviewer".into(), persona: "Reviews.".into(), role: None, reports_to: Some("lead".into()) }).await;
    assert!(matches!(added, ServerMessage::SettingsSaved { .. }), "{added:?}");
    let config = load_config_from_path(&path, false).unwrap();
    let new = config.agents.iter().find(|a| a.id == "reviewer").unwrap();
    assert_eq!((new.reports_to.as_deref(), new.autonomy, new.can_delegate_to_agents), (Some("lead"), 3, false));

    let removed = edit(&mut conn, "test-key", AgentOrgEdit::Remove { id: "lead".into() }).await;
    assert!(matches!(removed, ServerMessage::SettingsSaved { .. }), "{removed:?}");
    let config = load_config_from_path(&path, false).unwrap();
    assert!(config.agents.iter().all(|a| a.id != "lead"));
    assert_eq!(config.agents.iter().find(|a| a.id == "reviewer").unwrap().reports_to.as_deref(), Some("chief"), "its reports went up to its superior");
    assert_eq!(builds.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn the_owner_sets_the_model_policies_and_an_agents_limit_from_a_light_client() {
    use warden_bootstrap::{Provider, ProviderConfig};
    use warden_server_protocol::protocol::ModelPolicyDto;

    let provider = |id: &str| ProviderConfig { id: id.into(), kind: Provider::Gemini, api_key: Some("k".into()), base_url: None, model: None, node: None };
    let (url, path, builds) = spin_up_with(FileConfig { agents: vec![agent("chief", None)], providers: vec![provider("main"), provider("spare")], ..FileConfig::default() }).await;
    let mut conn = ServerConnection::connect(&url, "phone-1", "Phone", "test-key").await.unwrap();

    let policies = AgentOrgEdit::SetModelPolicies { policies: vec![ModelPolicyDto { id: "fast".into(), model: "spare".into(), description: "simple work".into() }] };
    match edit(&mut conn, "test-key", policies).await {
        ServerMessage::SettingsSaved { settings, .. } => assert_eq!(settings.model_policies.iter().map(|p| (p.id.as_str(), p.model.as_str())).collect::<Vec<_>>(), [("fast", "spare")]),
        other => panic!("expected the new settings, got {other:?}"),
    }
    match edit(&mut conn, "test-key", AgentOrgEdit::SetDelegationModels { id: "chief".into(), models: vec!["fast".into(), "main".into()] }).await {
        ServerMessage::SettingsSaved { settings, .. } => assert_eq!(settings.agents[0].delegation_models, ["fast", "main"]),
        other => panic!("expected the new settings, got {other:?}"),
    }
    assert_eq!(builds.load(Ordering::SeqCst), 2);

    // A name that doesn't exist is refused with the reason, and nothing is written or restarted.
    let before = std::fs::read_to_string(&path).unwrap();
    match edit(&mut conn, "test-key", AgentOrgEdit::SetDelegationModels { id: "chief".into(), models: vec!["ghost".into()] }).await {
        ServerMessage::SettingsError { message, auth_rejected, .. } => assert!(!auth_rejected && message.contains("ghost"), "{message}"),
        other => panic!("expected a refusal, got {other:?}"),
    }
    match edit(&mut conn, "wrong", AgentOrgEdit::SetModelPolicies { policies: Vec::new() }).await {
        ServerMessage::SettingsError { auth_rejected, .. } => assert!(auth_rejected),
        other => panic!("expected a refusal, got {other:?}"),
    }
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    assert_eq!(builds.load(Ordering::SeqCst), 2);

    // Dropping the policy takes it out of the agent's limit as well.
    match edit(&mut conn, "test-key", AgentOrgEdit::SetModelPolicies { policies: Vec::new() }).await {
        ServerMessage::SettingsSaved { settings, .. } => assert_eq!(settings.agents[0].delegation_models, ["main"]),
        other => panic!("expected the new settings, got {other:?}"),
    }
    let config = load_config_from_path(&path, false).unwrap();
    assert!(config.model_policies.is_empty());
    assert_eq!(config.agents[0].delegation_models, ["main"]);
}

#[tokio::test]
async fn a_wrong_key_or_a_refused_edit_changes_nothing_and_does_not_restart_the_hub() {
    let (url, path, builds) = spin_up().await;
    let before = std::fs::read_to_string(&path).unwrap();
    let mut conn = ServerConnection::connect(&url, "web-1", "Browser", "test-key").await.unwrap();

    match edit(&mut conn, "wrong", AgentOrgEdit::Remove { id: "dev".into() }).await {
        ServerMessage::SettingsError { auth_rejected, message, .. } => assert!(auth_rejected && message.contains("wrong pairing key")),
        other => panic!("expected a refusal, got {other:?}"),
    }
    // A circle: chief can't report to its own report.
    match edit(&mut conn, "test-key", AgentOrgEdit::SetPosition { id: "chief".into(), role: None, reports_to: Some("dev".into()) }).await {
        ServerMessage::SettingsError { auth_rejected, message, .. } => assert!(!auth_rejected && !message.is_empty(), "{message}"),
        other => panic!("expected a refusal, got {other:?}"),
    }

    assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    assert_eq!(builds.load(Ordering::SeqCst), 0);
}
