//! Incoming webhooks (P105) on the hub: `POST /hooks/<id>` with the webhook's token runs the agent the `[[webhooks]]`
//! entry names, with its prompt and the request body as data (`warden_bootstrap::webhooks`). The call is answered at
//! once with `202`; the run goes on in the background and its result lands in the webhook's conversation, which every
//! device lists (the task conversations' directory), announced like a task's run.
//!
//! The order of a call matters, because the caller is a stranger until its token is checked:
//! 1. the method (only `POST`), then the token — **before** the body is read, so nobody without one makes the hub take
//!    256 KiB. A wrong token, another webhook's token and an id that doesn't exist all answer the same `401`, after
//!    `WRONG_KEY_DELAY` (the guessing rate of the pairing key), so the answer doesn't tell which ids exist;
//! 2. the webhook in the config (read on every call, so pausing or removing one holds at once): gone → `404`, paused →
//!    `403`;
//! 3. the body: `Content-Length` required, at most `MAX_BODY_BYTES`;
//! 4. the run: one at a time per webhook — a call while the last one is still working is `409`, never queued.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};
use tokio::sync::broadcast;
use warden_bootstrap::webhooks::{conversation_id, run_webhook, WebhookConfig, WebhookRequest, MAX_WEBHOOK_ID_LEN};
use warden_bootstrap::{load_config_from_path, FileConfig};
use warden_core::orchestrator::Orchestrator;

use crate::settings::{SettingsHost, SharedOrchestrator, WRONG_KEY_DELAY};
use crate::web_ui::{write_response, RequestHead};
use crate::webhook_tokens::WebhookTokenStore;

/// Every webhook's URL starts with this.
pub const HOOKS_PREFIX: &str = "/hooks/";

/// The largest request body accepted. The model sees far less of it (`MAX_PAYLOAD_CHARS`).
pub const MAX_BODY_BYTES: usize = 256 * 1024;

/// How long a caller gets to send the whole body.
const BODY_TIMEOUT: Duration = Duration::from_secs(30);

/// Runs webhooks for one hub: the ones working now and where their conversations are. Cheap to clone; every connection
/// shares the same `running` set, so two calls to one webhook never overlap.
#[derive(Clone)]
pub struct WebhookRunner {
    conversations_dir: PathBuf,
    running: Arc<Mutex<HashSet<String>>>,
    changes: broadcast::Sender<String>,
}

impl WebhookRunner {
    /// `conversations_dir`: where the task conversations live (`TaskStore::conversations_dir`); `changes`: where a
    /// finished run is announced.
    pub fn new(conversations_dir: PathBuf, changes: broadcast::Sender<String>) -> Self {
        Self { conversations_dir, running: Arc::default(), changes }
    }

    /// Starts one call in the background, or returns `false` when the webhook's last call hasn't finished.
    pub fn start(&self, base: Arc<Orchestrator>, config: Arc<FileConfig>, config_path: PathBuf, hook: WebhookConfig, content_type: Option<String>, body: Vec<u8>) -> bool {
        if !self.running.lock().unwrap_or_else(|e| e.into_inner()).insert(hook.id.clone()) {
            return false;
        }
        let this = self.clone();
        tokio::spawn(async move {
            eprintln!("warden-server: running webhook '{}'", hook.id);
            let request = WebhookRequest { content_type: content_type.as_deref(), body: &body };
            match run_webhook(&base, &config, Some(&config_path), &hook, &this.conversations_dir, request, now_millis()).await {
                Ok(_) => eprintln!("warden-server: webhook '{}' done", hook.id),
                Err(err) => eprintln!("warden-server: webhook '{}' failed: {err:#}", hook.id),
            }
            this.running.lock().unwrap_or_else(|e| e.into_inner()).remove(&hook.id);
            // Nobody may be connected; that's fine.
            let _ = this.changes.send(conversation_id(&hook.id));
        });
        true
    }
}

/// What the webhook routes need from the hub.
#[derive(Clone)]
pub(crate) struct WebhookContext {
    pub orchestrator: SharedOrchestrator,
    pub settings: Option<Arc<dyn SettingsHost>>,
    pub tokens_path: Arc<PathBuf>,
    /// `None`: this hub has no place to put the conversations (no `with_tasks`), so it offers no webhooks.
    pub runner: Option<WebhookRunner>,
}

/// A refusal: the HTTP status, a short code and what to tell the caller.
struct Refusal {
    status: &'static str,
    code: &'static str,
    message: String,
}

impl Refusal {
    fn new(status: &'static str, code: &'static str, message: impl Into<String>) -> Self {
        Self { status, code, message: message.into() }
    }
}

