//! The Warden API (P12): the hub's agent behind the OpenAI chat-completions wire format, on the same
//! port as the WebSocket protocol and the web UI. Anything that speaks to OpenAI (a script, n8n, a
//! chat client) can point its base URL at `http(s)://<hub>/v1` and a key made in the app
//! (`api_keys.rs`), and talk to the Warden: its vault, skills and tools, and a configured agent's
//! persona when `model` names one (`warden/<agent>`).
//!
//! Deliberately small, decisions of the user's:
//! - the client's `tools` (P91) are offered next to the agent's own, and the client's wins when a
//!   name repeats. A call to one ends the request with `finish_reason: "tool_calls"`; the client runs
//!   it and sends the results back as `tool` messages, and the turn carries on from there. A key
//!   bound to an agent with `allowed_tools` is how the agent's own tools are narrowed.
//!   `tool_choice: "none"` leaves the client's tools out; any other value is `auto`, since the
//!   providers aren't told a choice. `parallel_tool_calls` is ignored;
//! - the ids of the calls handed out are the hub's own (`call_<hex>`), unique per call. A Gemini
//!   `thought_signature`, which Gemini 3 requires back on the call, travels inside the id
//!   (`call_<hex>__ts_<base64url>`), so the hub keeps no state between requests;
//! - nothing is saved as a conversation; the spend goes to the `api` channel, per key, so spending
//!   limits (P4) apply;
//! - a key of a workspace member (P84 fatia 2) speaks as them: their vault, the tools the owner
//!   allows them, only the agents they see, and their spending as a person;
//! - plain HTTP/1.1 written by hand like `web_ui.rs`: one request per connection, `Content-Length`
//!   bodies only, `Connection: close`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use warden_bootstrap::users::agent_visible_to;
use warden_bootstrap::{build_model_for, load_config_from_path, scope_to_agent, AgentExtras, FileConfig};
use warden_core::model::{Message, StreamEvent, ToolCall, Usage};
use warden_core::orchestrator::{MessageOutcome, Orchestrator};
use warden_core::spend::SpendContext;
use warden_core::tool::ToolSpec;

use crate::api_keys::{ApiKey, ApiKeyStore};
use crate::people::{member_orchestrator, mount_member_spaces, tools_for, MemberSpace, SpaceVaults};
use crate::settings::{SettingsHost, SharedOrchestrator, WRONG_KEY_DELAY};
use crate::usage::spend_limit_id;
use crate::web_ui::{write_response, RequestHead};

/// Every route lives under this.
pub const API_PREFIX: &str = "/v1/";

/// The model name for the hub's own model and persona; `warden/<agent>` speaks as that agent.
pub const DEFAULT_MODEL: &str = "warden";

/// Largest request body accepted — a long history, in text.
const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;

/// How long a client gets to send the whole body.
const BODY_TIMEOUT: Duration = Duration::from_secs(30);

/// What the API needs from the hub.
#[derive(Clone)]
pub(crate) struct ApiContext {
    pub orchestrator: SharedOrchestrator,
    pub settings: Option<Arc<dyn SettingsHost>>,
    /// `None`: this hub has no API (the routes answer 404).
    pub keys_path: Option<Arc<PathBuf>>,
    /// Where members' vaults live (P84); `None`: a member's key is refused.
    pub users_dir: Option<Arc<PathBuf>>,
    /// The hub's conversations directory — only to describe a member's space; the API saves none.
    pub conversations_root: Arc<PathBuf>,
    /// The owner's shared folders' vaults (P84 fatia 3), for a member's key.
    pub space_vaults: SpaceVaults,
}

/// A failure, answered in OpenAI's error format.
struct ApiError {
    status: &'static str,
    kind: &'static str,
    code: Option<String>,
    message: String,
}

impl ApiError {
    fn new(status: &'static str, kind: &'static str, message: impl Into<String>) -> Self {
        Self { status, kind, code: None, message: message.into() }
    }

    fn bad_request(message: impl Into<String>) -> Self {
        Self::new("400 Bad Request", "invalid_request_error", message)
    }

    fn body(&self) -> Vec<u8> {
        json!({ "error": { "message": self.message, "type": self.kind, "param": null, "code": self.code } }).to_string().into_bytes()
    }
}

