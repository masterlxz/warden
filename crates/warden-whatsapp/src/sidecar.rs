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
pub async fn run_bot(sidecar: &mut impl WhatsAppSidecar, orchestrator: &Orchestrator, conversations_dir: &Path) -> anyhow::Result<()> {
    loop {
        match sidecar.recv_event().await? {
            None => anyhow::bail!("WhatsApp sidecar process exited unexpectedly"),
            Some(event) => handle_event(sidecar, orchestrator, conversations_dir, event).await,
        }
    }
}

async fn handle_event(sidecar: &mut impl WhatsAppSidecar, orchestrator: &Orchestrator, conversations_dir: &Path, event: SidecarEvent) {
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
            let (reply, attachments) = match text {
                None => (MEDIA_REPLY.to_string(), Vec::new()),
                Some(text) => {
                    let title_seed = sender_name.unwrap_or_else(|| chat_id.clone());
                    // Spending limits (P4) are counted per chat.
                    let orchestrator = orchestrator.with_spend_context(SpendContext::new("whatsapp").with_user(chat_id.clone()));
                    match warden_bootstrap::handle_turn(&orchestrator, conversations_dir, &chat_id, &title_seed, &text).await {
                        Ok(outcome) => (outcome.content, outcome.attachments),
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
        handle_event(&mut sidecar, &orchestrator, &conversations_dir, event).await;

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
        handle_event(&mut sidecar, &orchestrator, &conversations_dir, event).await;

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

        handle_event(&mut sidecar, &orchestrator, &conversations_dir, SidecarEvent::Connected).await;
        handle_event(&mut sidecar, &orchestrator, &conversations_dir, SidecarEvent::Disconnected { logged_out: false }).await;
        handle_event(&mut sidecar, &orchestrator, &conversations_dir, SidecarEvent::Disconnected { logged_out: true }).await;

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
        handle_event(&mut sidecar, &orchestrator, &conversations_dir, event).await;

        let sent = sidecar.sent.lock().unwrap();
        assert_eq!(sent[0], ("5511999999999@s.whatsapp.net".to_string(), "here you go".to_string()));

        let sent_media = sidecar.sent_media.lock().unwrap();
        assert_eq!(
            sent_media[0],
            ("5511999999999@s.whatsapp.net".to_string(), Attachment { mime_type: "image/png".to_string(), data: "aGVsbG8=".to_string() })
        );
    }
}
