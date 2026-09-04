//! Telegram Bot API client and receive loop (Fase 2). Long polling only (`getUpdates`, no
//! webhook) — matches the project's "no server required" principle (ARCHITECTURE.md): a
//! personal always-on client process is enough, no public HTTPS endpoint needed. Replies are
//! sent as plain text (no `parse_mode`) — a real MarkdownV2 conversion is a bigger job (Telegram
//! rejects the whole message if a reserved character isn't escaped right) left for later.

use std::path::Path;
use std::time::Duration;

use anyhow::Context;
use async_trait::async_trait;
use serde::Deserialize;
use warden_core::orchestrator::Orchestrator;

const TELEGRAM_MESSAGE_LIMIT: usize = 4096;
const POLL_TIMEOUT_SECS: u64 = 30;
const RETRY_DELAY: Duration = Duration::from_secs(5);

const HELP_TEXT: &str =
    "Hi! I'm Warden, your personal AI agent. Just send me a message and I'll reply. /help shows this message again.";

#[derive(Debug, Clone, Deserialize)]
pub struct Update {
    pub update_id: i64,
    #[serde(default)]
    pub message: Option<IncomingMessage>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IncomingMessage {
    pub chat: Chat,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub from: Option<Sender>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Chat {
    pub id: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Sender {
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub first_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ApiResponse<T> {
    ok: bool,
    #[serde(default)]
    result: Option<T>,
    #[serde(default)]
    description: Option<String>,
}

/// Thin enough to mock in tests without a real HTTP call — same spirit as `ModelProvider`/
/// `ScriptedModel` in `warden-core/tests/pipeline.rs`. The repo has no HTTP-mocking crate
/// (`wiremock` etc.) today, so this trait is what makes `run_bot`'s logic testable without one.
#[async_trait]
pub trait TelegramApi: Send + Sync {
    async fn get_updates(&self, offset: Option<i64>, timeout_secs: u64) -> anyhow::Result<Vec<Update>>;
    async fn send_message(&self, chat_id: i64, text: &str) -> anyhow::Result<()>;
}

pub struct TelegramClient {
    token: String,
    client: reqwest::Client,
}

impl TelegramClient {
    pub fn new(token: impl Into<String>) -> Self {
        // Long polling can legitimately wait up to POLL_TIMEOUT_SECS for a response; the extra
        // margin here is just so a slow-but-still-answering connection isn't cut off right at
        // that boundary.
        let client = reqwest::Client::builder().timeout(Duration::from_secs(POLL_TIMEOUT_SECS + 20)).build().unwrap_or_default();
        Self { token: token.into(), client }
    }

    fn url(&self, method: &str) -> String {
        format!("https://api.telegram.org/bot{}/{method}", self.token)
    }
}

#[async_trait]
impl TelegramApi for TelegramClient {
    async fn get_updates(&self, offset: Option<i64>, timeout_secs: u64) -> anyhow::Result<Vec<Update>> {
        let mut query = vec![("timeout".to_string(), timeout_secs.to_string())];
        if let Some(offset) = offset {
            query.push(("offset".to_string(), offset.to_string()));
        }

        let response = self.client.get(self.url("getUpdates")).query(&query).send().await?;
        let parsed: ApiResponse<Vec<Update>> = response.json().await.context("failed to parse getUpdates response")?;
        if !parsed.ok {
            anyhow::bail!("Telegram getUpdates error: {}", parsed.description.unwrap_or_default());
        }
        Ok(parsed.result.unwrap_or_default())
    }

    async fn send_message(&self, chat_id: i64, text: &str) -> anyhow::Result<()> {
        for chunk in split_into_chunks(text) {
            let response = self
                .client
                .post(self.url("sendMessage"))
                .json(&serde_json::json!({ "chat_id": chat_id, "text": chunk }))
                .send()
                .await?;
            let parsed: ApiResponse<serde_json::Value> =
                response.json().await.context("failed to parse sendMessage response")?;
            if !parsed.ok {
                anyhow::bail!("Telegram sendMessage error: {}", parsed.description.unwrap_or_default());
            }
        }
        Ok(())
    }
}

/// Splits `text` into pieces no longer than `TELEGRAM_MESSAGE_LIMIT` bytes, backing off to the
/// nearest char boundary so a multi-byte UTF-8 sequence is never split across two messages —
/// same technique as `warden_core::tool::shell::truncate`, repeated until nothing's left over
/// instead of cutting once.
fn split_into_chunks(text: &str) -> Vec<&str> {
    if text.len() <= TELEGRAM_MESSAGE_LIMIT {
        return vec![text];
    }

    let mut chunks = Vec::new();
    let mut rest = text;
    while rest.len() > TELEGRAM_MESSAGE_LIMIT {
        let mut end = TELEGRAM_MESSAGE_LIMIT;
        while !rest.is_char_boundary(end) {
            end -= 1;
        }
        chunks.push(&rest[..end]);
        rest = &rest[end..];
    }
    chunks.push(rest);
    chunks
}

/// Runs forever: long-polls for updates and handles each one. A polling error (network blip,
/// Telegram briefly unreachable) is logged and retried after `RETRY_DELAY` rather than crashing
/// the process — same graceful-degradation spirit as the rest of the project.
pub async fn run_bot(api: &impl TelegramApi, orchestrator: &Orchestrator, conversations_dir: &Path) -> anyhow::Result<()> {
    let mut offset: Option<i64> = None;
    loop {
        match api.get_updates(offset, POLL_TIMEOUT_SECS).await {
            Ok(updates) => process_updates(api, orchestrator, conversations_dir, updates, &mut offset).await,
            Err(err) => {
                eprintln!("error polling Telegram, retrying in {}s: {err:#}", RETRY_DELAY.as_secs());
                tokio::time::sleep(RETRY_DELAY).await;
            }
        }
    }
}

/// Processes one batch of updates, advancing `offset` past each as it's handled so the next
/// `getUpdates` call never sees it again — split out from `run_bot`'s loop body so tests can
/// drive it directly with a canned batch instead of running the loop forever.
async fn process_updates(
    api: &impl TelegramApi,
    orchestrator: &Orchestrator,
    conversations_dir: &Path,
    updates: Vec<Update>,
    offset: &mut Option<i64>,
) {
    for update in updates {
        *offset = Some(update.update_id + 1);

        let Some(message) = update.message else { continue };
        let Some(text) = message.text.clone() else { continue };
        handle_update(api, orchestrator, conversations_dir, &message, &text).await;
    }
}

async fn handle_update(
    api: &impl TelegramApi,
    orchestrator: &Orchestrator,
    conversations_dir: &Path,
    message: &IncomingMessage,
    text: &str,
) {
    if text == "/start" || text == "/help" {
        if let Err(err) = api.send_message(message.chat.id, HELP_TEXT).await {
            eprintln!("failed to send help text to chat {}: {err:#}", message.chat.id);
        }
        return;
    }

    let conversation_id = message.chat.id.to_string();
    let title_seed = message
        .from
        .as_ref()
        .and_then(|sender| sender.username.clone().or_else(|| sender.first_name.clone()))
        .unwrap_or_else(|| conversation_id.clone());

    let reply = match warden_bootstrap::handle_turn(orchestrator, conversations_dir, &conversation_id, &title_seed, text).await {
        Ok(outcome) => outcome.content,
        Err(err) => {
            eprintln!("error handling message from chat {}: {err:#}", message.chat.id);
            "Sorry, something went wrong handling your message.".to_string()
        }
    };

    if let Err(err) = api.send_message(message.chat.id, &reply).await {
        eprintln!("failed to send reply to chat {}: {err:#}", message.chat.id);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    use async_trait::async_trait;
    use warden_core::model::{ChatStream, Message, ModelProvider, Response, response_stream};
    use warden_core::tool::ToolSpec;

    use super::*;

    struct EchoModel;

    #[async_trait]
    impl ModelProvider for EchoModel {
        async fn chat_stream(&self, messages: Vec<Message>, _tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
            let last_user = messages.iter().rev().find(|m| m.role == warden_core::model::Role::User).unwrap();
            Ok(response_stream(Response { content: format!("echo: {}", last_user.content), tool_calls: Vec::new(), usage: None }))
        }
    }

    fn temp_orchestrator() -> Orchestrator {
        let vault = std::sync::Arc::new(warden_core::memory::Vault::new(std::env::temp_dir().join(format!(
            "warden-telegram-test-vault-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ))));
        Orchestrator::new(std::sync::Arc::new(EchoModel), vault)
    }

    fn temp_conversations_dir() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "warden-telegram-test-conversations-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ))
    }

    struct ScriptedTelegramApi {
        batches: Mutex<Vec<Vec<Update>>>,
        sent: Mutex<Vec<(i64, String)>>,
        get_updates_calls: AtomicUsize,
    }

    impl ScriptedTelegramApi {
        fn new(batches: Vec<Vec<Update>>) -> Self {
            Self { batches: Mutex::new(batches), sent: Mutex::new(Vec::new()), get_updates_calls: AtomicUsize::new(0) }
        }
    }

    #[async_trait]
    impl TelegramApi for ScriptedTelegramApi {
        async fn get_updates(&self, _offset: Option<i64>, _timeout_secs: u64) -> anyhow::Result<Vec<Update>> {
            self.get_updates_calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.batches.lock().unwrap().pop().unwrap_or_default())
        }

        async fn send_message(&self, chat_id: i64, text: &str) -> anyhow::Result<()> {
            self.sent.lock().unwrap().push((chat_id, text.to_string()));
            Ok(())
        }
    }

    fn text_update(update_id: i64, chat_id: i64, text: &str) -> Update {
        Update {
            update_id,
            message: Some(IncomingMessage {
                chat: Chat { id: chat_id },
                text: Some(text.to_string()),
                from: Some(Sender { username: Some("fabio".to_string()), first_name: None }),
            }),
        }
    }

    #[tokio::test]
    async fn full_turn_replies_and_persists_the_conversation() {
        let api = ScriptedTelegramApi::new(vec![vec![text_update(1, 42, "hello")]]);
        let orchestrator = temp_orchestrator();
        let conversations_dir = temp_conversations_dir();
        let mut offset = None;

        let updates = api.get_updates(offset, 30).await.unwrap();
        process_updates(&api, &orchestrator, &conversations_dir, updates, &mut offset).await;

        assert_eq!(offset, Some(2));
        let sent = api.sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0], (42, "echo: hello".to_string()));

        let conversation = warden_bootstrap::load_conversation(&conversations_dir, "42").unwrap().unwrap();
        assert_eq!(conversation.messages.len(), 2);
        assert_eq!(conversation.messages[0].content, "hello");
        assert_eq!(conversation.messages[1].content, "echo: hello");
        assert_eq!(conversation.title, "fabio");
    }

    #[tokio::test]
    async fn start_and_help_reply_without_calling_the_orchestrator() {
        let api = ScriptedTelegramApi::new(vec![vec![text_update(1, 42, "/start")]]);
        let orchestrator = temp_orchestrator();
        let conversations_dir = temp_conversations_dir();
        let mut offset = None;

        let updates = api.get_updates(offset, 30).await.unwrap();
        process_updates(&api, &orchestrator, &conversations_dir, updates, &mut offset).await;

        let sent = api.sent.lock().unwrap();
        assert_eq!(sent[0], (42, HELP_TEXT.to_string()));
        assert!(warden_bootstrap::load_conversation(&conversations_dir, "42").unwrap().is_none());
    }

    #[tokio::test]
    async fn offset_advances_past_updates_with_no_message_or_text() {
        let update_without_message = Update { update_id: 5, message: None };
        let api = ScriptedTelegramApi::new(vec![vec![update_without_message]]);
        let orchestrator = temp_orchestrator();
        let conversations_dir = temp_conversations_dir();
        let mut offset = None;

        let updates = api.get_updates(offset, 30).await.unwrap();
        process_updates(&api, &orchestrator, &conversations_dir, updates, &mut offset).await;

        assert_eq!(offset, Some(6));
        assert!(api.sent.lock().unwrap().is_empty());
    }

    #[test]
    fn split_into_chunks_respects_the_telegram_message_limit() {
        let short = "hello";
        assert_eq!(split_into_chunks(short), vec![short]);

        let long_text = "x".repeat(TELEGRAM_MESSAGE_LIMIT + 10);
        let chunks = split_into_chunks(&long_text);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].len(), TELEGRAM_MESSAGE_LIMIT);
        assert_eq!(chunks[1].len(), 10);
        assert_eq!(format!("{}{}", chunks[0], chunks[1]), long_text);
    }

    #[test]
    fn split_into_chunks_backs_off_to_a_char_boundary() {
        // A 3-byte UTF-8 char ('€') straddling the split point must land whole on one side.
        let text = format!("{}€", "x".repeat(TELEGRAM_MESSAGE_LIMIT - 1));
        let chunks = split_into_chunks(&text);
        for chunk in &chunks {
            assert!(chunk.is_char_boundary(0) && chunk.is_char_boundary(chunk.len()));
        }
        assert_eq!(chunks.concat(), text);
    }
}
