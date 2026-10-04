//! Tauri commands for P102 phase 2: this app's native interface using a hub on another machine, through the
//! `RemoteHandle` of `warden_server::remote_client` (which owns the WebSocket, the reconnecting and the matching of
//! replies). What lives here is the glue to the window:
//!
//! - what the hub pushes becomes the events the local engine already emits (`chat-event`, `approval-request`,
//!   `approval-cancelled`, `conversations-changed`), so the screens that listen don't change;
//! - an approval the hub asks for gets an id of this app's `ApprovalBroker` (the hub's ids are per connection and would
//!   collide with local ones), and `resolve_approval` forwards the answer to the hub when the id is one of those;
//! - the connection's state goes out as `remote-hub-state`, and the token the hub issues is kept in `remote_hub.json`,
//!   so the key or password typed once is never stored.
//!
//! One hub is the active one at a time; "this computer" is the absence of a session.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, State};
use warden_bootstrap::saved_hubs;
use warden_server::remote_client::{
    RemoteConfig, RemoteCredential, RemoteEvent, RemoteHandle, RemoteIdentities, RemoteSink, RemoteState, REQUEST_TIMEOUT, TURN_TIMEOUT,
};
use warden_server::ClientMessage;

use crate::approval::{ApprovalBroker, ApprovalPayload};
use crate::AppState;

/// What this app calls itself on the hubs it signs in to.
const DEVICE_NAME: &str = "Warden desktop";

/// The approvals a hub is waiting on: this app's id → the hub's id.
#[derive(Default)]
pub struct ApprovalMap {
    to_hub: HashMap<u64, u64>,
}

impl ApprovalMap {
    fn insert(&mut self, local: u64, hub: u64) {
        self.to_hub.insert(local, hub);
    }

    /// The hub's id for `local`, forgetting the pair: an approval is answered once.
    pub fn take(&mut self, local: u64) -> Option<u64> {
        self.to_hub.remove(&local)
    }

    /// This app's id for the hub's `hub`, forgetting the pair.
    fn take_hub(&mut self, hub: u64) -> Option<u64> {
        let local = self.to_hub.iter().find(|(_, h)| **h == hub).map(|(l, _)| *l)?;
        self.to_hub.remove(&local);
        Some(local)
    }
}

/// The hub in use: which one, how to talk to it, and its open approvals.
pub struct RemoteSession {
    pub hub_id: String,
    pub handle: RemoteHandle,
    pub approvals: Arc<Mutex<ApprovalMap>>,
}

/// Generic over the runtime so a test can emit through Tauri's mock one, with no window.
struct TauriSink<R: tauri::Runtime> {
    app: AppHandle<R>,
    hub_id: String,
    identities: RemoteIdentities,
    broker: Arc<ApprovalBroker>,
    approvals: Arc<Mutex<ApprovalMap>>,
}

impl<R: tauri::Runtime> RemoteSink for TauriSink<R> {
    fn emit(&self, event: RemoteEvent) {
        match event {
            RemoteEvent::State(state) => {
                // A hub that turned this device away: the token is dead, so the next connection has to sign in again.
                if let RemoteState::Stopped { error: Some(error) } = &state {
                    if error.contains("authentication rejected") {
                        if let Err(err) = self.identities.clear_token(&self.hub_id) {
                            eprintln!("remote hub: could not forget the token: {err:#}");
                        }
                    }
                }
                let _ = self.app.emit("remote-hub-state", json!({ "hubId": self.hub_id, "state": state }));
            }
            RemoteEvent::NewToken(token) => {
                if let Err(err) = self.identities.set_token(&self.hub_id, &token) {
                    eprintln!("remote hub: could not keep the token: {err:#}");
                }
            }
            RemoteEvent::ChatEvent { conversation_id, event } => {
                let _ = self.app.emit("chat-event", json!({ "conversationId": conversation_id, "event": event }));
            }
            RemoteEvent::Approval { approval_id, target, action, detail, always } => {
                let local = self.broker.allocate();
                self.approvals.lock().unwrap_or_else(|e| e.into_inner()).insert(local, approval_id);
                let _ = self.app.emit("approval-request", ApprovalPayload { id: local, target, action, detail, always });
            }
            RemoteEvent::ApprovalCancelled { approval_id } => {
                let local = self.approvals.lock().unwrap_or_else(|e| e.into_inner()).take_hub(approval_id);
                if let Some(local) = local {
                    let _ = self.app.emit("approval-cancelled", local);
                }
            }
            RemoteEvent::ConversationsChanged { conversation_id } => {
                let _ = self.app.emit("conversations-changed", conversation_id);
            }
        }
    }
}

