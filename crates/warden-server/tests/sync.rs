//! P61 on the hub: the web's Sync screen over a real socket (status, init, sync now), against a
//! real local bare git remote, and the standalone hub's own loop reloading the orchestrator when a
//! round brings another device's `config.toml`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use warden_bootstrap::auto_sync::SyncRunner;
use warden_core::memory::Vault;
use warden_core::model::{ChatStream, Message, ModelProvider};
use warden_core::orchestrator::Orchestrator;
use warden_core::tool::ToolSpec;
use warden_server::{ClientMessage, Server, ServerConnection, ServerMessage, SettingsHost};
use warden_server_protocol::protocol::{SyncActionDto, SyncBackendDto, SyncStatusDto};

/// These tests never run a chat turn.
struct NoModel;

#[async_trait]
impl ModelProvider for NoModel {
    async fn chat_stream(&self, _messages: Vec<Message>, _tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
        anyhow::bail!("no model in these tests")
    }
}

fn orchestrator(dir: &Path) -> Orchestrator {
    Orchestrator::new(Arc::new(NoModel), Arc::new(Vault::new(dir.join("vault"))))
}

/// Counts how often the hub rebuilt its orchestrator from the file.
struct CountingHost {
    dir: PathBuf,
    builds: Arc<AtomicUsize>,
}

#[async_trait]
impl SettingsHost for CountingHost {
    fn config_path(&self) -> PathBuf {
        self.dir.join("hub").join("config.toml")
    }

    async fn build(&self) -> anyhow::Result<Orchestrator> {
        self.builds.fetch_add(1, Ordering::SeqCst);
        Ok(orchestrator(&self.dir))
    }
}

fn temp_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "warden-server-sync-{}",
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn bare_remote(dir: &Path) -> PathBuf {
    let path = dir.join("remote.git");
    assert!(std::process::Command::new("git").args(["init", "--bare", "-q", &path.to_string_lossy()]).status().unwrap().success());
    path
}

/// A device's own vault/config/secrets under `dir/name`, with `[git_sync]` pointing at `remote`
/// (written straight to the file: the web only accepts https:// remotes).
fn device(dir: &Path, name: &str, remote: &Path, extra_config: &str) -> SyncRunner {
    let root = dir.join(name);
    std::fs::create_dir_all(root.join("vault")).unwrap();
    std::fs::write(root.join("config.toml"), format!("{extra_config}[git_sync]\nremote_url = {:?}\ntoken = \"t\"\n", remote.to_string_lossy())).unwrap();
    SyncRunner::new(root.join("vault"), root.join("config.toml"), root.join("secrets.json"), root.join("manifest.json"), root.join("git-repo"))
}

fn secrets(dir: &Path, name: &str) -> PathBuf {
    dir.join(name).join("secrets.json")
}

async fn status(conn: &mut ServerConnection, message: ClientMessage) -> Result<SyncStatusDto, (String, bool)> {
    conn.send(&message).await.unwrap();
    match conn.recv().await.unwrap() {
        Some(ServerMessage::SyncStatus { status, .. }) => Ok(status),
        Some(ServerMessage::SyncError { message, auth_rejected, .. }) => Err((message, auth_rejected)),
        other => panic!("expected a sync reply, got {other:?}"),
    }
}

fn action(pairing_key: &str, action: SyncActionDto) -> ClientMessage {
    ClientMessage::SyncAction { request_id: 2, pairing_key: pairing_key.into(), action }
}

#[tokio::test]
async fn the_web_sets_up_the_hub_and_its_notes_reach_another_device() {
    let dir = temp_dir();
    let remote = bare_remote(&dir);
    let hub_runner = Arc::new(device(&dir, "hub", &remote, ""));
    let server = Server::bind("127.0.0.1:0".parse().unwrap(), "test-key", "Test Hub", Arc::new(orchestrator(&dir)), dir.join("conversations"), dir.join("devices.json"))
        .await
        .unwrap()
        .with_sync(hub_runner, None);
    let addr = server.local_addr().unwrap();
    tokio::spawn(server.serve());
    let mut conn = ServerConnection::connect(&format!("ws://{addr}"), "web-1", "Browser", "test-key").await.unwrap();

    let first = status(&mut conn, ClientMessage::RequestSyncStatus { request_id: 1 }).await.unwrap();
    assert_eq!((first.backend, first.git_remote.is_some()), (SyncBackendDto::NotSetUp, true));

    assert!(status(&mut conn, action("wrong", SyncActionDto::Init)).await.unwrap_err().1, "auth rejected");
    assert!(!secrets(&dir, "hub").exists(), "a wrong key ran nothing");

    let ready = status(&mut conn, action("test-key", SyncActionDto::Init)).await.unwrap();
    assert_eq!(ready.backend, SyncBackendDto::Git);

    std::fs::write(dir.join("hub").join("vault").join("nota.md"), "do hub").unwrap();
    let after = status(&mut conn, action("test-key", SyncActionDto::SyncNow)).await.unwrap();
    let round = after.last_round.expect("the round is reported");
    assert_eq!(round.error, None);
    assert_eq!(round.pushed.map(|p| p.files_changed), Some(1));
    assert_eq!(after.pending_vault_changes, 0);

    // Another device holding the same key (as pairing would leave it) gets the note.
    let laptop = device(&dir, "laptop", &remote, "");
    std::fs::copy(secrets(&dir, "hub"), secrets(&dir, "laptop")).unwrap();
    let report = laptop.run_once().await;
    assert_eq!(report.error, None, "{report:?}");
    assert_eq!(std::fs::read_to_string(dir.join("laptop").join("vault").join("nota.md")).unwrap(), "do hub");
}

#[tokio::test]
async fn the_hub_loop_pulls_a_new_config_and_reloads() {
    let dir = temp_dir();
    let remote = bare_remote(&dir);
    let hub_runner = Arc::new(device(&dir, "hub", &remote, ""));
    hub_runner.init_fresh().await.unwrap();

    // The laptop publishes a config.toml different from the hub's.
    let laptop = device(&dir, "laptop", &remote, "enable_shell = false\n");
    std::fs::copy(secrets(&dir, "hub"), secrets(&dir, "laptop")).unwrap();
    let pushed = laptop.run_once().await;
    assert!(pushed.pushed.is_some(), "{pushed:?}");

    let builds = Arc::new(AtomicUsize::new(0));
    let server = Server::bind("127.0.0.1:0".parse().unwrap(), "test-key", "Test Hub", Arc::new(orchestrator(&dir)), dir.join("conversations"), dir.join("devices.json"))
        .await
        .unwrap()
        .with_settings(Arc::new(CountingHost { dir: dir.clone(), builds: builds.clone() }))
        .with_sync(hub_runner, Some(Duration::from_millis(200)));
    tokio::spawn(server.serve());

    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while builds.load(Ordering::SeqCst) == 0 {
        assert!(tokio::time::Instant::now() < deadline, "the hub never reloaded");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let hub_config = std::fs::read_to_string(dir.join("hub").join("config.toml")).unwrap();
    assert!(hub_config.contains("enable_shell = false"), "{hub_config}");
    // Only rounds that bring a new config reload.
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert_eq!(builds.load(Ordering::SeqCst), 1);
}