/// Answers one request under `HOOKS_PREFIX`; the connection is done afterwards.
pub(crate) async fn serve<S: AsyncRead + AsyncWrite + Unpin>(stream: &mut S, head: &RequestHead, ctx: &WebhookContext) -> std::io::Result<()> {
    let (status, body) = match route(stream, head, ctx).await {
        Ok(body) => ("202 Accepted", body),
        Err(refusal) => (refusal.status, json!({ "error": { "code": refusal.code, "message": refusal.message } })),
    };
    write_response(stream, status, &[], "application/json", body.to_string().as_bytes(), false).await
}

async fn route<S: AsyncRead + Unpin>(stream: &mut S, head: &RequestHead, ctx: &WebhookContext) -> Result<Value, Refusal> {
    let Some(runner) = &ctx.runner else {
        return Err(Refusal::new("404 Not Found", "no_webhooks", "this hub doesn't offer webhooks"));
    };
    let path = head.path.split('?').next().unwrap_or_default();
    let id = path.strip_prefix(HOOKS_PREFIX).unwrap_or_default().trim_end_matches('/');
    // An id that can't be one is refused like an unknown one, and before anything is read.
    let id_is_possible = !id.is_empty() && id.len() <= MAX_WEBHOOK_ID_LEN && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if head.method != "POST" {
        return Err(Refusal::new("405 Method Not Allowed", "method_not_allowed", format!("{} isn't allowed here — POST to /hooks/<id>", head.method)));
    }

    let presented = presented_token(head);
    let known = match (id_is_possible, presented) {
        (true, Some(token)) => WebhookTokenStore::new(ctx.tokens_path.as_ref().clone())
            .authenticate(id, token)
            .map_err(|e| Refusal::new("500 Internal Server Error", "server_error", format!("could not read the webhook tokens: {e:#}")))?,
        _ => false,
    };
    if !known {
        tokio::time::sleep(WRONG_KEY_DELAY).await;
        return Err(Refusal::new("401 Unauthorized", "invalid_token", "missing or invalid webhook token — create one with `warden-server webhooks token <id>`"));
    }

    let Some(settings) = &ctx.settings else {
        return Err(Refusal::new("404 Not Found", "no_webhooks", "this hub has no settings file, so no webhooks"));
    };
    let config_path = settings.config_path();
    let config = load_config_from_path(&config_path, false).map_err(|e| Refusal::new("500 Internal Server Error", "server_error", format!("could not read the config: {e:#}")))?;
    let Some(hook) = config.webhooks.iter().find(|h| h.id == id).cloned() else {
        return Err(Refusal::new("404 Not Found", "unknown_webhook", format!("there is no webhook '{id}' (it may have been removed)")));
    };
    if !hook.enabled {
        return Err(Refusal::new("403 Forbidden", "webhook_paused", format!("webhook '{id}' is paused")));
    }

    let content_type = head.header("content-type").map(str::to_string);
    let body = read_body(stream, head).await?;
    let conversation = conversation_id(id);
    if !runner.start(ctx.orchestrator.current(), Arc::new(config), config_path, hook, content_type, body) {
        return Err(Refusal::new("409 Conflict", "still_running", format!("webhook '{id}' is still working on the last call — try again when it finishes")));
    }
    Ok(json!({ "status": "started", "webhook": id, "conversation": conversation }))
}

/// The token in `Authorization: Bearer ...`, or in `X-Warden-Token` for the services that can't set that header.
fn presented_token(head: &RequestHead) -> Option<&str> {
    head.header("authorization")
        .and_then(|v| v.strip_prefix("Bearer ").or_else(|| v.strip_prefix("bearer ")))
        .or_else(|| head.header("x-warden-token"))
        .map(str::trim)
        .filter(|t| !t.is_empty())
}

async fn read_body<S: AsyncRead + Unpin>(stream: &mut S, head: &RequestHead) -> Result<Vec<u8>, Refusal> {
    let needs_length = || Refusal::new("411 Length Required", "length_required", "send the body with a Content-Length");
    if head.header("transfer-encoding").is_some_and(|v| v.to_ascii_lowercase().contains("chunked")) {
        return Err(needs_length());
    }
    let length: usize = head
        .header("content-length")
        .ok_or_else(needs_length)?
        .trim()
        .parse()
        .map_err(|_| Refusal::new("400 Bad Request", "bad_request", "Content-Length isn't a number"))?;
    if length > MAX_BODY_BYTES {
        return Err(Refusal::new("413 Payload Too Large", "payload_too_large", format!("the body is over {} KiB", MAX_BODY_BYTES / 1024)));
    }
    let mut body = head.raw[head.body_start.min(head.raw.len())..].to_vec();
    body.truncate(length);
    let start = body.len();
    if start < length {
        body.resize(length, 0);
        tokio::time::timeout(BODY_TIMEOUT, stream.read_exact(&mut body[start..]))
            .await
            .map_err(|_| Refusal::new("400 Bad Request", "bad_request", "the body took too long to arrive"))?
            .map_err(|_| Refusal::new("400 Bad Request", "bad_request", "the connection closed before the whole body arrived"))?;
    }
    Ok(body)
}

fn now_millis() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as i64
}