/// `ws://`/`wss://` for the saved hub's `http(s)://` address, which is where its web interface is.
pub(crate) fn ws_url_for(hub_url: &str) -> Result<String, String> {
    let rest = hub_url.strip_prefix("https://").map(|r| ("wss", r)).or_else(|| hub_url.strip_prefix("http://").map(|r| ("ws", r)));
    match rest {
        Some((scheme, rest)) => Ok(format!("{scheme}://{}", rest.trim_end_matches('/'))),
        None => Err(format!("'{hub_url}' is not an http or https address")),
    }
}

/// The client message in `value` (the shape the web page sends), with `requestId` set to `id`.
pub(crate) fn message_with_request_id(mut value: Value, id: u64) -> Result<ClientMessage, String> {
    let Some(object) = value.as_object_mut() else {
        return Err("a message is a JSON object".to_string());
    };
    object.insert("requestId".to_string(), json!(id));
    serde_json::from_value(value).map_err(|e| format!("not a message the hub knows: {e}"))
}

fn identities() -> Result<RemoteIdentities, String> {
    Ok(RemoteIdentities::beside(&crate::config_paths::config_file()?))
}

/// How the first sign-in to a hub is made. Not kept: only the token the hub issues is.
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum CredentialPayload {
    Key { key: String },
    Member { username: String, password: String },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteStatusPayload {
    hub_id: String,
    state: RemoteState,
}

/// Starts using the saved hub `hub_id`, ending the one in use. With a `credential` it signs in as that (the key makes the
/// owner, a username and password a member); without one it uses the token from the last sign-in, and says so when there
/// is none. Returns at once: the state follows as `remote-hub-state`.
#[tauri::command]
pub async fn remote_connect<R: tauri::Runtime>(app: AppHandle<R>, state: State<'_, AppState>, hub_id: String, credential: Option<CredentialPayload>) -> Result<(), String> {
    let hubs_path = crate::config_paths::saved_hubs_file()?;
    let hub = saved_hubs::find(&hubs_path, &hub_id).map_err(|e| format!("{e:#}"))?.ok_or_else(|| format!("there is no saved hub '{hub_id}' (it may have been removed)"))?;
    let url = ws_url_for(&hub.url)?;
    let identities = identities()?;
    let identity = identities.load_or_create(&hub_id, DEVICE_NAME).map_err(|e| format!("{e:#}"))?;
    let credential = credential.map(|c| match c {
        CredentialPayload::Key { key } => RemoteCredential::PairingKey(key.trim().to_string()),
        CredentialPayload::Member { username, password } => RemoteCredential::Member { username: username.trim().to_string(), password },
    });
    if credential.is_none() && identity.device_token.is_none() {
        return Err(format!("not signed in to '{}' yet: give its pairing key or a username and password", hub.name));
    }
    let config = RemoteConfig { url, device_id: identity.device_id, device_name: DEVICE_NAME.to_string(), credential, token: identity.device_token };

    let approvals: Arc<Mutex<ApprovalMap>> = Arc::default();
    let sink = Arc::new(TauriSink { app, hub_id: hub_id.clone(), identities, broker: state.approvals.clone(), approvals: approvals.clone() });
    let handle = RemoteHandle::start(config, sink);
    let previous = state.remote.lock().unwrap_or_else(|e| e.into_inner()).replace(RemoteSession { hub_id, handle, approvals });
    if let Some(previous) = previous {
        previous.handle.stop();
    }
    Ok(())
}

/// Goes back to this computer. With `forget` the token goes too, so the next connection asks for the key or password.
#[tauri::command]
pub fn remote_disconnect(state: State<'_, AppState>, forget: Option<bool>) -> Result<(), String> {
    let session = state.remote.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(session) = session {
        session.handle.stop();
        if forget.unwrap_or(false) {
            identities()?.clear_token(&session.hub_id).map_err(|e| format!("{e:#}"))?;
        }
    }
    Ok(())
}

#[tauri::command]
pub fn remote_status(state: State<'_, AppState>) -> Option<RemoteStatusPayload> {
    let guard = state.remote.lock().unwrap_or_else(|e| e.into_inner());
    guard.as_ref().map(|s| RemoteStatusPayload { hub_id: s.hub_id.clone(), state: s.handle.state() })
}

fn active_handle(state: &State<'_, AppState>) -> Result<RemoteHandle, String> {
    state.remote.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|s| s.handle.clone()).ok_or_else(|| "no hub is in use".to_string())
}