/// Answers one request under `API_PREFIX`; the connection is done afterwards.
pub(crate) async fn serve<S: AsyncRead + AsyncWrite + Unpin>(stream: &mut S, head: &RequestHead, api: &ApiContext) -> std::io::Result<()> {
    match route(stream, head, api).await {
        Ok(()) => Ok(()),
        Err(err) => write_json(stream, err.status, &err.body()).await,
    }
}

async fn route<S: AsyncRead + AsyncWrite + Unpin>(stream: &mut S, head: &RequestHead, api: &ApiContext) -> Result<(), ApiError> {
    let Some(keys_path) = &api.keys_path else {
        return Err(ApiError::new("404 Not Found", "invalid_request_error", "this hub doesn't offer the Warden API"));
    };
    let key = authenticate(head, &ApiKeyStore::new(keys_path.as_ref().clone())).await?;
    let path = head.path.split('?').next().unwrap_or_default();
    match (head.method.as_str(), path) {
        ("GET", "/v1/models") => {
            let body = json!({ "object": "list", "data": model_ids(api, &key).into_iter().map(|id| json!({ "id": id, "object": "model", "created": 0, "owned_by": "warden" })).collect::<Vec<_>>() });
            write_json(stream, "200 OK", body.to_string().as_bytes()).await.map_err(io_error)
        }
        ("POST", "/v1/chat/completions") => {
            let body = read_body(stream, head).await?;
            let request: Value = serde_json::from_slice(&body).map_err(|e| ApiError::bad_request(format!("the body isn't valid JSON: {e}")))?;
            chat_completions(stream, api, &key, &request).await
        }
        (_, "/v1/models" | "/v1/chat/completions") => Err(ApiError::new("405 Method Not Allowed", "invalid_request_error", format!("{} isn't allowed here", head.method))),
        _ => Err(ApiError::new("404 Not Found", "invalid_request_error", format!("unknown route {path} — this hub serves /v1/models and /v1/chat/completions"))),
    }
}

/// The key in `Authorization: Bearer ...`. A missing or wrong one waits `WRONG_KEY_DELAY` first,
/// the same guessing rate as the pairing key.
async fn authenticate(head: &RequestHead, store: &ApiKeyStore) -> Result<ApiKey, ApiError> {
    let presented = head.header("authorization").and_then(|v| v.strip_prefix("Bearer ").or_else(|| v.strip_prefix("bearer "))).map(str::trim);
    let found = match presented {
        Some(key) => store.authenticate(key).map_err(|e| ApiError::new("500 Internal Server Error", "server_error", format!("could not read the API keys: {e:#}")))?,
        None => None,
    };
    match found {
        Some(key) => Ok(key),
        None => {
            tokio::time::sleep(WRONG_KEY_DELAY).await;
            let mut err = ApiError::new("401 Unauthorized", "invalid_request_error", "missing or invalid API key — create one in the Warden app (Settings → Warden API)");
            err.code = Some("invalid_api_key".into());
            Err(err)
        }
    }
}

async fn read_body<S: AsyncRead + Unpin>(stream: &mut S, head: &RequestHead) -> Result<Vec<u8>, ApiError> {
    if head.header("transfer-encoding").is_some_and(|v| v.to_ascii_lowercase().contains("chunked")) {
        return Err(ApiError::new("411 Length Required", "invalid_request_error", "send the body with a Content-Length"));
    }
    let length: usize = head
        .header("content-length")
        .ok_or_else(|| ApiError::new("411 Length Required", "invalid_request_error", "send the body with a Content-Length"))?
        .trim()
        .parse()
        .map_err(|_| ApiError::bad_request("Content-Length isn't a number"))?;
    if length > MAX_BODY_BYTES {
        return Err(ApiError::new("413 Payload Too Large", "invalid_request_error", format!("the body is over {} MiB", MAX_BODY_BYTES / 1024 / 1024)));
    }
    let mut body = head.raw[head.body_start.min(head.raw.len())..].to_vec();
    body.truncate(length);
    let rest = length - body.len();
    if rest > 0 {
        let start = body.len();
        body.resize(length, 0);
        tokio::time::timeout(BODY_TIMEOUT, stream.read_exact(&mut body[start..]))
            .await
            .map_err(|_| ApiError::bad_request("the body took too long to arrive"))?
            .map_err(|_| ApiError::bad_request("the connection closed before the whole body arrived"))?;
    }
    Ok(body)
}

