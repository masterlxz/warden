//! Incoming webhooks (P105) on the hub: `POST /hooks/<id>` with the webhook's credential runs the agent the
//! `[[webhooks]]` entry names, with its prompt and the request body as data (`warden_bootstrap::webhooks`). The call is
//! answered at once with `202`; the run goes on in the background and its result lands in the webhook's conversation,
//! which every device lists (the task conversations' directory), announced like a task's run.
//!
//! The credential is a bearer token or, for services that sign what they send, an HMAC of the body
//! (`webhook_signature.rs`); which one a webhook takes is decided by the credential it has (`webhook_tokens.rs`), and the
//! config's `auth` has to agree with it.
//!
//! The order of a call matters, because the caller is a stranger until it has proven itself:
//! 1. the method (only `POST`), then the proof. For a **token** it comes **before** the body is read, so nobody without one
//!    makes the hub take 256 KiB. A **signature** can only be checked over the body, so for a webhook that has a signing
//!    secret the body is read first (same size cap and timeout) — which also means that the way an unknown id and a
//!    signed one answer differs in time by that read. Whatever the proof, a wrong one, another webhook's, one of the
//!    wrong kind and an id that doesn't exist all answer the same `401`, after `WRONG_KEY_DELAY` (the guessing rate of the
//!    pairing key), so the answer doesn't tell which ids exist;
//! 2. the webhook in the config (read on every call, so pausing or removing one holds at once): gone → `404`, paused →
//!    `403`;
//! 3. the body: `Content-Length` required, at most `MAX_BODY_BYTES`;
//! 4. for a signed call, a delivery id (`X-GitHub-Delivery`, `Idempotency-Key`) seen in the last hour is acknowledged and
//!    not run again — it is only believed because the signature was checked first;
//! 5. the run: one at a time per webhook — a call while the last one is still working is `409`, never queued.

use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};
use tokio::sync::broadcast;
use warden_bootstrap::webhooks::{conversation_id, run_webhook_notifying, WebhookAuth, WebhookConfig, WebhookRequest, MAX_WEBHOOK_ID_LEN};
use warden_bootstrap::{load_config_from_path, FileConfig};
use warden_core::orchestrator::Orchestrator;

use crate::settings::{SettingsHost, SharedOrchestrator, WRONG_KEY_DELAY};
use crate::web_ui::{write_response, RequestHead};
use crate::webhook_signature;
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
    /// The signed deliveries that already ran, newest last, with when (so a repeat can be told).
    deliveries: Arc<Mutex<VecDeque<(String, Instant)>>>,
}

/// How long a delivery id is remembered, and how many at most: a service that repeats a call does it within minutes.
const DELIVERY_MEMORY: Duration = Duration::from_secs(60 * 60);
const MAX_REMEMBERED_DELIVERIES: usize = 1024;

impl WebhookRunner {
    /// `conversations_dir`: where the task conversations live (`TaskStore::conversations_dir`); `changes`: where a
    /// finished run is announced.
    pub fn new(conversations_dir: PathBuf, changes: broadcast::Sender<String>) -> Self {
        Self { conversations_dir, running: Arc::default(), changes, deliveries: Arc::default() }
    }

    /// Whether this webhook already ran for this delivery id, within the last hour.
    pub fn seen_delivery(&self, webhook: &str, delivery: &str) -> bool {
        let key = format!("{webhook}\n{delivery}");
        let mut seen = self.deliveries.lock().unwrap_or_else(|e| e.into_inner());
        seen.retain(|(_, at)| at.elapsed() < DELIVERY_MEMORY);
        seen.iter().any(|(k, _)| *k == key)
    }