/// Sends one of the web page's messages (`{"type": "listConversations"}`...) and returns the reply that carries its
/// request id, as JSON. Hub-side refusals (`conversationError`, `projectError`...) are replies, not errors here.
#[tauri::command]
pub async fn remote_request(state: State<'_, AppState>, message: Value) -> Result<Value, String> {
    let handle = active_handle(&state)?;
    let reply = handle.request(|id| message_with_request_id(message, id), REQUEST_TIMEOUT).await?;
    serde_json::to_value(reply).map_err(|e| format!("{e}"))
}

/// Runs a `chat` turn on the hub and returns its `chatResponse` or `chatError`. The message must name its conversation.
#[tauri::command]
pub async fn remote_chat(state: State<'_, AppState>, message: Value) -> Result<Value, String> {
    let handle = active_handle(&state)?;
    let message: ClientMessage = serde_json::from_value(message).map_err(|e| format!("not a message the hub knows: {e}"))?;
    let ClientMessage::Chat { conversation_id: Some(conversation_id), .. } = &message else {
        return Err("a turn has to name its conversation".to_string());
    };
    let conversation_id = conversation_id.clone();
    let reply = handle.turn(&conversation_id, message, TURN_TIMEOUT).await?;
    serde_json::to_value(reply).map_err(|e| format!("{e}"))
}