/// `warden`, then `warden/<agent>` for every configured agent — or, for a key bound to an agent,
/// only that agent's.
fn model_ids(api: &ApiContext, key: &ApiKey) -> Vec<String> {
    if let Some(agent) = &key.agent_id {
        return vec![format!("{DEFAULT_MODEL}/{agent}")];
    }
    let mut ids = vec![DEFAULT_MODEL.to_string()];
    if let Some(host) = &api.settings {
        if let Ok(config) = load_config_from_path(&host.config_path(), false) {
            // P84: the agents this key's person sees.
            ids.extend(config.agents.iter().filter(|a| agent_visible_to(a, key.user.as_deref())).map(|a| format!("{DEFAULT_MODEL}/{}", a.id)));
        }
    }
    ids
}

/// The request's history, last user message and client system text. `input` is `None` when the
/// request ends with the results of the client's tools: the turn continues instead of starting.
struct Turn {
    history: Vec<Message>,
    input: Option<String>,
    system: Vec<String>,
}

/// Separates the id of a call from the Gemini signature riding in it (see the module doc).
const SIGNATURE_MARK: &str = "__ts_";

/// A fresh id for a call handed to the client, carrying `signature` when there is one.
fn encode_call_id(signature: Option<&str>) -> String {
    use base64::Engine;
    let id = format!("call_{}", &warden_bootstrap::generate_auth_key()[..24]);
    match signature {
        Some(signature) => format!("{id}{SIGNATURE_MARK}{}", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signature)),
        None => id,
    }
}

/// The id to use with the provider and the signature, from an id the client sent back. An id the
/// hub didn't make (or a mangled signature) is kept whole, without a signature.
fn decode_call_id(id: &str) -> (String, Option<String>) {
    use base64::Engine;
    if let Some((base, encoded)) = id.split_once(SIGNATURE_MARK) {
        let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(encoded).ok().and_then(|bytes| String::from_utf8(bytes).ok());
        if let Some(signature) = decoded {
            return (base.to_string(), Some(signature));
        }
    }
    (id.to_string(), None)
}

/// The client's `tools`, as the model is offered them. `tool_choice: "none"` offers none.
fn client_tools(request: &Value) -> Result<Vec<ToolSpec>, ApiError> {
    if request.get("tool_choice").and_then(Value::as_str) == Some("none") {
        return Ok(Vec::new());
    }
    let Some(tools) = request.get("tools").filter(|t| !t.is_null()) else { return Ok(Vec::new()) };
    let tools = tools.as_array().ok_or_else(|| ApiError::bad_request("'tools' must be a list"))?;
    tools
        .iter()
        .map(|tool| {
            if tool.get("type").and_then(Value::as_str) != Some("function") {
                return Err(ApiError::bad_request("only tools of type 'function' are supported"));
            }
            let function = tool.get("function").ok_or_else(|| ApiError::bad_request("a tool needs its 'function'"))?;
            let name = function.get("name").and_then(Value::as_str).filter(|n| !n.is_empty()).ok_or_else(|| ApiError::bad_request("a tool needs a 'name'"))?;
            Ok(ToolSpec {
                name: name.to_string(),
                description: function.get("description").and_then(Value::as_str).unwrap_or_default().to_string(),
                parameters: function.get("parameters").cloned().unwrap_or_else(|| json!({ "type": "object", "properties": {} })),
            })
        })
        .collect()
}

/// An assistant message's `tool_calls`, as the model made them.
fn parse_tool_calls(message: &Value) -> Result<Vec<ToolCall>, ApiError> {
    let Some(calls) = message.get("tool_calls").filter(|c| !c.is_null()) else { return Ok(Vec::new()) };
    let calls = calls.as_array().ok_or_else(|| ApiError::bad_request("'tool_calls' must be a list"))?;
    calls
        .iter()
        .map(|call| {
            let id = call.get("id").and_then(Value::as_str).ok_or_else(|| ApiError::bad_request("a tool call needs its 'id'"))?;
            let name = call.pointer("/function/name").and_then(Value::as_str).ok_or_else(|| ApiError::bad_request("a tool call needs 'function.name'"))?;
            // `arguments` is a JSON string; one the model got wrong is sent back as no arguments.
            let arguments = call.pointer("/function/arguments").and_then(Value::as_str).and_then(|raw| serde_json::from_str(raw).ok()).unwrap_or_else(|| json!({}));
            let (id, thought_signature) = decode_call_id(id);
            Ok(ToolCall { id, name: name.to_string(), arguments, thought_signature })
        })
        .collect()
}

