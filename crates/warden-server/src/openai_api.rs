//! The Warden API (P12): the hub's agent behind the OpenAI chat-completions wire format, on the same
//! port as the WebSocket protocol and the web UI. Anything that speaks to OpenAI (a script, n8n, a
//! chat client) can point its base URL at `http(s)://<hub>/v1` and a key made in the app
//! (`api_keys.rs`), and talk to the Warden: its vault, skills and tools, and a configured agent's
//! persona when `model` names one (`warden/<agent>`).
//!
//! Deliberately small, decisions of the user's:
//! - tools the client sends (`tools`/`tool_choice`, `tool` messages) are ignored: the agent answers
//!   with its own tools, in text;
//! - nothing is saved as a conversation; the spend goes to the `api` channel, per key, so spending
//!   limits (P4) apply;
//! - plain HTTP/1.1 written by hand like `web_ui.rs`: one request per connection, `Content-Length`
//!   bodies only, `Connection: close`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use warden_bootstrap::{build_model_for, load_config_from_path, scope_to_agent, AgentExtras};
use warden_core::model::{Message, StreamEvent, Usage};
use warden_core::orchestrator::Orchestrator;
use warden_core::spend::SpendContext;

use crate::api_keys::{ApiKey, ApiKeyStore};
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
            ids.extend(config.agents.iter().map(|a| format!("{DEFAULT_MODEL}/{}", a.id)));
        }
    }
    ids
}

/// The request's history, last user message and client system text.
struct Turn {
    history: Vec<Message>,
    input: String,
    system: Vec<String>,
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
    if last.get("role").and_then(Value::as_str) != Some("user") {
        return Err(ApiError::bad_request("the last message must be the user's"));
    }
    let input = content_text(last.get("content"))?;
    if input.trim().is_empty() {
        return Err(ApiError::bad_request("the last message is empty"));
    }
    let mut history = Vec::new();
    let mut system = Vec::new();
    for message in earlier {
        let text = content_text(message.get("content"))?;
        match message.get("role").and_then(Value::as_str) {
            Some("system" | "developer") if !text.trim().is_empty() => system.push(text),
            Some("user") => history.push(Message::user(text)),
            // An assistant turn that only asked for the client's tools has no text to keep.
            Some("assistant") if !text.is_empty() => history.push(Message::assistant(text)),
            // The client's own tool calls and results: ignored, like its `tools`.
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

/// The orchestrator and persona for `model`: the hub's own, or a configured agent's.
fn scope_model(api: &ApiContext, key: &ApiKey, model: &str) -> Result<(Orchestrator, Option<String>), ApiError> {
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
    let (orchestrator, persona) = scope_model(api, key, &model)?;
    let system_prompt: Option<String> = {
        let parts: Vec<&str> = persona.iter().map(String::as_str).chain(system.iter().map(String::as_str)).collect();
        (!parts.is_empty()).then(|| parts.join("\n\n"))
    };
    let id = completion_id();
    let created = now_secs();

    if request.get("stream").and_then(Value::as_bool) != Some(true) {
        let outcome = orchestrator.handle_turn(&history, &input, Vec::new(), system_prompt.as_deref()).await.map_err(|e| turn_error(&e))?;
        let body = json!({
            "id": id,
            "object": "chat.completion",
            "created": created,
            "model": model,
            "choices": [{ "index": 0, "message": { "role": "assistant", "content": outcome.content }, "finish_reason": "stop" }],
            "usage": usage_json(outcome.usage.as_ref()),
        });
        return write_json(stream, "200 OK", body.to_string().as_bytes()).await.map_err(io_error);
    }

    let include_usage = request.pointer("/stream_options/include_usage").and_then(Value::as_bool) == Some(true);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let turn = tokio::spawn(async move {
        orchestrator
            .handle_turn_streaming(&history, &input, Vec::new(), system_prompt.as_deref(), move |event| {
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
    send_event(stream, &chunk(json!({}), Some("stop"))).await.map_err(io_error)?;
    if include_usage {
        let usage = json!({ "id": id, "object": "chat.completion.chunk", "created": created, "model": model, "choices": [], "usage": usage_json(outcome.usage.as_ref()) });
        send_event(stream, &usage).await.map_err(io_error)?;
    }
    finish_event_stream(stream).await.map_err(io_error)
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

    #[test]
    fn a_turn_keeps_the_text_and_leaves_the_clients_tools_out() {
        let request = json!({ "messages": [
            { "role": "system", "content": "Be brief." },
            { "role": "user", "content": "first" },
            { "role": "assistant", "content": null, "tool_calls": [{ "id": "c1", "type": "function", "function": { "name": "x", "arguments": "{}" } }] },
            { "role": "tool", "tool_call_id": "c1", "content": "tool output" },
            { "role": "assistant", "content": [{ "type": "text", "text": "an answer" }] },
            { "role": "user", "content": "second" },
        ]});
        let turn = parse_turn(&request).ok().unwrap();
        assert_eq!(turn.system, ["Be brief."]);
        assert_eq!(turn.input, "second");
        let history: Vec<_> = turn.history.iter().map(|m| m.content.as_str()).collect();
        assert_eq!(history, ["first", "an answer"]);
    }

    #[test]
    fn a_turn_must_end_with_the_users_text() {
        let error = |request: Value| parse_turn(&request).err().map(|e| e.message).unwrap_or_default();
        assert!(error(json!({})).contains("'messages' is required"));
        assert!(error(json!({ "messages": [] })).contains("empty"));
        assert!(error(json!({ "messages": [{ "role": "assistant", "content": "x" }] })).contains("the user's"));
        assert!(error(json!({ "messages": [{ "role": "user", "content": [{ "type": "image_url", "image_url": { "url": "data:" } }] }] })).contains("image_url"));
    }
}
