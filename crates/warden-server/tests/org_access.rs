//! P120 on the hub: the workspace has one organization of agents, the owner's, and the owner says per member what they do with it —
//! `none`, `view` or `edit`. A member never sends a pairing key: their session plus the access in the config is the authorization, read
//! from the file at each request.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use warden_bootstrap::{load_config_from_path, save_config, AgentConfig, FileConfig};
use warden_core::memory::Vault;
use warden_core::model::{ChatStream, Message, ModelProvider};
use warden_core::orchestrator::Orchestrator;
use warden_core::tool::ToolSpec;
use warden_server::{ClientMessage, Server, ServerConnection, ServerMessage, SettingsHost};
use warden_server_protocol::protocol::{AgentOrgEdit, OrgAgentDto};

const KEY: &str = "test-key";
const TEMP: &str = "provisional-1";

struct NoModel;

#[async_trait]
impl ModelProvider for NoModel {
    async fn chat_stream(&self, _messages: Vec<Message>, _tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
        anyhow::bail!("these tests never run a turn")
    }
}

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

fn agent(id: &str, boss: Option<&str>, owner: Option<&str>) -> AgentConfig {
    AgentConfig {
        id: id.into(),
        persona: format!("{id} SECRET persona"),
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
        owner: owner.map(str::to_string),
        shared_with: Vec::new(),
        delegation_models: Vec::new(),
        can_start_tasks: true,
        can_create_workers: true,
    }
}

struct Hub {
    url: String,
    path: PathBuf,
    builds: Arc<AtomicUsize>,
}