/// The calls handed to the client, in OpenAI's shape, with the hub's ids. With `index` for a
/// streamed delta.
fn tool_calls_json(calls: &[ToolCall], streamed: bool) -> Vec<Value> {
    calls
        .iter()
        .enumerate()
        .map(|(index, call)| {
            let arguments = if call.arguments.is_null() { "{}".to_string() } else { call.arguments.to_string() };
            let mut value = json!({ "id": encode_call_id(call.thought_signature.as_deref()), "type": "function", "function": { "name": call.name, "arguments": arguments } });
            if streamed {
                value["index"] = json!(index);
            }
            value
        })
        .collect()
}

/// The text of one message's `content`: a string, `null`, or a list of text parts. Anything else
/// (an image) isn't supported yet.
fn content_text(content: Option<&Value>) -> Result<String, ApiError> {
    match content {
        None | Some(Value::Null) => Ok(String::new()),
        Some(Value::String(text)) => Ok(text.clone()),
        Some(Value::Array(parts)) => {
            let mut text = String::new();
            for part in parts {
                match (part.get("type").and_then(Value::as_str), part.get("text").and_then(Value::as_str)) {
                    (Some("text"), Some(fragment)) => text.push_str(fragment),
                    (kind, _) => return Err(ApiError::bad_request(format!("content parts of type '{}' aren't supported yet — send text", kind.unwrap_or("?")))),
                }
            }
            Ok(text)
        }
        Some(_) => Err(ApiError::bad_request("a message's content must be a string or a list of text parts")),
    }
}

fn parse_turn(request: &Value) -> Result<Turn, ApiError> {
    let messages = request.get("messages").and_then(Value::as_array).ok_or_else(|| ApiError::bad_request("'messages' is required"))?;
    let (last, earlier) = messages.split_last().ok_or_else(|| ApiError::bad_request("'messages' is empty"))?;
    // A request ends with the user's message (a new turn) or the client's tool results (the turn
    // it stopped continues), which are part of the history.
    let (input, earlier) = match last.get("role").and_then(Value::as_str) {
        Some("user") => {
            let input = content_text(last.get("content"))?;
            if input.trim().is_empty() {
                return Err(ApiError::bad_request("the last message is empty"));
            }
            (Some(input), earlier)
        }
        Some("tool") => (None, messages.as_slice()),
        _ => return Err(ApiError::bad_request("the last message must be the user's or a tool result")),
    };
    let mut history = Vec::new();
    let mut system = Vec::new();
    // Which tool each call id is for: a result names only the call, and Gemini keys results by name.
    let mut called: Vec<ToolCall> = Vec::new();
    for message in earlier {
        let text = content_text(message.get("content"))?;
        match message.get("role").and_then(Value::as_str) {
            Some("system" | "developer") if !text.trim().is_empty() => system.push(text),
            Some("user") => history.push(Message::user(text)),
            Some("assistant") => {
                let calls = parse_tool_calls(message)?;
                if calls.is_empty() {
                    if !text.is_empty() {
                        history.push(Message::assistant(text));
                    }
                } else {
                    called.extend(calls.iter().cloned());
                    history.push(Message { content: text, ..Message::assistant_tool_calls(calls) });
                }
            }
            Some("tool") => {
                let id = message.get("tool_call_id").and_then(Value::as_str).ok_or_else(|| ApiError::bad_request("a tool message needs its 'tool_call_id'"))?;
                let (id, _) = decode_call_id(id);
                let call = called
                    .iter()
                    .find(|call| call.id == id)
                    .ok_or_else(|| ApiError::bad_request(format!("the tool message for '{id}' answers no earlier tool call")))?;
                history.push(Message::tool_result(call, text));
            }
            _ => {}
        }
    }
    Ok(Turn { history, input, system })
}

