//! P102 on a real hub: a conversation in no project picks a folder of the hub's machine before its first message. The
//! owner may pick any folder; a member only inside the folders the owner named for them, checked on every turn and
//! not just when the folder is picked; the folder browser shows folders and nothing else.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use warden_bootstrap::{load_config_from_path, save_config, FileConfig};
use warden_core::memory::Vault;
use warden_core::model::{response_stream, ChatStream, Message, ModelProvider, Response};
use warden_core::orchestrator::Orchestrator;
use warden_core::tool::ToolSpec;
use warden_server::{ClientMessage, Server, ServerConnection, ServerMessage, SettingsHost};
use warden_server_protocol::protocol::DirEntryDto;

const KEY: &str = "test-key";
const TEMP: &str = "provisional-1";

/// Answers with every message it was sent ("|"-separated), so a test sees what a turn told the model.
struct Echo;

#[async_trait]
impl ModelProvider for Echo {
    async fn chat_stream(&self, messages: Vec<Message>, _: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
        let told = messages.iter().map(|m| m.content.clone()).collect::<Vec<_>>().join("|");
        Ok(response_stream(Response { content: told, tool_calls: Vec::new(), usage: None }))
    }
}

struct Host {
    path: PathBuf,
}

#[async_trait]
impl SettingsHost for Host {
    fn config_path(&self) -> PathBuf {
        self.path.clone()
    }

    async fn build(&self) -> anyhow::Result<Orchestrator> {
        anyhow::bail!("not used by these tests")
    }
}

struct Hub {
    url: String,
    config_path: PathBuf,
    /// The machine's folders: `allowed/` (with `inner/`), `other/`.
    tree: PathBuf,
}

