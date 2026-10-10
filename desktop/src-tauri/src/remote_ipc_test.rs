//! P102 — the two halves together, without a window: the commands the native screens call (`remote_*`, `resolve_approval`,
//! `remove_hub`) driven through Tauri's own IPC (`tauri::test`, the JSON the page sends, with the page's camelCase argument
//! names) against a **real hub** in process. The screens' checks use a mock of the hub; the actor's tests use no Tauri; this
//! is the seam between them: argument names, the events the screens listen to, the token kept on disk, the approval ids
//! swapped, the session ended when its hub is removed. What it can't show is the webview itself.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::{json, Value};
use tauri::ipc::{CallbackFn, InvokeBody};
use tauri::test::{get_ipc_response, mock_builder, mock_context, noop_assets, MockRuntime, INVOKE_KEY};
use tauri::webview::InvokeRequest;
use tauri::{Listener, WebviewWindow, WebviewWindowBuilder};
use warden_bootstrap::{load_config_from_path, save_config, saved_hubs, AgentConfig, FileConfig};
use warden_core::memory::Vault;
use warden_core::model::{response_stream, ChatStream, Message, ModelProvider, Response, Role, ToolCall};
use warden_core::orchestrator::Orchestrator;
use warden_core::tool::ToolSpec;
use warden_server::{Server, SettingsHost};

use crate::{approval, hub_cmds, remote_cmds};

const KEY: &str = "ipc-test-key";

/// "CREATE" asks `manage_agents` for an agent (which asks the person); anything else is `echo:<message>`.
struct Scripted;