/// The model a request actually gets. A general key takes what it asks for (`warden` when it asks
/// for nothing). A key bound to agent `X` only speaks as `X`: nothing, `warden` or `warden/X` is
/// `warden/X`, and any other agent is refused.
fn effective_model(key: &ApiKey, requested: Option<&str>) -> Result<String, ApiError> {
    let Some(agent) = &key.agent_id else {
        return Ok(requested.unwrap_or(DEFAULT_MODEL).to_string());
    };
    let bound = format!("{DEFAULT_MODEL}/{agent}");
    match requested {
        None => Ok(bound),
        Some(model) if model == DEFAULT_MODEL || model == bound => Ok(bound),
        Some(model) => {
            let mut err = ApiError::new("403 Forbidden", "permission_error", format!("this key only speaks as agent '{agent}' (model '{bound}'), not '{model}'"));
            err.code = Some("model_not_allowed".into());
            Err(err)
        }
    }
}

/// The orchestrator and persona for `model`: the hub's own, or a configured agent's — narrowed to
/// the member's space when the key is a member's (P84).
fn scope_model(api: &ApiContext, key: &ApiKey, model: &str) -> Result<(Orchestrator, Option<String>), ApiError> {
    let (orchestrator, persona) = scope_agent(api, key, model)?;
    let Some(user) = &key.user else {
        return Ok((orchestrator.with_conversations_dir(&warden_bootstrap::users::root_conversations_dir(&api.conversations_root)), persona));
    };
    let gone = || {
        let mut err = ApiError::new("403 Forbidden", "permission_error", "the person this key belongs to is no longer part of the workspace");
        err.code = Some("user_gone".into());
        err
    };
    let host = api.settings.as_ref().ok_or_else(gone)?;
    let users_dir = api.users_dir.as_ref().ok_or_else(gone)?;
    let config: FileConfig = load_config_from_path(&host.config_path(), false).map_err(|e| ApiError::new("500 Internal Server Error", "server_error", format!("{e:#}")))?;
    let member = config.users.iter().find(|u| &u.id == user).ok_or_else(gone)?;
    let space = MemberSpace::new(member, users_dir, &api.conversations_root);
    let tools = tools_for(&orchestrator, &config, user);
    mount_member_spaces(&space, &config, orchestrator.vault().root(), &api.space_vaults);
    Ok((member_orchestrator(&orchestrator, &space, &tools, "api", &key.name), persona))
}

fn scope_agent(api: &ApiContext, key: &ApiKey, model: &str) -> Result<(Orchestrator, Option<String>), ApiError> {
    let base = api.orchestrator.current().with_spend_context(SpendContext::new("api").with_user(key.name.clone()));
    if model == DEFAULT_MODEL {
        return Ok((base, None));
    }
    let unknown = || match &key.agent_id {
        // The key's own agent was deleted or renamed: refused, never the hub's default instead.
        Some(agent) => {
            let mut err = ApiError::new("403 Forbidden", "permission_error", format!("the agent this key is bound to ('{agent}') no longer exists — create a new key"));
            err.code = Some("agent_gone".into());
            err
        }
        None => {
            let mut err = ApiError::new("404 Not Found", "invalid_request_error", format!("the model '{model}' does not exist — see GET /v1/models"));
            err.code = Some("model_not_found".into());
            err
        }
    };
    let agent_id = model.strip_prefix(&format!("{DEFAULT_MODEL}/")).ok_or_else(unknown)?;
    let host = api.settings.as_ref().ok_or_else(unknown)?;
    let path = host.config_path();
    let config = load_config_from_path(&path, false).map_err(|e| ApiError::new("500 Internal Server Error", "server_error", format!("{e:#}")))?;
    // P84: only an agent this key's person sees.
    if !config.agents.iter().any(|a| a.id == agent_id && agent_visible_to(a, key.user.as_deref())) {
        return Err(unknown());
    }
    // No conversations directory (nothing is saved) and no approver: `message_agent` isn't there,
    // and a tool that needs a person's yes is refused.
    let scoped = scope_to_agent(&base, &config, Some(&path), agent_id, AgentExtras::default()).ok_or_else(unknown)?;
    let mut orchestrator = scoped.orchestrator;
    if let Some(provider_id) = &scoped.provider_id {
        let model = build_model_for(&config, provider_id, None)
            .map_err(|e| ApiError::new("500 Internal Server Error", "server_error", format!("agent '{agent_id}' can't use its model '{provider_id}': {e:#}")))?;
        orchestrator = orchestrator.with_model(model);
    }
    Ok((orchestrator, Some(scoped.persona)))
}

