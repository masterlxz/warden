//! IPC with the Node/Baileys sidecar (Fase 3): a real WhatsApp connection lives entirely in
//! `sidecar/whatsapp/index.mjs`, spoken to over stdin/stdout as one JSON object per line — the
//! first custom IPC in the project (everything else that spawns a Node process, like the MCP
//! client, uses a pre-existing protocol). QR pairing is rendered by the sidecar directly to its
//! own stderr, a separate stream from the stdin/stdout channel here, so it never corrupts a line
//! of protocol JSON — this process just inherits that stderr so the user sees it in their
//! terminal.

use std::path::Path;
use std::process::Stdio;

use anyhow::Context;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use warden_bootstrap::bot_pairing::{self, BotPairing, Issued};
use warden_bootstrap::FileConfig;
use warden_core::model::Attachment;
use warden_core::orchestrator::Orchestrator;
use warden_core::spend::SpendContext;

const MEDIA_REPLY: &str = "Sorry, I can only read text messages for now.";

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum SidecarEvent {
    Connected,
    Disconnected { logged_out: bool },
    Message { chat_id: String, sender_name: Option<String>, text: Option<String> },
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum SidecarCommand {
    Send { chat_id: String, text: String },
    /// Media extracted from an MCP tool result during a turn (P64 frente 2 fatia 2) — `data` is
    /// the same base64 payload `Attachment` already carries, decoded on the Node side (Baileys'
    /// `sendMessage` wants a `Buffer`, not base64 text).
    SendMedia { chat_id: String, mime_type: String, data: String },
}

/// Reading events is inherently stateful (lines pulled one at a time off a live stream), unlike
/// `TelegramApi::get_updates` (Fase 2) which is a fresh HTTP call each time — hence `&mut self`
/// here instead of `&self`.
#[async_trait]
pub trait WhatsAppSidecar: Send + Sync {
    /// `Ok(None)` means the sidecar process's stdout closed (it exited).
    async fn recv_event(&mut self) -> anyhow::Result<Option<SidecarEvent>>;
    async fn send(&mut self, chat_id: &str, text: &str) -> anyhow::Result<()>;
    /// Media extracted from an MCP tool result during a turn (P64 frente 2 fatia 2) — sent as its
    /// own WhatsApp message, native to whatever kind it is (image/video/audio/document).
    async fn send_attachment(&mut self, chat_id: &str, attachment: &Attachment) -> anyhow::Result<()>;
}

pub struct ChildSidecar {
    _child: Child,
    stdin: ChildStdin,
    lines: Lines<BufReader<ChildStdout>>,
}

impl ChildSidecar {
    /// Spawns `node <script_path>` with `WARDEN_WHATSAPP_AUTH_DIR` set for the sidecar's
    /// `useMultiFileAuthState`. stdin/stdout are piped for the JSON-lines protocol; stderr is
    /// inherited so the sidecar's QR code (and any Node-side error output) reaches the user's
    /// terminal directly.
    pub async fn spawn(script_path: &Path, auth_dir: &Path) -> anyhow::Result<Self> {
        let mut child = Command::new("node")
            .arg(script_path)
            .env("WARDEN_WHATSAPP_AUTH_DIR", auth_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|err| {
                if err.kind() == std::io::ErrorKind::NotFound {
                    anyhow::anyhow!(
                        "Node.js not found on PATH — required for the WhatsApp sidecar, install it from https://nodejs.org"
                    )
                } else {
                    anyhow::anyhow!("failed to spawn the WhatsApp sidecar: {err}")
                }
            })?;

        let stdin = child.stdin.take().context("sidecar child process has no stdin handle")?;
        let stdout = child.stdout.take().context("sidecar child process has no stdout handle")?;

        Ok(Self { _child: child, stdin, lines: BufReader::new(stdout).lines() })
    }
}

#[async_trait]
impl WhatsAppSidecar for ChildSidecar {
    async fn recv_event(&mut self) -> anyhow::Result<Option<SidecarEvent>> {
        match self.lines.next_line().await.context("failed to read from the WhatsApp sidecar's stdout")? {
            Some(line) => {
                let event = serde_json::from_str(&line)
                    .with_context(|| format!("failed to parse event from the WhatsApp sidecar: {line}"))?;
                Ok(Some(event))
            }
            None => Ok(None),
        }
    }

    async fn send(&mut self, chat_id: &str, text: &str) -> anyhow::Result<()> {
        let command = SidecarCommand::Send { chat_id: chat_id.to_string(), text: text.to_string() };
        let json = serde_json::to_string(&command).context("failed to serialize sidecar command")?;
        self.stdin.write_all(json.as_bytes()).await.context("failed to write to the WhatsApp sidecar's stdin")?;
        self.stdin.write_all(b"\n").await.context("failed to write to the WhatsApp sidecar's stdin")?;
        Ok(())
    }

    async fn send_attachment(&mut self, chat_id: &str, attachment: &Attachment) -> anyhow::Result<()> {
        let command = SidecarCommand::SendMedia {
            chat_id: chat_id.to_string(),
            mime_type: attachment.mime_type.clone(),
            data: attachment.data.clone(),
        };
        let json = serde_json::to_string(&command).context("failed to serialize sidecar command")?;
        self.stdin.write_all(json.as_bytes()).await.context("failed to write to the WhatsApp sidecar's stdin")?;
        self.stdin.write_all(b"\n").await.context("failed to write to the WhatsApp sidecar's stdin")?;
        Ok(())
    }
}

/// Runs forever: reads one event at a time from the sidecar and handles it. Unlike Telegram's
/// long-poll loop, there's no "retry after a delay" branch here — a sidecar event stream ending
/// (`recv_event` returning `None`) means the Node process itself exited, which isn't recoverable
/// from this loop (reconnecting to WhatsApp after a *dropped connection* is the sidecar's own
/// job, handled entirely inside `index.mjs`; a `disconnected` event for that case still comes
/// through as a normal event here, not a stream end).
///
/// Answers only the chats `[whatsapp] allowed_chats` lists in `config_path` (P117), read again at each
/// message so an edit counts from the next one; nobody when there is no file or list.
///
/// The same read gives `pairing` (P117) and `[learning]` (P104), so turning either on or off counts
/// from the next message too, without restarting the bot.
pub async fn run_bot(sidecar: &mut impl WhatsAppSidecar, orchestrator: &Orchestrator, conversations_dir: &Path, config_path: Option<&Path>) -> anyhow::Result<()> {
    let mut access = Access { pairing: config_path.map(BotPairing::beside), ..Access::default() };
    // The last config that read well: where learning comes from.
    let mut live: Option<FileConfig> = None;
    loop {
        match sidecar.recv_event().await? {
            None => anyhow::bail!("WhatsApp sidecar process exited unexpectedly"),
            Some(event) => {
                if let (Some(path), SidecarEvent::Message { .. }) = (config_path, &event) {
                    match warden_bootstrap::bot_access::read_config(path) {
                        Ok(config) => {
                            access.settings = config.whatsapp.clone();
                            live = Some(config);
                        }
                        Err(err) => eprintln!("can't read the config from {}, keeping the last one: {err:#}", path.display()),
                    }
                }
                let learning = live.as_ref().filter(|c| c.learning.enabled);
                handle_event(sidecar, orchestrator, conversations_dir, learning, &mut access, event).await
            }
        }
    }
}

/// Who may talk to the bot (P117), and who it has already said it ignored.
#[derive(Default)]
struct Access {
    settings: warden_bootstrap::bot_access::WhatsAppSettings,
    /// Ignored chats already logged, so a stranger who keeps writing costs one line, not one per message.
    reported: std::collections::HashSet<String>,
    /// Where a stranger's pairing request goes when `settings.pairing` is on.
    pairing: Option<BotPairing>,
}

/// What the gate decided about one message.
#[derive(Debug, PartialEq)]
enum Gate {
    Allowed,
    /// Ignored: no reply, no conversation kept.
    Silent,
    /// A stranger who just asked to pair: tell them this code.
    Pair(String),
}

/// What a stranger is told when they ask to pair.
fn pairing_reply(code: &str) -> String {
    format!(
        "I only talk to people my owner has approved. Give them this code: {} (valid for {} minutes). Once they approve it, write to me again.",
        bot_pairing::display_code(code),
        bot_pairing::VALID_FOR_SECS / 60
    )
}

impl Access {
    /// Whether chat `chat_id` may be answered. A refusal is silent to the sender (no reply, no
    /// conversation kept, the media notice included) and logged once per chat, with what to add to allow it.
    /// With `pairing` on, a stranger in a private chat gets a code once instead (then silence while it's
    /// valid); `label` is the name they go by, for the owner to recognise them.
    fn check(&mut self, chat_id: &str, label: Option<&str>) -> Gate {
        if self.settings.allows(chat_id) {
            return Gate::Allowed;
        }
        let private = warden_bootstrap::bot_access::is_private_chat(chat_id);
        if self.reported.insert(chat_id.to_string()) {
            if private {
                eprintln!("ignored whatsapp chat {chat_id}: to allow it, add \"{chat_id}\" (or just its number) to [whatsapp] allowed_chats in config.toml");
            } else {
                eprintln!("ignored whatsapp chat {chat_id}: the bot only answers private chats, never groups");
            }
        }
        let (true, true, Some(store)) = (private, self.settings.pairing, self.pairing.as_ref()) else { return Gate::Silent };
        match store.request(bot_pairing::WHATSAPP, chat_id, label.unwrap_or_default(), warden_bootstrap::bot_access::unix_now()) {
            Ok(Issued::Fresh(code)) => {
                eprintln!("whatsapp chat {chat_id} asked to pair: approve with `warden bots pair approve {}`", bot_pairing::display_code(&code));
                Gate::Pair(code)
            }
            Ok(Issued::Existing) => Gate::Silent,
            Ok(Issued::Full) => {
                eprintln!("whatsapp chat {chat_id} asked to pair, but {} requests are already waiting: not answering", bot_pairing::MAX_PENDING_PER_CHANNEL);
                Gate::Silent
            }
            Err(err) => {
                eprintln!("can't record the pairing request of whatsapp chat {chat_id}: {err:#}");
                Gate::Silent
            }
        }
    }
}

async fn handle_event(sidecar: &mut impl WhatsAppSidecar, orchestrator: &Orchestrator, conversations_dir: &Path, learning: Option<&FileConfig>, access: &mut Access, event: SidecarEvent) {
    match event {
        SidecarEvent::Connected => println!("WhatsApp connected."),
        SidecarEvent::Disconnected { logged_out } => {
            if logged_out {
                eprintln!("WhatsApp session logged out — delete the auth directory and restart to re-pair.");
            } else {
                eprintln!("WhatsApp disconnected, the sidecar will attempt to reconnect.");
            }
        }
        SidecarEvent::Message { chat_id, sender_name, text } => {
            // Before anything else, the "I only read text" notice included: a stranger gets no answer at all.
            match access.check(&chat_id, sender_name.as_deref()) {
                Gate::Allowed => {}
                Gate::Silent => return,
                Gate::Pair(code) => {
                    if let Err(err) = sidecar.send(&chat_id, &pairing_reply(&code)).await {
                        eprintln!("failed to send the pairing code to {chat_id}: {err:#}");
                    }
                    return;
                }
            }
            let mut learn_from = None;
            let (reply, attachments) = match text {
                None => (MEDIA_REPLY.to_string(), Vec::new()),
                Some(text) => {
                    let title_seed = sender_name.unwrap_or_else(|| chat_id.clone());
                    // Spending limits (P4) are counted per chat.
                    let orchestrator = orchestrator.with_spend_context(SpendContext::new("whatsapp").with_user(chat_id.clone()));
                    match warden_bootstrap::handle_turn(&orchestrator, conversations_dir, &chat_id, &title_seed, &text, Vec::new()).await {
                        Ok(outcome) => {
                            learn_from = Some(orchestrator);
                            (outcome.content, outcome.attachments)
                        }
                        Err(err) => {
                            eprintln!("error handling message from {chat_id}: {err:#}");
                            (warden_bootstrap::spend::chat_error_reply(&err), Vec::new())
                        }
                    }
                }
            };

            // A turn that only called a tool (no final prose) can legitimately have nothing to
            // say here — skip the send rather than deliver an empty message.
            if !reply.trim().is_empty() {
                if let Err(err) = sidecar.send(&chat_id, &reply).await {
                    eprintln!("failed to send reply to {chat_id}: {err:#}");
                }
            }

            for attachment in &attachments {
                if let Err(err) = sidecar.send_attachment(&chat_id, attachment).await {
                    eprintln!("failed to send attachment to {chat_id}: {err:#}");
                }
            }

            // P104: once the person has their answer, the assistant may look for something to learn from it.
            // Only from the chats the owner listed in `[learning] bot_chats`.
            if let (Some(orchestrator), Some(config)) = (learn_from, learning.filter(|c| c.learning.bot_chat_allowed("whatsapp", &chat_id))) {
                warden_bootstrap::learning::learn_with_config("warden-whatsapp", &orchestrator, config, conversations_dir, &chat_id, None, None).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    use async_trait::async_trait;
    use serde_json::json;
    use warden_core::model::{ChatStream, Message, ModelProvider, Response, ToolCall, response_stream};
    use warden_core::tool::{Tool, ToolSpec};

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
            "warden-whatsapp-test-vault-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ))));
        Orchestrator::new(std::sync::Arc::new(EchoModel), vault)
    }

    fn temp_conversations_dir() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "warden-whatsapp-test-conversations-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ))
    }

    struct ScriptedSidecar {
        events: Mutex<VecDeque<SidecarEvent>>,
        sent: Mutex<Vec<(String, String)>>,
        sent_media: Mutex<Vec<(String, Attachment)>>,
    }

    impl ScriptedSidecar {
        fn new(events: Vec<SidecarEvent>) -> Self {
            Self { events: Mutex::new(events.into()), sent: Mutex::new(Vec::new()), sent_media: Mutex::new(Vec::new()) }
        }
    }

    #[async_trait]
    impl WhatsAppSidecar for ScriptedSidecar {
        async fn recv_event(&mut self) -> anyhow::Result<Option<SidecarEvent>> {
            Ok(self.events.lock().unwrap().pop_front())
        }

        async fn send(&mut self, chat_id: &str, text: &str) -> anyhow::Result<()> {
            self.sent.lock().unwrap().push((chat_id.to_string(), text.to_string()));
            Ok(())
        }

        async fn send_attachment(&mut self, chat_id: &str, attachment: &Attachment) -> anyhow::Result<()> {
            self.sent_media.lock().unwrap().push((chat_id.to_string(), attachment.clone()));
            Ok(())
        }
    }

    /// The chats `[whatsapp] allowed_chats` lists.
    fn access_for(chats: &[&str]) -> Access {
        Access { settings: warden_bootstrap::bot_access::WhatsAppSettings { allowed_chats: chats.iter().map(|c| c.to_string()).collect(), ..Default::default() }, ..Access::default() }
    }

    /// P117: a stranger gets nothing (no reply, no media notice, no conversation, no model call), a group
    /// is never answered even if its id is listed, and a chat is logged once, not once per message.
    #[tokio::test]
    async fn only_a_listed_private_chat_is_answered() {
        struct Counting(std::sync::Arc<AtomicUsize>);
        #[async_trait]
        impl ModelProvider for Counting {
            async fn chat_stream(&self, _messages: Vec<Message>, _tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Ok(response_stream(Response { content: "hi".to_string(), tool_calls: Vec::new(), usage: None }))
            }
        }
        let calls = std::sync::Arc::new(AtomicUsize::new(0));
        let vault = std::sync::Arc::new(warden_core::memory::Vault::new(std::env::temp_dir().join(format!("warden-whatsapp-access-vault-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()))));
        let orchestrator = Orchestrator::new(std::sync::Arc::new(Counting(calls.clone())), vault);
        let conversations_dir = temp_conversations_dir();
        let message = |chat: &str, text: Option<&str>| SidecarEvent::Message { chat_id: chat.to_string(), sender_name: None, text: text.map(String::from) };
        let mut sidecar = ScriptedSidecar::new(Vec::new());
        let mut access = access_for(&["5511999999999", "120363000000000000@g.us"]);

        for event in [
            message("5511000000000@s.whatsapp.net", Some("hello")),
            message("5511000000000@s.whatsapp.net", Some("hello again")),
            message("5511000000000@s.whatsapp.net", None),
            message("120363000000000000@g.us", Some("hello group")),
            message("status@broadcast", Some("a status")),
        ] {
            handle_event(&mut sidecar, &orchestrator, &conversations_dir, None, &mut access, event).await;
        }
        assert!(sidecar.sent.lock().unwrap().is_empty(), "no reply to a stranger, a group or a status, not even the media notice: {:?}", sidecar.sent.lock().unwrap());
        assert_eq!(calls.load(Ordering::SeqCst), 0, "no model call");
        assert!(warden_bootstrap::load_conversation(&conversations_dir, "5511000000000@s.whatsapp.net").unwrap().is_none(), "no conversation kept");
        assert_eq!(access.reported.len(), 3, "each ignored chat once, not once per message");

        handle_event(&mut sidecar, &orchestrator, &conversations_dir, None, &mut access, message("5511999999999@s.whatsapp.net", Some("hello"))).await;
        assert_eq!(sidecar.sent.lock().unwrap().len(), 1, "the listed number is answered");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    /// P117: with `pairing` on, a stranger in a private chat gets a code once and then silence, a group gets
    /// nothing, and once the owner approves the code the next message is answered.
    #[tokio::test]
    async fn a_stranger_gets_a_pairing_code_once_and_is_answered_after_the_owner_approves() {
        let dir = std::env::temp_dir().join(format!("warden-whatsapp-pairing-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&dir).unwrap();
        let config_path = dir.join("config.toml");
        std::fs::write(&config_path, "[whatsapp]\npairing = true\n").unwrap();

        let orchestrator = temp_orchestrator();
        let conversations_dir = temp_conversations_dir();
        let stranger = "5511000000000@s.whatsapp.net";
        let message = |chat: &str, text: &str| SidecarEvent::Message { chat_id: chat.to_string(), sender_name: Some("Ana".to_string()), text: Some(text.to_string()) };
        let mut sidecar = ScriptedSidecar::new(Vec::new());
        let mut access = Access { pairing: Some(BotPairing::beside(&config_path)), ..access_for(&[]) };
        access.settings.pairing = true;

        for event in [message(stranger, "hello"), message(stranger, "hello again"), message("120363000000000000@g.us", "hello group")] {
            handle_event(&mut sidecar, &orchestrator, &conversations_dir, None, &mut access, event).await;
        }
        let pending = BotPairing::beside(&config_path).list(warden_bootstrap::bot_access::unix_now()).unwrap();
        assert_eq!(pending.len(), 1, "one request, from the private chat only");
        assert_eq!((pending[0].channel.as_str(), pending[0].sender.as_str(), pending[0].label.as_str()), ("whatsapp", stranger, "Ana"));
        {
            let sent = sidecar.sent.lock().unwrap();
            assert_eq!(sent.len(), 1, "the code once, then silence: {sent:?}");
            assert_eq!(sent[0].0, stranger);
            assert!(sent[0].1.contains(&bot_pairing::display_code(&pending[0].code)), "{}", sent[0].1);
        }
        assert!(warden_bootstrap::load_conversation(&conversations_dir, stranger).unwrap().is_none(), "no conversation kept");

        // The owner approves; the bot reads the config again and the same chat is answered.
        BotPairing::beside(&config_path).approve(&pending[0].code, warden_bootstrap::bot_access::unix_now(), &config_path).unwrap();
        access.settings = warden_bootstrap::bot_access::read_config(&config_path).unwrap().whatsapp;
        handle_event(&mut sidecar, &orchestrator, &conversations_dir, None, &mut access, message(stranger, "hello")).await;
        let sent = sidecar.sent.lock().unwrap();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[1], (stranger.to_string(), "echo: hello".to_string()));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn text_message_calls_the_orchestrator_and_persists_the_conversation() {
        let mut sidecar = ScriptedSidecar::new(vec![SidecarEvent::Message {
            chat_id: "5511999999999@s.whatsapp.net".to_string(),
            sender_name: Some("Fabio".to_string()),
            text: Some("hello".to_string()),
        }]);
        let orchestrator = temp_orchestrator();
        let conversations_dir = temp_conversations_dir();

        let event = sidecar.recv_event().await.unwrap().unwrap();
        handle_event(&mut sidecar, &orchestrator, &conversations_dir, None, &mut access_for(&["5511999999999"]), event).await;

        let sent = sidecar.sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0], ("5511999999999@s.whatsapp.net".to_string(), "echo: hello".to_string()));

        let conversation = warden_bootstrap::load_conversation(&conversations_dir, "5511999999999@s.whatsapp.net")
            .unwrap()
            .unwrap();
        assert_eq!(conversation.messages.len(), 2);
        assert_eq!(conversation.messages[0].content, "hello");
        assert_eq!(conversation.messages[1].content, "echo: hello");
        assert_eq!(conversation.title, "Fabio");
    }

    /// P115: the assistant learns from a bot chat only when the owner listed it in `[learning] bot_chats`.
    #[tokio::test]
    async fn learning_runs_only_in_the_chats_the_owner_listed() {
        struct VerdictModel(std::sync::Arc<AtomicUsize>);
        #[async_trait]
        impl ModelProvider for VerdictModel {
            async fn chat_stream(&self, _messages: Vec<Message>, _tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Ok(response_stream(Response { content: r#"{"signal":"none"}"#.to_string(), tool_calls: Vec::new(), usage: None }))
            }
        }
        const CHAT: &str = "5511999999999@s.whatsapp.net";
        let calls_for = |bot_chats: Vec<String>| async move {
            let mut config = FileConfig::default();
            config.learning.enabled = true;
            config.learning.bot_chats = bot_chats;
            let calls = std::sync::Arc::new(AtomicUsize::new(0));
            let vault = std::sync::Arc::new(warden_core::memory::Vault::new(std::env::temp_dir().join(format!("warden-whatsapp-learn-vault-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()))));
            let orchestrator = Orchestrator::new(std::sync::Arc::new(VerdictModel(calls.clone())), vault);
            let mut sidecar = ScriptedSidecar::new(vec![SidecarEvent::Message { chat_id: CHAT.to_string(), sender_name: None, text: Some("no, use tabs, not spaces".to_string()) }]);
            let event = sidecar.recv_event().await.unwrap().unwrap();
            handle_event(&mut sidecar, &orchestrator, &temp_conversations_dir(), Some(&config), &mut access_for(&["5511999999999"]), event).await;
            calls.load(Ordering::SeqCst)
        };

        assert_eq!(calls_for(Vec::new()).await, 1, "just the turn itself: no list, no learning");
        assert_eq!(calls_for(vec![format!("telegram:{CHAT}"), "whatsapp:other@s.whatsapp.net".into()]).await, 1, "another chat, and another channel's entry, don't count");
        assert!(calls_for(vec![format!("whatsapp:{CHAT}")]).await > 1, "the listed chat gets the detector's call");
    }

    #[tokio::test]
    async fn media_message_replies_without_calling_the_orchestrator() {
        struct CountingModel {
            calls: AtomicUsize,
        }
        #[async_trait]
        impl ModelProvider for CountingModel {
            async fn chat_stream(&self, _messages: Vec<Message>, _tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                Ok(response_stream(Response { content: "should not be called".to_string(), tool_calls: Vec::new(), usage: None }))
            }
        }

        let vault = std::sync::Arc::new(warden_core::memory::Vault::new(std::env::temp_dir().join(format!(
            "warden-whatsapp-test-media-vault-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ))));
        let model = std::sync::Arc::new(CountingModel { calls: AtomicUsize::new(0) });
        let orchestrator = Orchestrator::new(model.clone(), vault);
        let conversations_dir = temp_conversations_dir();

        let mut sidecar = ScriptedSidecar::new(vec![SidecarEvent::Message {
            chat_id: "5511999999999@s.whatsapp.net".to_string(),
            sender_name: None,
            text: None,
        }]);

        let event = sidecar.recv_event().await.unwrap().unwrap();
        handle_event(&mut sidecar, &orchestrator, &conversations_dir, None, &mut access_for(&["5511999999999"]), event).await;

        assert_eq!(model.calls.load(Ordering::SeqCst), 0);
        let sent = sidecar.sent.lock().unwrap();
        assert_eq!(sent[0], ("5511999999999@s.whatsapp.net".to_string(), MEDIA_REPLY.to_string()));
        assert!(warden_bootstrap::load_conversation(&conversations_dir, "5511999999999@s.whatsapp.net").unwrap().is_none());
    }

    #[tokio::test]
    async fn connected_and_disconnected_events_do_not_panic_or_send_anything() {
        let orchestrator = temp_orchestrator();
        let conversations_dir = temp_conversations_dir();
        let mut sidecar = ScriptedSidecar::new(Vec::new());

        handle_event(&mut sidecar, &orchestrator, &conversations_dir, None, &mut Access::default(), SidecarEvent::Connected).await;
        handle_event(&mut sidecar, &orchestrator, &conversations_dir, None, &mut Access::default(), SidecarEvent::Disconnected { logged_out: false }).await;
        handle_event(&mut sidecar, &orchestrator, &conversations_dir, None, &mut Access::default(), SidecarEvent::Disconnected { logged_out: true }).await;

        assert!(sidecar.sent.lock().unwrap().is_empty());
    }

    // --- P64 frente 2 fatia 2: attachments extracted from a tool call are sent to the chat ---

    struct ImageTool;

    #[async_trait]
    impl Tool for ImageTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec { name: "gen_image".to_string(), description: "returns an MCP-shaped image block".to_string(), parameters: json!({}) }
        }

        async fn call(&self, _args: serde_json::Value) -> anyhow::Result<serde_json::Value> {
            Ok(json!({ "content": [{ "type": "image", "data": "aGVsbG8=", "mimeType": "image/png" }] }))
        }
    }

    struct CallsToolThenAnswersModel {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl ModelProvider for CallsToolThenAnswersModel {
        async fn chat_stream(&self, _messages: Vec<Message>, _tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            if call == 0 {
                Ok(response_stream(Response {
                    content: String::new(),
                    tool_calls: vec![ToolCall { id: "call_1".to_string(), name: "gen_image".to_string(), arguments: json!({}), thought_signature: None }],
                    usage: None,
                }))
            } else {
                Ok(response_stream(Response { content: "here you go".to_string(), tool_calls: Vec::new(), usage: None }))
            }
        }
    }

    #[tokio::test]
    async fn an_attachment_extracted_from_a_tool_call_is_sent_after_the_text_reply() {
        let vault = std::sync::Arc::new(warden_core::memory::Vault::new(std::env::temp_dir().join(format!(
            "warden-whatsapp-test-attachment-vault-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ))));
        let mut orchestrator = Orchestrator::new(std::sync::Arc::new(CallsToolThenAnswersModel { calls: AtomicUsize::new(0) }), vault);
        orchestrator.register_tool(std::sync::Arc::new(ImageTool));
        let conversations_dir = temp_conversations_dir();

        let mut sidecar = ScriptedSidecar::new(vec![SidecarEvent::Message {
            chat_id: "5511999999999@s.whatsapp.net".to_string(),
            sender_name: Some("Fabio".to_string()),
            text: Some("make an image".to_string()),
        }]);

        let event = sidecar.recv_event().await.unwrap().unwrap();
        handle_event(&mut sidecar, &orchestrator, &conversations_dir, None, &mut access_for(&["5511999999999"]), event).await;

        let sent = sidecar.sent.lock().unwrap();
        assert_eq!(sent[0], ("5511999999999@s.whatsapp.net".to_string(), "here you go".to_string()));

        let sent_media = sidecar.sent_media.lock().unwrap();
        assert_eq!(
            sent_media[0],
            ("5511999999999@s.whatsapp.net".to_string(), Attachment { mime_type: "image/png".to_string(), data: "aGVsbG8=".to_string() })
        );
    }
}