/// A hub whose member Ana may work in `allowed/` only.
async fn spin_up() -> Hub {
    std::env::set_var(warden_core::memory::embed::OFF_SWITCH, "1");
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("warden-server-workdir-{}-{n}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    let tree = dir.join("machine");
    for folder in ["allowed/inner", "other"] {
        std::fs::create_dir_all(tree.join(folder)).unwrap();
    }
    std::fs::write(tree.join("allowed/notes.txt"), "x").unwrap();
    let tree = tree.canonicalize().unwrap();

    let config_path = dir.join("config.toml");
    let mut config = FileConfig::default();
    warden_bootstrap::users::add_user(&mut config, "ana", "Ana", TEMP).unwrap();
    config.users[0].workdirs = vec![tree.join("allowed").to_string_lossy().into_owned()];
    save_config(&config_path, &config).unwrap();

    let orchestrator = Orchestrator::new(Arc::new(Echo), Arc::new(Vault::new(dir.join("vault"))));
    let server = Server::bind("127.0.0.1:0".parse().unwrap(), KEY, "Test Hub", Arc::new(orchestrator), dir.join("conversations"), dir.join("devices.json"))
        .await
        .unwrap()
        .with_users_dir(dir.join("users"))
        .with_settings(Arc::new(Host { path: config_path.clone() }));
    let addr = server.local_addr().unwrap();
    tokio::spawn(server.serve());
    Hub { url: format!("ws://{addr}"), config_path, tree }
}

async fn owner(hub: &Hub) -> ServerConnection {
    ServerConnection::connect(&hub.url, "laptop", "Laptop", KEY).await.unwrap()
}

async fn ana(hub: &Hub) -> ServerConnection {
    let (mut conn, _, _) = ServerConnection::handshake_as_member(&hub.url, "ana-phone", "Ana's phone", "ana", TEMP, None, warden_server_protocol::tls::default_client_config()).await.unwrap();
    conn.send(&ClientMessage::ChangePassword { request_id: 1, old_password: TEMP.into(), new_password: "anas-own-pass".into(), recovery_code: None }).await.unwrap();
    assert!(matches!(next(&mut conn).await, ServerMessage::PasswordChanged { .. }));
    conn
}

async fn next(conn: &mut ServerConnection) -> ServerMessage {
    loop {
        match tokio::time::timeout(Duration::from_secs(10), conn.recv()).await.expect("no message").unwrap().expect("connection closed") {
            ServerMessage::ConversationsChanged { .. } | ServerMessage::Pong { .. } => continue,
            msg => return msg,
        }
    }
}

async fn chat(conn: &mut ServerConnection, id: &str, text: &str, workdir: Option<&Path>) -> ServerMessage {
    conn.send(&ClientMessage::Chat { message: text.into(), conversation_id: Some(id.into()), attachments: Vec::new(), agent_id: None, project_id: None, workdir: workdir.map(|p| p.to_string_lossy().into_owned()), thread_of: None }).await.unwrap();
    next(conn).await
}

async fn dirs(conn: &mut ServerConnection, path: Option<&Path>) -> Result<(String, Option<String>, Vec<DirEntryDto>), String> {
    conn.send(&ClientMessage::ListDirs { request_id: 7, path: path.map(|p| p.to_string_lossy().into_owned()) }).await.unwrap();
    match next(conn).await {
        ServerMessage::DirList { request_id: 7, path, parent, dirs } => Ok((path, parent, dirs)),
        ServerMessage::DirError { request_id: 7, message } => Err(message),
        other => panic!("{other:?}"),
    }
}

async fn folder_of(conn: &mut ServerConnection, id: &str) -> Option<String> {
    conn.send(&ClientMessage::ListConversations { request_id: 9 }).await.unwrap();
    match next(conn).await {
        ServerMessage::ConversationList { conversations, .. } => conversations.into_iter().find(|c| c.id == id).unwrap().workdir,
        other => panic!("{other:?}"),
    }
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[tokio::test]
async fn the_owner_picks_any_folder_before_the_first_message_and_the_conversation_keeps_it() {
    let hub = spin_up().await;
    let mut me = owner(&hub).await;

    // The browser: folders only, with the way up.
    let (path, parent, listed) = dirs(&mut me, Some(&hub.tree)).await.unwrap();
    assert_eq!(path, text(&hub.tree));
    assert!(parent.is_some());
    assert_eq!(listed.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), ["allowed", "other"]);
    assert!(dirs(&mut me, None).await.is_ok(), "no path starts at home");
    assert!(dirs(&mut me, Some(&hub.tree.join("allowed/notes.txt"))).await.is_err(), "a file is not a folder");

    // The first message names it: the model is told, and the list says where the conversation works.
    let other = hub.tree.join("other");
    match chat(&mut me, "c1", "hello", Some(&other)).await {
        ServerMessage::ChatResponse { content, .. } => assert!(content.contains(&format!("works in the folder '{}'", other.display())), "{content}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(folder_of(&mut me, "c1").await, Some(text(&other)));

    // Naming another folder later, or none, doesn't move it.
    let allowed = hub.tree.join("allowed");
    match chat(&mut me, "c1", "again", Some(&allowed)).await {
        ServerMessage::ChatResponse { content, .. } => assert!(content.contains(&format!("works in the folder '{}'", other.display())), "{content}"),
        other => panic!("{other:?}"),
    }
    assert!(matches!(chat(&mut me, "c1", "once more", None).await, ServerMessage::ChatResponse { .. }));
    assert_eq!(folder_of(&mut me, "c1").await, Some(text(&other)));

    // A folder that isn't there is refused, and nothing is saved for it.
    match chat(&mut me, "c2", "hello", Some(&hub.tree.join("gone"))).await {
        ServerMessage::ChatError { message, .. } => assert!(message.contains("not a folder"), "{message}"),
        other => panic!("{other:?}"),
    }
    me.send(&ClientMessage::ListConversations { request_id: 10 }).await.unwrap();
    match next(&mut me).await {
        ServerMessage::ConversationList { conversations, .. } => assert!(conversations.iter().all(|c| c.id != "c2")),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn a_member_only_browses_and_works_in_the_folders_the_owner_named_for_them() {
    let hub = spin_up().await;
    let mut ana = ana(&hub).await;
    let allowed = hub.tree.join("allowed");

    // The browser starts at their folders, goes down and back up to the list, and never leaves.
    let (path, parent, listed) = dirs(&mut ana, None).await.unwrap();
    assert_eq!((path.as_str(), parent), ("", None));
    assert_eq!(listed, vec![DirEntryDto { name: "allowed".into(), path: text(&allowed) }]);
    let (_, parent, inner) = dirs(&mut ana, Some(&allowed)).await.unwrap();
    assert_eq!((parent.as_deref(), inner.iter().map(|d| d.name.as_str()).collect::<Vec<_>>()), (Some(""), vec!["inner"]));
    assert!(dirs(&mut ana, Some(&hub.tree.join("other"))).await.is_err());
    assert!(dirs(&mut ana, Some(&hub.tree)).await.is_err(), "the folder above theirs is not theirs");
    assert!(dirs(&mut ana, Some(&PathBuf::from(format!("{}/../other", allowed.display())))).await.is_err());

    // Working: inside their folder yes (a deeper one too), outside no — nothing saved for the refused one.
    assert!(matches!(chat(&mut ana, "a1", "hello", Some(&allowed.join("inner"))).await, ServerMessage::ChatResponse { .. }));
    assert_eq!(folder_of(&mut ana, "a1").await, Some(text(&allowed.join("inner"))));
    match chat(&mut ana, "a2", "hello", Some(&hub.tree.join("other"))).await {
        ServerMessage::ChatError { message, .. } => assert!(message.contains("not a folder you can work in"), "{message}"),
        other => panic!("{other:?}"),
    }
    ana.send(&ClientMessage::ListConversations { request_id: 11 }).await.unwrap();
    match next(&mut ana).await {
        ServerMessage::ConversationList { conversations, .. } => assert!(conversations.iter().all(|c| c.id != "a2")),
        other => panic!("{other:?}"),
    }

    // The owner takes the folder away: her old conversation in it stops too, on its next turn.
    let mut config = load_config_from_path(&hub.config_path, false).unwrap();
    config.users[0].workdirs.clear();
    save_config(&hub.config_path, &config).unwrap();
    match chat(&mut ana, "a1", "still there?", None).await {
        ServerMessage::ChatError { message, .. } => assert!(message.contains("not a folder you can work in"), "{message}"),
        other => panic!("{other:?}"),
    }
    assert!(dirs(&mut ana, None).await.unwrap().2.is_empty(), "and she has none to pick");
}