fn turn_error(err: &anyhow::Error) -> ApiError {
    match spend_limit_id(err) {
        Some(limit) => {
            let mut api = ApiError::new("429 Too Many Requests", "insufficient_quota", format!("{err:#}"));
            api.code = Some(format!("spend_limit:{limit}"));
            api
        }
        None => ApiError::new("500 Internal Server Error", "server_error", format!("{err:#}")),
    }
}

fn usage_json(usage: Option<&Usage>) -> Value {
    match usage {
        Some(u) => json!({ "prompt_tokens": u.prompt_tokens, "completion_tokens": u.completion_tokens, "total_tokens": u.total_tokens }),
        None => json!({ "prompt_tokens": 0, "completion_tokens": 0, "total_tokens": 0 }),
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs()
}

fn completion_id() -> String {
    format!("chatcmpl-{}", &warden_bootstrap::generate_auth_key()[..24])
}

async fn chat_completions<S: AsyncWrite + Unpin>(stream: &mut S, api: &ApiContext, key: &ApiKey, request: &Value) -> Result<(), ApiError> {
    let model = effective_model(key, request.get("model").and_then(Value::as_str))?;
    let Turn { history, input, system } = parse_turn(request)?;
    let tools = client_tools(request)?;
    let (orchestrator, persona) = scope_model(api, key, &model)?;
    let orchestrator = orchestrator.with_client_tools(tools);
    let system_prompt: Option<String> = {
        let parts: Vec<&str> = persona.iter().map(String::as_str).chain(system.iter().map(String::as_str)).collect();
        (!parts.is_empty()).then(|| parts.join("\n\n"))
    };
    let id = completion_id();
    let created = now_secs();

    if request.get("stream").and_then(Value::as_bool) != Some(true) {
        let outcome = run_turn(&orchestrator, &history, input.as_deref(), system_prompt.as_deref(), |_| {}).await.map_err(|e| turn_error(&e))?;
        let message = if outcome.client_tool_calls.is_empty() {
            json!({ "role": "assistant", "content": outcome.content })
        } else {
            let content = (!outcome.content.is_empty()).then_some(outcome.content.as_str());
            json!({ "role": "assistant", "content": content, "tool_calls": tool_calls_json(&outcome.client_tool_calls, false) })
        };
        let body = json!({
            "id": id,
            "object": "chat.completion",
            "created": created,
            "model": model,
            "choices": [{ "index": 0, "message": message, "finish_reason": finish_reason(&outcome) }],
            "usage": usage_json(outcome.usage.as_ref()),
        });
        return write_json(stream, "200 OK", body.to_string().as_bytes()).await.map_err(io_error);
    }

    let include_usage = request.pointer("/stream_options/include_usage").and_then(Value::as_bool) == Some(true);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    // Only the text goes out live: whether a tool call is the client's is known once the turn ends.
    let turn = tokio::spawn(async move {
        run_turn(&orchestrator, &history, input.as_deref(), system_prompt.as_deref(), move |event| {
            if let StreamEvent::ContentDelta(text) = event {
                let _ = tx.send(text.clone());
            }
        })
        .await
    });

    // The status line can only be sent once: the first delta, or a finished turn, decides it.
    let mut started = false;
    let chunk = |delta: Value, finish: Option<&str>| {
        json!({ "id": id, "object": "chat.completion.chunk", "created": created, "model": model, "choices": [{ "index": 0, "delta": delta, "finish_reason": finish }] })
    };
    while let Some(text) = rx.recv().await {
        if !started {
            start_event_stream(stream).await.map_err(io_error)?;
            send_event(stream, &chunk(json!({ "role": "assistant", "content": "" }), None)).await.map_err(io_error)?;
            started = true;
        }
        send_event(stream, &chunk(json!({ "content": text }), None)).await.map_err(io_error)?;
    }
    let outcome = turn.await.map_err(|e| ApiError::new("500 Internal Server Error", "server_error", format!("{e}")))?;
    let outcome = match outcome {
        Ok(outcome) => outcome,
        Err(err) if !started => return Err(turn_error(&err)),
        Err(err) => {
            // Mid-stream: the status is sent already, so the error goes as one more event.
            let error = turn_error(&err);
            let body: Value = serde_json::from_slice(&error.body()).unwrap_or_default();
            send_event(stream, &body).await.map_err(io_error)?;
            return finish_event_stream(stream).await.map_err(io_error);
        }
    };
    if !started {
        start_event_stream(stream).await.map_err(io_error)?;
        send_event(stream, &chunk(json!({ "role": "assistant", "content": outcome.content }), None)).await.map_err(io_error)?;
    }
    for call in tool_calls_json(&outcome.client_tool_calls, true) {
        send_event(stream, &chunk(json!({ "tool_calls": [call] }), None)).await.map_err(io_error)?;
    }
    send_event(stream, &chunk(json!({}), Some(finish_reason(&outcome)))).await.map_err(io_error)?;
    if include_usage {
        let usage = json!({ "id": id, "object": "chat.completion.chunk", "created": created, "model": model, "choices": [], "usage": usage_json(outcome.usage.as_ref()) });
        send_event(stream, &usage).await.map_err(io_error)?;
    }
    finish_event_stream(stream).await.map_err(io_error)
}

/// A new turn for the user's `input`, or the one that stopped on the client's tools when `None`.
async fn run_turn(
    orchestrator: &Orchestrator,
    history: &[Message],
    input: Option<&str>,
    system_prompt: Option<&str>,
    on_event: impl FnMut(&StreamEvent) + Send,
) -> anyhow::Result<MessageOutcome> {
    match input {
        Some(input) => orchestrator.handle_turn_streaming(history, input, Vec::new(), system_prompt, on_event).await,
        None => orchestrator.resume_turn_streaming(history, system_prompt, on_event).await,
    }
}

fn finish_reason(outcome: &MessageOutcome) -> &'static str {
    if outcome.client_tool_calls.is_empty() {
        "stop"
    } else {
        "tool_calls"
    }
}