/// Sends a message that has no reply (`cancelTurn`, `setCodeMode`).
#[tauri::command]
pub fn remote_send(state: State<'_, AppState>, message: Value) -> Result<(), String> {
    let handle = active_handle(&state)?;
    let message: ClientMessage = serde_json::from_value(message).map_err(|e| format!("not a message the hub knows: {e}"))?;
    handle.send(message);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_saved_address_becomes_the_websocket_one() {
        assert_eq!(ws_url_for("http://192.168.1.5:7420/").unwrap(), "ws://192.168.1.5:7420");
        assert_eq!(ws_url_for("https://hub.example.ts.net/").unwrap(), "wss://hub.example.ts.net");
        assert_eq!(ws_url_for("https://hub.example.ts.net:8443/").unwrap(), "wss://hub.example.ts.net:8443");
        for bad in ["ws://x/", "file:///etc/passwd", "hub.example", ""] {
            assert!(ws_url_for(bad).is_err(), "'{bad}'");
        }
    }

    #[test]
    fn a_page_message_gets_its_request_id_and_must_be_one_the_hub_knows() {
        let message = message_with_request_id(json!({ "type": "listConversations" }), 7).unwrap();
        assert_eq!(message, ClientMessage::ListConversations { request_id: 7 });
        let dirs = message_with_request_id(json!({ "type": "listDirs", "path": "/srv" }), 8).unwrap();
        assert_eq!(dirs, ClientMessage::ListDirs { request_id: 8, path: Some("/srv".into()) });
        // The id is the connection's to give: one the page sent is replaced.
        let replaced = message_with_request_id(json!({ "type": "listConversations", "requestId": 1 }), 9).unwrap();
        assert_eq!(replaced, ClientMessage::ListConversations { request_id: 9 });
        for bad in [json!({ "type": "noSuchThing" }), json!("listConversations"), json!([1]), json!({})] {
            assert!(message_with_request_id(bad.clone(), 1).is_err(), "{bad}");
        }
    }

    /// The contract with `desktop/src/lib/hubMap.ts`: the real messages of the hub, serialized as `remote_request` and
    /// `remote_chat` hand them to the screens, carry the type names and fields the mappers read — and the messages the
    /// screens send parse as the hub's own. (The screens' checks use a mock hub written by hand; this ties it to the types.)
    #[test]
    fn the_hub_messages_have_the_names_the_screens_read_and_send() {
        use warden_server::ServerMessage;
        use warden_server_protocol::protocol::{AgentSettingsDto, ConversationSummary, HistoryMessage, HistoryRole, ProjectDto, ProviderFallbackDto};

        let value = |m: ServerMessage| serde_json::to_value(m).unwrap();
        let keys = |v: &Value| -> Vec<String> {
            let mut k: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
            k.sort();
            k
        };

        let list = value(ServerMessage::ConversationList {
            request_id: 1,
            conversations: vec![ConversationSummary { id: "c1".into(), title: "T".into(), created_at: 1, updated_at: 2, agent_id: Some("poet".into()), project_id: None, workdir: None }],
        });
        assert_eq!(list["type"], "conversationList");
        assert_eq!(keys(&list["conversations"][0]), ["agentId", "createdAt", "id", "title", "updatedAt"], "empty optionals are left out: the mapper copes with their absence");

        let history = value(ServerMessage::History { request_id: 1, messages: vec![HistoryMessage { role: HistoryRole::Assistant, content: "hi".into(), created_at: 3, attachments: Vec::new() }] });
        assert_eq!(history["type"], "history");
        assert_eq!((history["messages"][0]["role"].as_str(), history["messages"][0]["content"].as_str(), history["messages"][0]["createdAt"].as_i64()), (Some("assistant"), Some("hi"), Some(3)));

        let projects = value(ServerMessage::ProjectList { request_id: 1, projects: vec![ProjectDto { id: "tax".into(), name: "Tax".into(), description: String::new(), instructions: String::new(), workdir: None, code: false }] });
        assert_eq!(projects["type"], "projectList");
        assert_eq!(keys(&projects["projects"][0]), ["description", "id", "instructions", "name"], "workdir and code come only when set");

        let answer = value(ServerMessage::ChatResponse {
            content: "ahoy".into(),
            usage: None,
            attachments: Vec::new(),
            conversation_id: Some("c1".into()),
            fallbacks: vec![ProviderFallbackDto { from: "a".into(), to: "b".into(), model: "m".into(), reason: "down".into() }],
        });
        assert_eq!(answer["type"], "chatResponse");
        assert_eq!((answer["content"].as_str(), answer["conversationId"].as_str(), answer["usage"].is_null()), (Some("ahoy"), Some("c1"), true), "usage is null, not absent");
        assert_eq!(keys(&answer["fallbacks"][0]), ["from", "model", "reason", "to"]);
        let failed = value(ServerMessage::ChatError { message: "boom".into(), conversation_id: Some("c1".into()), spend_limit_id: Some("day".into()) });
        assert_eq!((failed["type"].as_str(), failed["message"].as_str(), failed["spendLimitId"].as_str()), (Some("chatError"), Some("boom"), Some("day")));

        let agent = serde_json::to_value(AgentSettingsDto {
            original_id: None,
            id: "poet".into(),
            persona: "You write.".into(),
            provider_id: String::new(),
            can_delegate_to_agents: false,
            can_manage_agents: false,
            can_message_agents: false,
            can_manage_tasks: false,
            allowed_tools: None,
            shared_with: Vec::new(),
            owner: None,
        })
        .unwrap();
        for field in ["id", "persona", "providerId", "canDelegateToAgents", "canManageAgents", "canMessageAgents", "canManageTasks", "allowedTools", "sharedWith"] {
            assert!(agent.get(field).is_some(), "the agent has no '{field}': {agent}");
        }
        assert!(agent["allowedTools"].is_null(), "null, as the mapper's `string[] | null` says");

        // What the screens send, parsed as the hub's own messages.
        let sends = [
            (json!({ "type": "listProjects" }), ClientMessage::ListProjects { request_id: 5 }),
            (json!({ "type": "requestSettings" }), ClientMessage::RequestSettings { request_id: 5 }),
            (json!({ "type": "requestHistory", "conversationId": "c1" }), ClientMessage::RequestHistory { request_id: 5, limit: None, conversation_id: Some("c1".into()) }),
            (json!({ "type": "moveConversation", "conversationId": "c1", "projectId": "tax" }), ClientMessage::MoveConversation { request_id: 5, conversation_id: "c1".into(), project_id: Some("tax".into()) }),
            (json!({ "type": "moveConversation", "conversationId": "c1" }), ClientMessage::MoveConversation { request_id: 5, conversation_id: "c1".into(), project_id: None }),
        ];
        for (sent, wanted) in sends {
            assert_eq!(message_with_request_id(sent.clone(), 5).unwrap(), wanted, "{sent}");
        }
        let no_reply = [
            (json!({ "type": "setCodeMode", "conversationId": "c1", "mode": "plan" }), ClientMessage::SetCodeMode { conversation_id: "c1".into(), mode: "plan".into() }),
            (json!({ "type": "cancelTurn", "conversationId": "c1" }), ClientMessage::CancelTurn { conversation_id: "c1".into() }),
            (
                json!({ "type": "chat", "message": "hello", "conversationId": "c1", "attachments": [], "agentId": "poet", "projectId": "tax" }),
                ClientMessage::Chat { message: "hello".into(), conversation_id: Some("c1".into()), attachments: Vec::new(), agent_id: Some("poet".into()), project_id: Some("tax".into()), workdir: None },
            ),
            (
                json!({ "type": "chat", "message": "hi", "conversationId": "c2", "attachments": [], "workdir": "/srv/work" }),
                ClientMessage::Chat { message: "hi".into(), conversation_id: Some("c2".into()), attachments: Vec::new(), agent_id: None, project_id: None, workdir: Some("/srv/work".into()) },
            ),
        ];
        for (sent, wanted) in no_reply {
            assert_eq!(serde_json::from_value::<ClientMessage>(sent.clone()).unwrap(), wanted, "{sent}");
        }
    }

    #[test]
    fn credentials_are_read_the_way_the_screen_sends_them() {
        let key: CredentialPayload = serde_json::from_value(json!({ "kind": "key", "key": "k" })).unwrap();
        assert!(matches!(key, CredentialPayload::Key { key } if key == "k"));
        let member: CredentialPayload = serde_json::from_value(json!({ "kind": "member", "username": "ana", "password": "p" })).unwrap();
        assert!(matches!(member, CredentialPayload::Member { username, password } if (username.as_str(), password.as_str()) == ("ana", "p")));
        assert!(serde_json::from_value::<CredentialPayload>(json!({ "kind": "token", "token": "t" })).is_err(), "a token is never typed in");
    }

    /// Not mocked: a real `AppHandle` (Tauri's own test runtime, no window) that the sink emits through, and listeners
    /// on the very event names the screens listen to.
    #[test]
    fn what_the_hub_pushes_reaches_the_screens_as_the_events_they_already_listen_to() {
        use tauri::Listener;
        use warden_server_protocol::protocol::ChatEventDto;

        let app = tauri::test::mock_app();
        let handle = app.handle().clone();
        let seen: Arc<Mutex<Vec<(String, Value)>>> = Arc::default();
        for name in ["remote-hub-state", "chat-event", "approval-request", "approval-cancelled", "conversations-changed"] {
            let seen = seen.clone();
            handle.listen(name, move |event| seen.lock().unwrap().push((name.to_string(), serde_json::from_str(event.payload()).unwrap())));
        }
        let events = |name: &str| -> Vec<Value> { seen.lock().unwrap().iter().filter(|(n, _)| n == name).map(|(_, v)| v.clone()).collect() };

        let dir = std::env::temp_dir().join(format!("warden-remote-cmds-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("remote_hub.json");
        let identities = RemoteIdentities::new(file.clone());
        identities.load_or_create("hub-1", "Desktop").unwrap();
        let broker = Arc::new(ApprovalBroker::default());
        // Local approvals already used some ids: the hub's must not be mistaken for one of them.
        for _ in 0..3 {
            broker.allocate();
        }
        let approvals: Arc<Mutex<ApprovalMap>> = Arc::default();
        let sink = TauriSink { app: handle, hub_id: "hub-1".into(), identities, broker, approvals: approvals.clone() };

        sink.emit(RemoteEvent::State(RemoteState::Connected { user: None }));
        assert_eq!(events("remote-hub-state"), vec![json!({ "hubId": "hub-1", "state": { "state": "connected", "user": null } })]);

        sink.emit(RemoteEvent::NewToken("tok-1".into()));
        assert_eq!(RemoteIdentities::new(file.clone()).load_or_create("hub-1", "x").unwrap().device_token.as_deref(), Some("tok-1"), "the token is kept");

        sink.emit(RemoteEvent::ChatEvent { conversation_id: "c1".into(), event: ChatEventDto::Text { text: "hi".into() } });
        assert_eq!(events("chat-event"), vec![json!({ "conversationId": "c1", "event": { "type": "text", "text": "hi" } })], "the shape liveTurn.applyEvent reads");

        sink.emit(RemoteEvent::ConversationsChanged { conversation_id: "c1".into() });
        assert_eq!(events("conversations-changed"), vec![json!("c1")], "a bare id, as the local engine sends it");

        sink.emit(RemoteEvent::Approval { approval_id: 5, target: "critic".into(), action: "create_agent".into(), detail: "d".into(), always: Some("git *".into()) });
        let asked = events("approval-request");
        assert_eq!(asked.len(), 1);
        let local = asked[0]["id"].as_u64().unwrap();
        assert_eq!(local, 3, "an id of this app's own space, not the hub's 5");
        assert_eq!((asked[0]["target"].as_str(), asked[0]["action"].as_str(), asked[0]["always"].as_str()), (Some("critic"), Some("create_agent"), Some("git *")));
        assert_eq!(approvals.lock().unwrap().take(local), Some(5), "and it is mapped back to the hub's");
        approvals.lock().unwrap().insert(local, 5);

        sink.emit(RemoteEvent::ApprovalCancelled { approval_id: 5 });
        assert_eq!(events("approval-cancelled"), vec![json!(local)], "the modal is closed under the id it was opened with");
        assert_eq!(approvals.lock().unwrap().take(local), None, "and forgotten");
        // One the hub cancels that this app never saw closes nothing.
        sink.emit(RemoteEvent::ApprovalCancelled { approval_id: 77 });
        assert_eq!(events("approval-cancelled").len(), 1);

        // A hub that turned this device away: the token is dead, and the next connection signs in again.
        sink.emit(RemoteEvent::State(RemoteState::Stopped { error: Some("authentication rejected: device revoked".into()) }));
        assert_eq!(RemoteIdentities::new(file.clone()).load_or_create("hub-1", "x").unwrap().device_token, None);
        // Any other reason to stop keeps it.
        sink.emit(RemoteEvent::NewToken("tok-2".into()));
        sink.emit(RemoteEvent::State(RemoteState::Stopped { error: None }));
        assert_eq!(RemoteIdentities::new(file).load_or_create("hub-1", "x").unwrap().device_token.as_deref(), Some("tok-2"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_approval_pair_is_answered_once_and_found_from_either_side() {
        let mut map = ApprovalMap::default();
        map.insert(10, 1);
        map.insert(11, 2);
        assert_eq!(map.take(10), Some(1));
        assert_eq!(map.take(10), None, "answered once");
        assert_eq!(map.take_hub(2), Some(11));
        assert_eq!(map.take_hub(2), None);
        assert_eq!(map.take(99), None, "a local approval is not in it");
    }
}