#[async_trait]
impl ModelProvider for Scripted {
    async fn chat_stream(&self, messages: Vec<Message>, _tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
        let last = messages.last().unwrap();
        let reply = |content: String| Ok(response_stream(Response { content, tool_calls: Vec::new(), usage: None }));
        if last.role == Role::Tool {
            return reply(format!("tool said: {}", last.content));
        }
        if last.content.contains("CREATE") {
            return Ok(response_stream(Response {
                content: String::new(),
                tool_calls: vec![ToolCall {
                    id: "call-1".into(),
                    name: "manage_agents".into(),
                    arguments: json!({ "action": "create", "id": "critic", "persona": "You critique." }),
                    thought_signature: None,
                }],
                usage: None,
            }));
        }
        reply(format!("echo:{}", last.content))
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

/// A real hub with one agent that may manage agents. Returns its address and its own `config.toml`.
async fn spin_up_hub(dir: &Path) -> (std::net::SocketAddr, PathBuf) {
    let config_path = dir.join("hub-config.toml");
    let chief = AgentConfig {
        id: "chief".into(),
        persona: "You are the chief.".into(),
        provider_id: None,
        can_delegate_to_agents: false,
        can_manage_agents: true,
        can_message_agents: false,
        can_manage_tasks: false,
        allowed_tools: None,
        autonomy: warden_bootstrap::default_autonomy(),
        approval_required: Vec::new(),
        role: None,
        reports_to: None,
        owner: None,
        shared_with: Vec::new(),
        delegation_models: Vec::new(),
        can_start_tasks: true,
        can_create_workers: true,
        can_message_user: true,
        can_choose_models: true,
    };
    save_config(&config_path, &FileConfig { agents: vec![chief], ..FileConfig::default() }).unwrap();
    let orchestrator = Orchestrator::new(Arc::new(Scripted), Arc::new(Vault::new(dir.join("hub-vault"))));
    let server = Server::bind("127.0.0.1:0".parse().unwrap(), KEY, "Test Hub", Arc::new(orchestrator), dir.join("hub-conversations"), dir.join("hub-devices.json"))
        .await
        .unwrap()
        .with_settings(Arc::new(TestHost { path: config_path.clone() }))
        .with_tasks(warden_bootstrap::tasks::TaskStore::new(dir.join("hub-tasks")), false)
        .with_webhooks(dir.join("hub-webhook-tokens.json"));
    let addr = server.local_addr().unwrap();
    tokio::spawn(server.serve());
    (addr, config_path)
}

/// The app's state with only what these commands touch: no engine, the rest a throwaway.
fn app_state(dir: &Path) -> crate::AppState {
    crate::AppState {
        orchestrator: Arc::new(Mutex::new(Err("not used by this test".to_string()))),
        recording: Mutex::new(None),
        sync: warden_sync::SyncEngine::new(dir.join("vault"), dir.join("config.toml"), dir.join("secrets.json"), dir.join("manifest.json")),
        pending_push: Mutex::new(None),
        generated_files_root: dir.join("generated"),
        embedded_server: Mutex::new(None),
        approvals: Arc::new(approval::ApprovalBroker::default()),
        code: crate::code_cmds::CodeState::default(),
        sync_runner: Arc::new(warden_bootstrap::auto_sync::SyncRunner::new(
            dir.join("vault"),
            dir.join("config.toml"),
            dir.join("secrets.json"),
            dir.join("manifest.json"),
            dir.join("git-sync-repo"),
        )),
        lending: Mutex::new(None),
        remote: Mutex::new(None),
    }
}

/// One IPC call, as the page makes it: a command name and a JSON body of camelCase arguments. The call blocks until the
/// command answers, so it runs off the async threads.
async fn ipc(webview: &WebviewWindow<MockRuntime>, cmd: &str, body: Value) -> Result<Value, Value> {
    let webview = webview.clone();
    let request = InvokeRequest {
        cmd: cmd.to_string(),
        callback: CallbackFn(0),
        error: CallbackFn(1),
        // The app's own page, not a remote one: that is the origin whose commands are allowed.
        url: "tauri://localhost".parse().unwrap(),
        body: InvokeBody::Json(body),
        headers: Default::default(),
        invoke_key: INVOKE_KEY.to_string(),
    };
    tokio::task::spawn_blocking(move || get_ipc_response(&webview, request).map(|answer| answer.deserialize::<Value>().expect("a JSON answer")))
        .await
        .unwrap()
}

type Seen = Arc<Mutex<Vec<(String, Value)>>>;

fn events(seen: &Seen, name: &str) -> Vec<Value> {
    seen.lock().unwrap().iter().filter(|(n, _)| n == name).map(|(_, v)| v.clone()).collect()
}

async fn until(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !check() {
        assert!(Instant::now() < deadline, "timed out waiting for: {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn states(seen: &Seen) -> Vec<String> {
    events(seen, "remote-hub-state").iter().map(|e| e["state"]["state"].as_str().unwrap_or("?").to_string()).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_commands_the_screens_call_drive_a_real_hub_through_the_ipc() {
    let dir = std::env::temp_dir().join(format!("warden-remote-ipc-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (addr, hub_config) = spin_up_hub(&dir).await;
    // This computer's own folder for the saved hubs and what it remembers of them: not the person's.
    *crate::config_paths::TEST_DIR.lock().unwrap() = Some(dir.clone());
    let hubs_file = dir.join("hubs.json");
    let identities_file = dir.join("remote_hub.json");
    let hub_id = saved_hubs::save(&hubs_file, None, "Test hub", &format!("http://{addr}")).unwrap().id;

    let app = mock_builder()
        .manage(app_state(&dir))
        .invoke_handler(tauri::generate_handler![
            remote_cmds::remote_connect,
            remote_cmds::remote_disconnect,
            remote_cmds::remote_status,
            remote_cmds::remote_request,
            remote_cmds::remote_chat,
            remote_cmds::remote_send,
            approval::resolve_approval,
            hub_cmds::list_hubs,
            hub_cmds::remove_hub,
        ])
        .build(mock_context(noop_assets()))
        .unwrap();
    let webview = WebviewWindowBuilder::new(&app, "main", Default::default()).build().unwrap();
    let seen: Seen = Arc::default();
    for name in ["remote-hub-state", "approval-request", "approval-cancelled", "conversations-changed", "chat-event"] {
        let seen = seen.clone();
        app.handle().listen(name, move |event| seen.lock().unwrap().push((name.to_string(), serde_json::from_str(event.payload()).unwrap())));
    }

    // Nothing in use yet; the saved hub is listed, not open.
    assert_eq!(ipc(&webview, "remote_status", json!({})).await.unwrap(), Value::Null);
    let listed = ipc(&webview, "list_hubs", json!({})).await.unwrap();
    assert_eq!((listed[0]["id"].as_str(), listed[0]["name"].as_str(), listed[0]["open"].as_bool()), (Some(hub_id.as_str()), Some("Test hub"), Some(false)));

    // A wrong key: the call returns at once and the state says why it ended.
    ipc(&webview, "remote_connect", json!({ "hubId": hub_id, "credential": { "kind": "key", "key": "not-the-key" } })).await.unwrap();
    until("the wrong key to end it", || events(&seen, "remote-hub-state").iter().any(|e| e["state"]["state"] == "stopped" && e["state"]["error"].is_string())).await;
    let stopped = events(&seen, "remote-hub-state").into_iter().find(|e| e["state"]["state"] == "stopped").unwrap();
    assert_eq!(stopped["hubId"], json!(hub_id));
    assert!(stopped["state"]["error"].as_str().unwrap().contains("authentication rejected"), "{stopped}");
    assert!(!identities_file.exists() || !std::fs::read_to_string(&identities_file).unwrap().contains("device_token\": \""), "no token from a refused key");

    // The right key: connected as the owner, and only a token is kept.
    let before = events(&seen, "remote-hub-state").len();
    ipc(&webview, "remote_connect", json!({ "hubId": hub_id, "credential": { "kind": "key", "key": KEY } })).await.unwrap();
    until("the owner to be connected", || events(&seen, "remote-hub-state")[before..].iter().any(|e| e["state"]["state"] == "connected")).await;
    let connected = events(&seen, "remote-hub-state").into_iter().rev().find(|e| e["state"]["state"] == "connected").unwrap();
    assert_eq!(connected, json!({ "hubId": hub_id, "state": { "state": "connected", "user": null } }), "the screens read exactly this");
    let status = ipc(&webview, "remote_status", json!({})).await.unwrap();
    assert_eq!((status["hubId"].as_str(), status["state"]["state"].as_str()), (Some(hub_id.as_str()), Some("connected")));
    until("the token to be kept", || identities_file.exists() && std::fs::read_to_string(&identities_file).unwrap().contains("device_token\": \"")).await;
    let kept = std::fs::read_to_string(&identities_file).unwrap();
    assert!(!kept.contains(KEY), "the key is never written: {kept}");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&identities_file).unwrap().permissions().mode() & 0o777, 0o600);
    }

    // What the screens ask and send, as the page writes it.
    let list = ipc(&webview, "remote_request", json!({ "message": { "type": "listConversations" } })).await.unwrap();
    assert_eq!(list["type"], "conversationList");
    let refused = ipc(&webview, "remote_request", json!({ "message": { "type": "noSuchThing" } })).await.unwrap_err();
    assert!(refused.as_str().unwrap().contains("not a message the hub knows"), "{refused}");
    let answer = ipc(&webview, "remote_chat", json!({ "message": { "type": "chat", "message": "hello", "conversationId": "c1", "attachments": [] } })).await.unwrap();
    assert_eq!((answer["type"].as_str(), answer["content"].as_str(), answer["conversationId"].as_str()), (Some("chatResponse"), Some("echo:hello"), Some("c1")));
    let history = ipc(&webview, "remote_request", json!({ "message": { "type": "requestHistory", "conversationId": "c1" } })).await.unwrap();
    assert_eq!(history["type"], "history");
    let turns: Vec<_> = history["messages"].as_array().unwrap().iter().map(|m| (m["role"].as_str().unwrap().to_string(), m["content"].as_str().unwrap().to_string())).collect();
    assert_eq!(turns, vec![("user".to_string(), "hello".to_string()), ("assistant".to_string(), "echo:hello".to_string())]);
    let unnamed = ipc(&webview, "remote_chat", json!({ "message": { "type": "chat", "message": "x", "attachments": [] } })).await.unwrap_err();
    assert!(unnamed.as_str().unwrap().contains("has to name its conversation"), "{unnamed}");
    ipc(&webview, "remote_send", json!({ "message": { "type": "setCodeMode", "conversationId": "c1", "mode": "plan" } })).await.unwrap();

    // An approval the hub asks for reaches the modal under this app's id, and the answer goes back to the hub.
    let turn = {
        let webview = webview.clone();
        tokio::spawn(async move { ipc(&webview, "remote_chat", json!({ "message": { "type": "chat", "message": "CREATE a critic", "conversationId": "c2", "attachments": [], "agentId": "chief" } })).await })
    };
    until("the approval to reach the modal", || !events(&seen, "approval-request").is_empty()).await;
    let asked = events(&seen, "approval-request").remove(0);
    assert_eq!((asked["action"].as_str(), asked["target"].as_str()), (Some("create_agent"), Some("critic")));
    let local_id = asked["id"].as_u64().unwrap();
    assert_eq!(ipc(&webview, "resolve_approval", json!({ "id": 999_999, "approved": true })).await.unwrap(), json!(false), "an id nobody waits on finds nothing");
    assert_eq!(ipc(&webview, "resolve_approval", json!({ "id": local_id, "approved": true, "always": false })).await.unwrap(), json!(true), "answered to the hub");
    let created = tokio::time::timeout(Duration::from_secs(15), turn).await.expect("the turn did not finish after the approval was answered").unwrap().unwrap();
    assert_eq!(created["type"], "chatResponse", "{created}");
    assert!(load_config_from_path(&hub_config, false).unwrap().agents.iter().any(|a| a.id == "critic"), "a yes on this app made the hub save the agent");

    // Coming back with only the token.
    ipc(&webview, "remote_disconnect", json!({})).await.unwrap();
    assert_eq!(ipc(&webview, "remote_status", json!({})).await.unwrap(), Value::Null);
    until("the stop to be told", || states(&seen).last().map(String::as_str) == Some("stopped")).await;
    let before = events(&seen, "remote-hub-state").len();
    ipc(&webview, "remote_connect", json!({ "hubId": hub_id })).await.unwrap();
    until("the token to sign in", || events(&seen, "remote-hub-state")[before..].iter().any(|e| e["state"]["state"] == "connected")).await;
    let again = ipc(&webview, "remote_request", json!({ "message": { "type": "listConversations" } })).await.unwrap();
    assert_eq!(again["type"], "conversationList");

    // The other screens, asked of the hub: the names and replies `lib/hub.ts` and `hubMap.ts` read.
    let ask = |message: Value| ipc(&webview, "remote_request", json!({ "message": message }));
    let saved = ask(json!({ "type": "saveVaultNote", "path": "notes/a.md", "content": "one" })).await.unwrap();
    assert_eq!(saved["type"], "vaultSaved", "{saved}");
    let version = saved["version"].as_str().unwrap().to_string();
    assert_eq!(ask(json!({ "type": "listVaultFiles" })).await.unwrap()["files"], json!(["notes/a.md"]));
    let note = ask(json!({ "type": "readVaultNote", "path": "notes/a.md" })).await.unwrap();
    assert_eq!((note["type"].as_str(), note["content"].as_str(), note["version"].as_str()), (Some("vaultNote"), Some("one"), Some(version.as_str())));
    let hits = ask(json!({ "type": "searchVault", "query": "one" })).await.unwrap();
    assert_eq!((hits["type"].as_str(), hits["hits"][0]["path"].as_str(), hits["hits"][0]["lineNumber"].as_u64()), (Some("vaultSearchResults"), Some("notes/a.md"), Some(1)));
    let stale = ask(json!({ "type": "saveVaultNote", "path": "notes/a.md", "content": "two", "expectedVersion": "not-the-version" })).await.unwrap();
    assert_eq!((stale["type"].as_str(), stale["conflict"].as_bool()), (Some("vaultError"), Some(true)), "a stale save says conflict, which the screen turns into reload or overwrite: {stale}");
    let deleted = ask(json!({ "type": "deleteVaultNote", "path": "notes/a.md", "expectedVersion": version })).await.unwrap();
    assert_eq!(deleted["type"], "vaultOk");

    let skill = json!({ "name": "greet", "description": "Say hello", "body": "Say hello.", "agents": [] });
    assert_eq!(ask(json!({ "type": "saveSkill", "skill": skill, "overwrite": false })).await.unwrap()["type"], "skillOk");
    let skills = ask(json!({ "type": "listSkills" })).await.unwrap();
    assert_eq!((skills["type"].as_str(), skills["skills"][0]["name"].as_str()), (Some("skillList"), Some("greet")));
    assert_eq!(ask(json!({ "type": "deleteSkill", "name": "greet" })).await.unwrap()["type"], "skillOk");

    let usage = ask(json!({ "type": "requestUsage", "tzOffsetMinutes": -180 })).await.unwrap();
    assert_eq!(usage["type"], "usageReport", "{usage}");
    for field in ["total", "conversationCount", "messageCount", "limitsEnabled", "limits"] {
        assert!(usage["report"].get(field).is_some(), "the Usage screen reads `{field}`: {usage}");
    }
    // The scripted model reports no tokens, so there are no model calls to count: the shape is what matters here.
    assert_eq!(usage["report"]["messageCount"], json!(0), "{usage}");
    assert_eq!(usage["report"]["total"]["totalTokens"], json!(0), "{usage}");

    // Tasks and webhooks: the list is open, every change carries the pairing key the person typed for it.
    let tasks = ask(json!({ "type": "listTasks" })).await.unwrap();
    assert_eq!((tasks["type"].as_str(), tasks["tasks"].as_array().map(Vec::len), tasks["runsHere"].is_boolean()), (Some("taskList"), Some(0), true), "{tasks}");
    let task = json!({ "id": "daily", "prompt": "Say hi", "every": "1d", "enabled": true });
    // A refusal is an ordinary reply ending in `Error`, whose `message` is what `expectReply` throws.
    let wrong = ask(json!({ "type": "saveTask", "pairingKey": "not-the-key", "task": task })).await.unwrap();
    assert_eq!((wrong["type"].as_str(), wrong["message"].as_str()), (Some("taskError"), Some("wrong pairing key")), "{wrong}");
    assert_eq!(ask(json!({ "type": "listTasks" })).await.unwrap()["tasks"].as_array().map(Vec::len), Some(0), "nothing was saved");
    let made = ask(json!({ "type": "saveTask", "pairingKey": KEY, "task": task })).await.unwrap();
    assert_eq!((made["type"].as_str(), made["tasks"][0]["id"].as_str()), (Some("taskList"), Some("daily")), "{made}");
    let paused = ask(json!({ "type": "setTaskEnabled", "pairingKey": KEY, "id": "daily", "enabled": false })).await.unwrap();
    assert_eq!(paused["tasks"][0]["enabled"], json!(false));
    let gone = ask(json!({ "type": "deleteTask", "pairingKey": KEY, "id": "daily" })).await.unwrap();
    assert_eq!(gone["tasks"].as_array().map(Vec::len), Some(0));
    let hooks = ask(json!({ "type": "listWebhooks" })).await.unwrap();
    assert_eq!((hooks["type"].as_str(), hooks["servesHere"].is_boolean()), (Some("webhookList"), true), "{hooks}");
    let hook = json!({ "id": "build", "prompt": "Why did it fail?", "enabled": true, "auth": "token" });
    let saved = ask(json!({ "type": "saveWebhook", "pairingKey": KEY, "webhook": hook })).await.unwrap();
    assert_eq!((saved["type"].as_str(), saved["webhooks"][0]["conversation"].as_str()), (Some("webhookList"), Some("task-hook-build")), "{saved}");
    let credential = ask(json!({ "type": "createWebhookCredential", "pairingKey": KEY, "id": "build" })).await.unwrap();
    assert_eq!((credential["type"].as_str(), credential["id"].as_str(), credential["kind"].as_str()), (Some("webhookCreated"), Some("build"), Some("token")), "{credential}");
    assert!(credential["credential"].as_str().is_some_and(|c| !c.is_empty()) && credential["webhooks"][0]["credential"] == "token", "{credential}");
    let revoked = ask(json!({ "type": "revokeWebhookCredential", "pairingKey": KEY, "id": "build" })).await.unwrap();
    assert!(revoked["webhooks"][0].get("credential").is_none(), "{revoked}");
    let removed = ask(json!({ "type": "deleteWebhook", "pairingKey": KEY, "id": "build" })).await.unwrap();
    assert_eq!(removed["webhooks"].as_array().map(Vec::len), Some(0));

    // Signing out forgets the token: the next connection has to ask.
    ipc(&webview, "remote_disconnect", json!({ "forget": true })).await.unwrap();
    let asked_for_sign_in = ipc(&webview, "remote_connect", json!({ "hubId": hub_id })).await.unwrap_err();
    assert!(asked_for_sign_in.as_str().unwrap().contains("not signed in"), "{asked_for_sign_in}");
    assert_eq!(ipc(&webview, "remote_status", json!({})).await.unwrap(), Value::Null);

    // Taking a hub off the list ends its session and forgets what is kept of it.
    ipc(&webview, "remote_connect", json!({ "hubId": hub_id, "credential": { "kind": "key", "key": KEY } })).await.unwrap();
    until("connected again", || ipc_state_is_connected(&seen)).await;
    assert_eq!(ipc(&webview, "remote_status", json!({})).await.unwrap()["hubId"], json!(hub_id));
    ipc(&webview, "remove_hub", json!({ "id": hub_id })).await.unwrap();
    assert_eq!(ipc(&webview, "remote_status", json!({})).await.unwrap(), Value::Null, "the session of a removed hub is over");
    assert!(saved_hubs::list(&hubs_file).unwrap().is_empty());
    assert!(!std::fs::read_to_string(&identities_file).unwrap().contains(&hub_id), "nothing of it is remembered");

    *crate::config_paths::TEST_DIR.lock().unwrap() = None;
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Whether the last state told is "connected".
fn ipc_state_is_connected(seen: &Seen) -> bool {
    states(seen).last().map(String::as_str) == Some("connected")
}