fn io_error(err: std::io::Error) -> ApiError {
    ApiError::new("500 Internal Server Error", "server_error", format!("{err}"))
}

async fn write_json<S: AsyncWrite + Unpin>(stream: &mut S, status: &str, body: &[u8]) -> std::io::Result<()> {
    write_response(stream, status, &[], "application/json", body, false).await
}

async fn start_event_stream<S: AsyncWrite + Unpin>(stream: &mut S) -> std::io::Result<()> {
    stream
        .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n")
        .await?;
    stream.flush().await
}

async fn send_event<S: AsyncWrite + Unpin>(stream: &mut S, event: &Value) -> std::io::Result<()> {
    stream.write_all(format!("data: {event}\n\n").as_bytes()).await?;
    stream.flush().await
}

async fn finish_event_stream<S: AsyncWrite + Unpin>(stream: &mut S) -> std::io::Result<()> {
    stream.write_all(b"data: [DONE]\n\n").await?;
    stream.flush().await?;
    stream.shutdown().await
}

#[cfg(test)]
mod tests {
    use super::*;

    use warden_core::model::Role;

    #[test]
    fn a_turn_keeps_the_text_and_the_clients_tool_calls_and_results() {
        let request = json!({ "messages": [
            { "role": "system", "content": "Be brief." },
            { "role": "user", "content": "first" },
            { "role": "assistant", "content": null, "tool_calls": [{ "id": "c1", "type": "function", "function": { "name": "x", "arguments": "{\"a\":1}" } }] },
            { "role": "tool", "tool_call_id": "c1", "content": "tool output" },
            { "role": "assistant", "content": [{ "type": "text", "text": "an answer" }] },
            { "role": "user", "content": "second" },
        ]});
        let turn = parse_turn(&request).ok().unwrap();
        assert_eq!(turn.system, ["Be brief."]);
        assert_eq!(turn.input.as_deref(), Some("second"));
        let roles: Vec<_> = turn.history.iter().map(|m| m.role).collect();
        assert_eq!(roles, [Role::User, Role::Assistant, Role::Tool, Role::Assistant]);
        assert_eq!(turn.history[1].tool_calls[0].name, "x");
        assert_eq!(turn.history[1].tool_calls[0].arguments, json!({ "a": 1 }));
        assert_eq!(turn.history[2].tool_name.as_deref(), Some("x"));
        assert_eq!(turn.history[2].content, "tool output");
    }