async fn spin_up() -> Hub {
    let dir = std::env::temp_dir().join(format!("warden-server-org-access-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    let mut config = FileConfig { agents: vec![agent("chief", None, None), agent("lead", Some("chief"), None), agent("dev", Some("lead"), None), agent("anas-own", None, Some("ana"))], ..FileConfig::default() };
    warden_bootstrap::users::add_user(&mut config, "ana", "Ana", TEMP).unwrap();
    warden_bootstrap::users::add_user(&mut config, "bo", "Bo", TEMP).unwrap();
    save_config(&path, &config).unwrap();
    let builds = Arc::new(AtomicUsize::new(0));
    let orchestrator = Orchestrator::new(Arc::new(NoModel), Arc::new(Vault::new(dir.join("vault"))));
    let server = Server::bind("127.0.0.1:0".parse().unwrap(), KEY, "Test Hub", Arc::new(orchestrator), dir.join("conversations"), dir.join("devices.json"))
        .await
        .unwrap()
        .with_settings(Arc::new(Host { path: path.clone(), dir: dir.clone(), builds: builds.clone() }))
        .with_users_dir(dir.join("users"));
    let addr = server.local_addr().unwrap();
    tokio::spawn(server.serve());
    Hub { url: format!("ws://{addr}"), path, builds }
}

async fn next(conn: &mut ServerConnection) -> ServerMessage {
    loop {
        match tokio::time::timeout(Duration::from_secs(10), conn.recv()).await.expect("no reply").unwrap().expect("connection closed") {
            // The push of a new level is not an answer to anything these helpers asked (`pushed_access` reads it).
            ServerMessage::ConversationsChanged { .. } | ServerMessage::Pong { .. } | ServerMessage::OrgAccessChanged { .. } => continue,
            message => return message,
        }
    }
}

/// Ana, past her provisional password.
async fn ana(hub: &Hub) -> ServerConnection {
    let (mut ana, _, _) =
        ServerConnection::handshake_as_member(&hub.url, "ana-phone", "Ana's phone", "ana", TEMP, None, warden_server_protocol::tls::default_client_config()).await.unwrap();
    ana.send(&ClientMessage::ChangePassword { request_id: 1, old_password: TEMP.into(), new_password: "anas-own-pass".into(), recovery_code: None }).await.unwrap();
    assert!(matches!(next(&mut ana).await, ServerMessage::PasswordChanged { .. }));
    ana
}

async fn owner_sets(hub: &Hub, access: &str) -> String {
    let mut owner = ServerConnection::connect(&hub.url, "laptop", "Laptop", KEY).await.unwrap();
    owner.send(&ClientMessage::SetUserOrgAccess { request_id: 1, pairing_key: KEY.into(), id: "ana".into(), access: access.into() }).await.unwrap();
    match next(&mut owner).await {
        ServerMessage::UserList { users, .. } => users.into_iter().find(|u| u.id == "ana").unwrap().org_access,
        other => panic!("expected the member list, got {other:?}"),
    }
}

async fn list(conn: &mut ServerConnection) -> ServerMessage {
    conn.send(&ClientMessage::ListAgentOrg { request_id: 2 }).await.unwrap();
    next(conn).await
}

async fn edit(conn: &mut ServerConnection, key: &str, edit: AgentOrgEdit) -> ServerMessage {
    conn.send(&ClientMessage::EditAgentOrg { request_id: 3, pairing_key: key.into(), edit }).await.unwrap();
    next(conn).await
}

fn refused(message: &ServerMessage) -> bool {
    matches!(message, ServerMessage::SettingsError { auth_rejected: true, .. })
}

fn tree(message: &ServerMessage) -> (Vec<(String, Option<String>)>, String) {
    match message {
        ServerMessage::AgentOrgList { agents, access, .. } => (agents.iter().map(|a: &OrgAgentDto| (a.id.clone(), a.reports_to.clone())).collect(), access.clone()),
        other => panic!("expected the tree, got {other:?}"),
    }
}

fn position(id: &str, boss: &str) -> AgentOrgEdit {
    AgentOrgEdit::SetPosition { id: id.into(), role: None, reports_to: Some(boss.into()) }
}

fn boss_of(hub: &Hub, id: &str) -> Option<String> {
    load_config_from_path(&hub.path, false).unwrap().agents.iter().find(|a| a.id == id).and_then(|a| a.reports_to.clone())
}

#[tokio::test]
async fn a_member_sees_and_changes_the_one_organization_only_as_far_as_the_owner_allows() {
    let hub = spin_up().await;

    // On the provisional password, not even a look.
    let (mut first, _, _) =
        ServerConnection::handshake_as_member(&hub.url, "ana-phone", "Ana's phone", "ana", TEMP, None, warden_server_protocol::tls::default_client_config()).await.unwrap();
    assert!(refused(&list(&mut first).await));
    drop(first);

    let mut ana = ana(&hub).await;

    // `none` is what every member had before: no tree, and a pairing key (which she should not have) buys nothing.
    assert!(refused(&list(&mut ana).await));
    assert!(refused(&edit(&mut ana, KEY, position("dev", "chief")).await));
    assert_eq!(boss_of(&hub, "dev").as_deref(), Some("lead"), "nothing moved");

    // `view`: the owner's agents with their positions, not her own, and nothing she may not read.
    assert_eq!(owner_sets(&hub, "view").await, "view");
    let seen = list(&mut ana).await;
    let (agents, access) = tree(&seen);
    assert_eq!(access, "view");
    assert_eq!(agents, [("chief".to_string(), None), ("lead".to_string(), Some("chief".to_string())), ("dev".to_string(), Some("lead".to_string()))]);
    assert!(!serde_json::to_string(&seen).unwrap().contains("SECRET"), "an agent's persona is not part of the tree");
    assert!(refused(&edit(&mut ana, "", position("dev", "chief")).await), "seeing is not changing");
    assert_eq!(hub.builds.load(Ordering::SeqCst), 0);

    // `edit`: the three edits of the tree, with the owner's rules, and no key.
    assert_eq!(owner_sets(&hub, "edit").await, "edit");
    let moved = edit(&mut ana, "", position("dev", "chief")).await;
    let (agents, access) = tree(&moved);
    assert_eq!(access, "edit");
    assert!(agents.contains(&("dev".to_string(), Some("chief".to_string()))));
    assert_eq!(boss_of(&hub, "dev").as_deref(), Some("chief"), "the file has it");
    assert_eq!(hub.builds.load(Ordering::SeqCst), 1, "the hub started again with the change, as it does for the owner");

    let added = edit(&mut ana, "", AgentOrgEdit::AddReport { id: "reviewer".into(), persona: "Reviews.".into(), role: Some("QA".into()), reports_to: Some("lead".into()) }).await;
    assert!(tree(&added).0.contains(&("reviewer".to_string(), Some("lead".to_string()))));
    let new = load_config_from_path(&hub.path, false).unwrap().agents.into_iter().find(|a| a.id == "reviewer").unwrap();
    assert_eq!((new.owner, new.autonomy, new.can_delegate_to_agents, new.can_manage_agents), (None, 3, false, false), "careful by default, and the owner's, not hers");

    let removed = edit(&mut ana, "", AgentOrgEdit::Remove { id: "lead".into() }).await;
    assert!(!tree(&removed).0.iter().any(|(id, _)| id == "lead"));
    assert_eq!(boss_of(&hub, "dev").as_deref(), Some("chief"));

    // The owner's rules still hold for her: a move that would close a circle is an error, not a refusal of her access.
    let circle = edit(&mut ana, "", position("chief", "dev")).await;
    assert!(matches!(circle, ServerMessage::SettingsError { auth_rejected: false, .. }), "{circle:?}");

    // What is not the tree stays the owner's.
    for owners_only in [AgentOrgEdit::SetDelegationModels { id: "chief".into(), models: vec!["x".into()] }, AgentOrgEdit::SetModelPolicies { policies: Vec::new() }] {
        assert!(refused(&edit(&mut ana, "", owners_only).await));
    }

    // She cannot give herself more, and the access is read again at each request, so the owner can take it back.
    ana.send(&ClientMessage::SetUserOrgAccess { request_id: 4, pairing_key: KEY.into(), id: "ana".into(), access: "edit".into() }).await.unwrap();
    assert!(matches!(next(&mut ana).await, ServerMessage::UserError { auth_rejected: true, .. }));
    assert_eq!(owner_sets(&hub, "none").await, "", "none is no access at all");
    assert!(refused(&edit(&mut ana, "", position("dev", "reviewer")).await));
    assert!(refused(&list(&mut ana).await));
    assert_eq!(boss_of(&hub, "dev").as_deref(), Some("chief"), "the edit she tried after losing the access changed nothing");

    // A value that is not an access is an error and changes nothing.
    let mut owner = ServerConnection::connect(&hub.url, "laptop", "Laptop", KEY).await.unwrap();
    owner.send(&ClientMessage::SetUserOrgAccess { request_id: 5, pairing_key: KEY.into(), id: "ana".into(), access: "admin".into() }).await.unwrap();
    assert!(matches!(next(&mut owner).await, ServerMessage::UserError { auth_rejected: false, .. }));
    assert!(refused(&list(&mut ana).await));
}

#[tokio::test]
async fn the_owner_always_sees_the_tree_and_a_wrong_pairing_key_still_changes_nothing() {
    let hub = spin_up().await;
    let mut owner = ServerConnection::connect(&hub.url, "laptop", "Laptop", KEY).await.unwrap();
    let (agents, access) = tree(&list(&mut owner).await);
    assert_eq!(access, "edit");
    assert_eq!(agents.len(), 3, "her own agent is not in the tree");
    assert!(refused(&edit(&mut owner, "wrong", position("dev", "chief")).await), "the owner still needs the key");
    assert_eq!(boss_of(&hub, "dev").as_deref(), Some("lead"));
}

/// What the connection is told next, apart from the chatter every connection gets; `None` when nothing comes.
async fn pushed(conn: &mut ServerConnection, wait: Duration) -> Option<ServerMessage> {
    loop {
        match tokio::time::timeout(wait, conn.recv()).await {
            Err(_) => return None,
            Ok(message) => match message.unwrap().expect("connection closed") {
                ServerMessage::ConversationsChanged { .. } | ServerMessage::Pong { .. } => continue,
                other => return Some(other),
            },
        }
    }
}

const HEARD: Duration = Duration::from_secs(10);
const QUIET: Duration = Duration::from_millis(600);

#[tokio::test]
async fn a_member_hears_the_new_access_on_every_device_at_once_and_nobody_else_does() {
    let hub = spin_up().await;
    let mut owner = ServerConnection::connect(&hub.url, "laptop", "Laptop", KEY).await.unwrap();
    let mut phone = ana(&hub).await;
    let (mut tablet, _, _) =
        ServerConnection::handshake_as_member(&hub.url, "ana-tablet", "Ana's tablet", "ana", "anas-own-pass", None, warden_server_protocol::tls::default_client_config()).await.unwrap();
    let (mut bo, _, _) = ServerConnection::handshake_as_member(&hub.url, "bo-phone", "Bo's phone", "bo", TEMP, None, warden_server_protocol::tls::default_client_config()).await.unwrap();

    // The owner gives `view`: both of Ana's devices are told, with the level the hub saved; the owner's own screen and Bo's are not.
    owner.send(&ClientMessage::SetUserOrgAccess { request_id: 1, pairing_key: KEY.into(), id: "ana".into(), access: "view".into() }).await.unwrap();
    assert!(matches!(next(&mut owner).await, ServerMessage::UserList { .. }));
    for device in [&mut phone, &mut tablet] {
        assert_eq!(pushed(device, HEARD).await, Some(ServerMessage::OrgAccessChanged { access: "view".into() }));
    }
    assert_eq!(pushed(&mut bo, QUIET).await, None, "another member hears nothing about Ana");
    assert_eq!(pushed(&mut owner, QUIET).await, None);

    // Taking it back is told too, and the level is the one saved, not the one typed.
    owner.send(&ClientMessage::SetUserOrgAccess { request_id: 2, pairing_key: KEY.into(), id: "ana".into(), access: "none".into() }).await.unwrap();
    assert!(matches!(next(&mut owner).await, ServerMessage::UserList { .. }));
    assert_eq!(pushed(&mut phone, HEARD).await, Some(ServerMessage::OrgAccessChanged { access: "none".into() }));

    // A change that did not happen is not announced: a wrong key, and a value that is not an access.
    owner.send(&ClientMessage::SetUserOrgAccess { request_id: 3, pairing_key: "wrong".into(), id: "ana".into(), access: "edit".into() }).await.unwrap();
    assert!(matches!(next(&mut owner).await, ServerMessage::UserError { auth_rejected: true, .. }));
    owner.send(&ClientMessage::SetUserOrgAccess { request_id: 4, pairing_key: KEY.into(), id: "ana".into(), access: "admin".into() }).await.unwrap();
    assert!(matches!(next(&mut owner).await, ServerMessage::UserError { auth_rejected: false, .. }));
    assert_eq!(pushed(&mut phone, QUIET).await, None);
    assert_eq!(pushed(&mut tablet, QUIET).await, Some(ServerMessage::OrgAccessChanged { access: "none".into() }), "the tablet had not read the second push yet; nothing after it");
    assert_eq!(pushed(&mut tablet, QUIET).await, None);
}