    /// Remembers that `webhook` ran for `delivery` (only once it really started: a call refused as busy may come again).
    pub fn remember_delivery(&self, webhook: &str, delivery: &str) {
        let mut seen = self.deliveries.lock().unwrap_or_else(|e| e.into_inner());
        seen.push_back((format!("{webhook}\n{delivery}"), Instant::now()));
        while seen.len() > MAX_REMEMBERED_DELIVERIES {
            seen.pop_front();
        }
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
            // P121: an agent that messages the person during the run changes its channel, which the connected devices should hear about.
            let changes = this.changes.clone();
            let on_changed: warden_bootstrap::ConversationsChanged = Arc::new(move |id: &str| {
                let _ = changes.send(id.to_string());
            });
            match run_webhook_notifying(&base, &config, Some(&config_path), &hook, &this.conversations_dir, request, now_millis(), Some(on_changed)).await {
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

    let tokens = WebhookTokenStore::new(ctx.tokens_path.as_ref().clone());
    let store_error = |e: anyhow::Error| Refusal::new("500 Internal Server Error", "server_error", format!("could not read the webhook credentials: {e:#}"));
    // What the caller must prove is decided by the credential the webhook has, not by what the caller claims.
    let kind = if id_is_possible { tokens.kind_of(id).map_err(store_error)? } else { None };
    let mut body = None;
    let proven = match kind {
        Some(WebhookAuth::Token) => match presented_token(head) {
            Some(token) => tokens.authenticate(id, token).map_err(store_error)?,
            None => false,
        },
        Some(WebhookAuth::Hmac) => {
            // A signature is made over the body, so for these the body has to come first (the same size cap and timeout).
            let received = read_body(stream, head).await?;
            let secret = tokens.secret_of(id).map_err(store_error)?;
            let good = secret.is_some_and(|secret| {
                let headers = webhook_signature::SignatureHeaders {
                    github: head.header("x-hub-signature-256"),
                    stripe: head.header("stripe-signature"),
                    slack: head.header("x-slack-signature"),
                    slack_timestamp: head.header("x-slack-request-timestamp"),
                };
                webhook_signature::verify(&secret, headers, &received, now_millis() / 1000)
            });
            if good {
                tokens.note_signature_used(id).map_err(store_error)?;
            }
            body = Some(received);
            good
        }
        None => false,
    };
    if !proven {
        return Err(unauthorized().await);
    }

    let Some(settings) = &ctx.settings else {
        return Err(Refusal::new("404 Not Found", "no_webhooks", "this hub has no settings file, so no webhooks"));
    };
    let config_path = settings.config_path();
    let config = load_config_from_path(&config_path, false).map_err(|e| Refusal::new("500 Internal Server Error", "server_error", format!("could not read the config: {e:#}")))?;
    let Some(hook) = config.webhooks.iter().find(|h| h.id == id).cloned() else {
        return Err(Refusal::new("404 Not Found", "unknown_webhook", format!("there is no webhook '{id}' (it may have been removed)")));
    };
    // The credential is of the other kind than the config now wants (its mode was changed and no new credential made):
    // it proves nothing, and the caller is told what any caller without proof is told.
    if Some(hook.auth) != kind {
        return Err(unauthorized().await);
    }
    if !hook.enabled {
        return Err(Refusal::new("403 Forbidden", "webhook_paused", format!("webhook '{id}' is paused")));
    }

    let body = match body {
        Some(body) => body,
        None => read_body(stream, head).await?,
    };
    let content_type = head.header("content-type").map(str::to_string);
    let conversation = conversation_id(id);
    // A signed call that a service sends again (GitHub does, when it thinks a delivery failed) isn't run twice. The id is
    // only believed because the signature was checked first.
    let delivery = if kind == Some(WebhookAuth::Hmac) { delivery_id(head) } else { None };
    if delivery.as_deref().is_some_and(|d| runner.seen_delivery(id, d)) {
        return Ok(json!({ "status": "duplicate", "webhook": id, "conversation": conversation }));
    }
    if !runner.start(ctx.orchestrator.current(), Arc::new(config), config_path, hook, content_type, body) {
        return Err(Refusal::new("409 Conflict", "still_running", format!("webhook '{id}' is still working on the last call — try again when it finishes")));
    }
    if let Some(delivery) = delivery {
        runner.remember_delivery(id, &delivery);
    }
    Ok(json!({ "status": "started", "webhook": id, "conversation": conversation }))
}

/// The `401` for every way of not proving oneself, after the wait that keeps guessing slow.
async fn unauthorized() -> Refusal {
    tokio::time::sleep(WRONG_KEY_DELAY).await;
    Refusal::new("401 Unauthorized", "invalid_token", "missing or invalid webhook credential — create one with `warden-server webhooks token <id>`")
}

/// The delivery id a service puts on a call, to tell a repeat from a new one: GitHub's, or the generic `Idempotency-Key`.
/// Nothing absurdly long is taken.
fn delivery_id(head: &RequestHead) -> Option<String> {
    head.header("x-github-delivery").or_else(|| head.header("idempotency-key")).map(str::trim).filter(|d| !d.is_empty() && d.len() <= 128).map(str::to_string)
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