    #[test]
    fn a_request_ending_in_tool_results_continues_the_turn() {
        let id = encode_call_id(Some("gemini-signature+/="));
        let request = json!({ "messages": [
            { "role": "user", "content": "weather?" },
            { "role": "assistant", "content": "checking", "tool_calls": [{ "id": id, "type": "function", "function": { "name": "get_weather", "arguments": "not json" } }] },
            { "role": "tool", "tool_call_id": id, "content": "sunny" },
        ]});
        let turn = parse_turn(&request).ok().unwrap();
        assert_eq!(turn.input, None);
        assert_eq!(turn.history.len(), 3);
        let call = &turn.history[1].tool_calls[0];
        assert_eq!(turn.history[1].content, "checking");
        assert_eq!(call.arguments, json!({}));
        assert_eq!(call.thought_signature.as_deref(), Some("gemini-signature+/="));
        assert!(!call.id.contains(SIGNATURE_MARK), "the provider gets the short id");
        assert_eq!(turn.history[2].tool_call_id.as_deref(), Some(call.id.as_str()));
        assert_eq!(turn.history[2].tool_name.as_deref(), Some("get_weather"));
    }

    #[test]
    fn a_tool_result_for_no_call_is_refused() {
        let request = json!({ "messages": [
            { "role": "user", "content": "hi" },
            { "role": "tool", "tool_call_id": "ghost", "content": "x" },
        ]});
        assert!(parse_turn(&request).err().unwrap().message.contains("ghost"));
    }

    #[test]
    fn call_ids_are_unique_and_carry_the_signature_there_and_back() {
        let plain = encode_call_id(None);
        assert!(plain.starts_with("call_") && plain.len() == 29, "{plain}");
        assert_ne!(plain, encode_call_id(None));
        assert_eq!(decode_call_id(&plain), (plain.clone(), None));

        let signed = encode_call_id(Some("opaque blob"));
        assert!(signed.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'), "{signed}");
        let (base, signature) = decode_call_id(&signed);
        assert_eq!(signature.as_deref(), Some("opaque blob"));
        assert!(signed.starts_with(&base) && base.len() == 29);

        // An id the hub didn't make passes through as it is.
        assert_eq!(decode_call_id("toolu_01abc"), ("toolu_01abc".to_string(), None));
        assert_eq!(decode_call_id("x__ts_!!!"), ("x__ts_!!!".to_string(), None));
    }

    #[test]
    fn the_clients_tools_are_read_and_none_leaves_them_out() {
        let tools = json!([{ "type": "function", "function": { "name": "get_weather", "description": "d", "parameters": { "type": "object" } } }]);
        let specs = client_tools(&json!({ "tools": tools })).ok().unwrap();
        assert_eq!(specs.len(), 1);
        assert_eq!((specs[0].name.as_str(), specs[0].description.as_str()), ("get_weather", "d"));
        assert!(client_tools(&json!({ "tools": tools, "tool_choice": "none" })).ok().unwrap().is_empty());
        assert_eq!(client_tools(&json!({ "tools": tools, "tool_choice": "required" })).ok().unwrap().len(), 1);
        assert!(client_tools(&json!({})).ok().unwrap().is_empty());
        assert!(client_tools(&json!({ "tools": [{ "type": "custom" }] })).is_err());
    }

    #[test]
    fn a_turn_must_end_with_the_users_text() {
        let error = |request: Value| parse_turn(&request).err().map(|e| e.message).unwrap_or_default();
        assert!(error(json!({})).contains("'messages' is required"));
        assert!(error(json!({ "messages": [] })).contains("empty"));
        assert!(error(json!({ "messages": [{ "role": "assistant", "content": "x" }] })).contains("the user's or a tool result"));
        assert!(error(json!({ "messages": [{ "role": "user", "content": [{ "type": "image_url", "image_url": { "url": "data:" } }] }] })).contains("image_url"));
    }
}
