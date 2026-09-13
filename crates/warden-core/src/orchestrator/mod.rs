use std::sync::Arc;

use serde_json::Value;

use crate::memory::Vault;
use crate::model::{Attachment, Message, ModelProvider, StreamEvent, ToolCall, Usage};
use crate::tool::{Tool, ToolProvider};

/// Caps how many rounds of tool calls a single `handle_message` will chase before
/// giving up, so a model stuck requesting tools can't loop forever.
const MAX_TOOL_ITERATIONS: usize = 8;

/// Central coordinator: owns the model, the vault, and the registered tools.
/// Channels (CLI, Telegram, WhatsApp, ...) call `handle_message` and don't
/// know anything about which model or tools are behind it.
/// What `handle_message` hands back to the caller: the final answer, plus the summed token
/// usage across every `chat` call made along the way (a single message can trigger several,
/// one per round of tool calls). `None` only when the provider never reported usage at all —
/// not the case for OpenAI/Gemini today, but kept optional since `ModelProvider` doesn't
/// guarantee it.
#[derive(Debug, Clone)]
pub struct MessageOutcome {
    pub content: String,
    pub usage: Option<Usage>,
    /// Media (image/audio/video) extracted from an MCP tool's `CallToolResult` content blocks
    /// during this turn (P64 frente 2) — never round-tripped back into the model's own context
    /// (that would re-inflate every subsequent turn with base64), only surfaced here for the
    /// caller to persist/render. Empty when no tool call this turn produced any.
    pub attachments: Vec<Attachment>,
}

#[derive(Clone)]
pub struct Orchestrator {
    model: Arc<dyn ModelProvider>,
    vault: Arc<Vault>,
    tools: Vec<Arc<dyn Tool>>,
}

impl Orchestrator {
    pub fn new(model: Arc<dyn ModelProvider>, vault: Arc<Vault>) -> Self {
        Self { model, vault, tools: Vec::new() }
    }

    pub fn register_tool(&mut self, tool: Arc<dyn Tool>) {
        self.tools.push(tool);
    }

    /// Registers every tool a `ToolProvider` currently exposes (e.g. an MCP server's
    /// `tools/list`). Snapshot at call time — a provider whose tool set changes later needs to
    /// be re-registered to pick up the change, there's no live sync.
    pub async fn register_provider(&mut self, provider: &dyn ToolProvider) -> anyhow::Result<()> {
        for tool in provider.tools().await? {
            self.register_tool(tool);
        }
        Ok(())
    }

    pub fn vault(&self) -> &Arc<Vault> {
        &self.vault
    }

    /// Returns a copy of this orchestrator using a different model — cheap, since `model` is an
    /// `Arc` and the rest of `Self` is `Clone` over `Arc`s/a `Vec<Arc<_>>`. Lets a caller (the
    /// desktop's per-conversation model selector) swap the model for one call without re-running
    /// `bootstrap()` (which would reconnect MCP servers, redo OAuth, etc.).
    pub fn with_model(&self, model: Arc<dyn ModelProvider>) -> Self {
        Self { model, ..self.clone() }
    }

    /// Returns a copy of this orchestrator with one extra tool registered — cheap, same reasoning
    /// as `with_model`. Lets a caller attach a tool to one specific turn/agent (P46's opt-in
    /// `delegate_to_agent`) without touching the shared instance every other conversation uses.
    pub fn with_tool(&self, tool: Arc<dyn Tool>) -> Self {
        let mut clone = self.clone();
        clone.register_tool(tool);
        clone
    }

    /// Every tool currently registered — used by `warden-mcp-server` to re-expose this
    /// orchestrator's whole capability set (vault access, shell if enabled, whatever MCP servers
    /// were connected in `bootstrap()`, ...) as its own MCP server for third-party clients.
    pub fn tools(&self) -> &[Arc<dyn Tool>] {
        &self.tools
    }

    /// `history` is the prior turns of this conversation (user/assistant pairs, oldest first),
    /// as tracked by the caller — the orchestrator itself is stateless across calls. Pass `&[]`
    /// for a fresh conversation or a one-off sub-agent task.
    pub async fn handle_message(&self, history: &[Message], user_input: &str) -> anyhow::Result<MessageOutcome> {
        self.handle_turn(history, user_input, Vec::new(), None).await
    }

    /// Same as `handle_message`, but the current turn can carry image attachments (P28) —
    /// only meaningful to providers/models that support multimodal input.
    pub async fn handle_message_with_attachments(
        &self,
        history: &[Message],
        user_input: &str,
        attachments: Vec<Attachment>,
    ) -> anyhow::Result<MessageOutcome> {
        self.handle_turn(history, user_input, attachments, None).await
    }

    /// Same as `handle_message`, but forwards every `StreamEvent` to `on_event` as it arrives —
    /// the entry point a live UI (the CLI's ratatui-based interactive loop) uses to render
    /// content as it streams in, instead of waiting for the whole turn to finish. Every other
    /// caller (Telegram, WhatsApp, the desktop app, `DelegateTool`) keeps using `handle_message`,
    /// which is just this with a no-op sink.
    pub async fn handle_message_streaming(
        &self,
        history: &[Message],
        user_input: &str,
        on_event: impl FnMut(&StreamEvent) + Send,
    ) -> anyhow::Result<MessageOutcome> {
        self.handle_turn_streaming(history, user_input, Vec::new(), None, on_event).await
    }

    /// The general form both `handle_message` and `handle_message_with_attachments` wrap —
    /// `system_prompt` is an agent's persona (a per-conversation concept, only the desktop's
    /// `send_message` passes one; every other caller keeps getting `None`, unchanged behavior).
    /// Ignored when empty/whitespace-only, so an agent with a blank persona field behaves like no
    /// agent at all rather than sending a hollow system message.
    pub async fn handle_turn(
        &self,
        history: &[Message],
        user_input: &str,
        attachments: Vec<Attachment>,
        system_prompt: Option<&str>,
    ) -> anyhow::Result<MessageOutcome> {
        self.handle_turn_streaming(history, user_input, attachments, system_prompt, |_| {}).await
    }

    /// The general, streaming-capable form every other `handle_*` method wraps. Each round of the
    /// tool-call loop drives the model's `chat_stream` directly instead of the buffered `chat`,
    /// forwarding every `StreamEvent` to `on_event` live — a non-streaming caller just passes a
    /// no-op sink, so this is the one real implementation of the loop rather than two that could
    /// drift apart. One new failure mode versus the old buffered-JSON world: a connection can now
    /// drop *mid-stream*, after some content already arrived — that partial content is discarded
    /// and the call still surfaces as a plain `Err`, exactly as a connection failure would have
    /// looked before.
    pub async fn handle_turn_streaming(
        &self,
        history: &[Message],
        user_input: &str,
        attachments: Vec<Attachment>,
        system_prompt: Option<&str>,
        mut on_event: impl FnMut(&StreamEvent) + Send,
    ) -> anyhow::Result<MessageOutcome> {
        let mut messages = Vec::new();

        if let Some(persona) = system_prompt {
            if !persona.trim().is_empty() {
                messages.push(Message::system(persona.to_string()));
            }
        }

        // Fixed/standard memory (P52) — unlike the search block below, not keyed off relevance to
        // this turn's input: always injected in full when non-empty, so the model has standing
        // context (user profile, behavior rules, accumulated feedback) on every single turn, not
        // just when a grep/embedding match happens to surface it.
        let standing_memory = self.vault.standing_memory();
        if !standing_memory.is_empty() {
            messages.push(Message::system(format!(
                "Standing memory from the user's vault (always included, not relevance-dependent):\n\n{standing_memory}"
            )));
        }

        // Semantic search runs ONNX inference (blocking, CPU-heavy) — `spawn_blocking` keeps it
        // off the async runtime thread. Falls back to the plain grep on any error (model download
        // failed offline, corrupt index, panic) so vault context injection never breaks outright.
        // Behind the `semantic-search` feature (default on) — see `warden-core/Cargo.toml`; every
        // real `Orchestrator` host (CLI/desktop/server/Telegram/WhatsApp) keeps it enabled, this
        // only matters for `warden-core` compiled with default features off (e.g. as a dependency
        // of `warden-sync`, which never constructs an `Orchestrator` at all).
        #[cfg(feature = "semantic-search")]
        let hits = {
            let vault_for_search = self.vault.clone();
            let query = user_input.to_string();
            tokio::task::spawn_blocking(move || vault_for_search.search_semantic(&query, 8))
                .await
                .ok()
                .and_then(|result| result.ok())
                .unwrap_or_else(|| self.vault.search(user_input, 8).unwrap_or_default())
        };
        #[cfg(not(feature = "semantic-search"))]
        let hits = self.vault.search(user_input, 8).unwrap_or_default();
        if !hits.is_empty() {
            let context = hits
                .iter()
                .map(|h| format!("[{}:{}] {}", h.path, h.line_number, h.line.trim()))
                .collect::<Vec<_>>()
                .join("\n");
            messages.push(Message::system(format!(
                "Relevant context found in the user's memory vault (may or may not be relevant — use your judgment):\n{context}"
            )));
        }

        messages.extend(history.iter().cloned());
        messages.push(Message::user_with_attachments(user_input, attachments));

        let tool_specs = self.tools.iter().map(|t| t.spec()).collect::<Vec<_>>();

        let mut usage = Usage::default();
        let mut has_usage = false;
        let mut attachments: Vec<Attachment> = Vec::new();

        for _ in 0..MAX_TOOL_ITERATIONS {
            let stream = self.model.chat_stream(messages.clone(), tool_specs.clone()).await?;
            let response = crate::model::drain_chat_stream(stream, &mut on_event).await?;

            if let Some(u) = response.usage {
                usage.prompt_tokens += u.prompt_tokens;
                usage.completion_tokens += u.completion_tokens;
                usage.total_tokens += u.total_tokens;
                has_usage = true;
            }

            if response.tool_calls.is_empty() {
                return Ok(MessageOutcome { content: response.content, usage: has_usage.then_some(usage), attachments });
            }

            messages.push(Message::assistant_tool_calls(response.tool_calls.clone()));

            for tool_call in &response.tool_calls {
                let content = match self.run_tool(tool_call).await {
                    Ok(value) => {
                        let (content, extracted) = extract_media_from_tool_result(&value);
                        attachments.extend(extracted);
                        content
                    }
                    Err(err) => format!("error: {err:#}"),
                };
                messages.push(Message::tool_result(tool_call, content));
            }
        }

        anyhow::bail!("exceeded max tool-call iterations ({MAX_TOOL_ITERATIONS}) without a final answer")
    }

    async fn run_tool(&self, tool_call: &ToolCall) -> anyhow::Result<serde_json::Value> {
        let tool = self
            .tools
            .iter()
            .find(|t| t.spec().name == tool_call.name)
            .ok_or_else(|| anyhow::anyhow!("model requested unknown tool '{}'", tool_call.name))?;
        tool.call(tool_call.arguments.clone()).await
    }
}

/// Above this, an image/audio/video content block is left as raw text instead of becoming an
/// `Attachment` (P64 frente 2) — big enough for a still image or a short audio clip, not enough
/// for real video (no attempt to raise this for video; that needs a file-reference path instead
/// of inline base64, a future slice). Checked against the base64 string's decoded byte length.
const MAX_INLINE_MEDIA_BYTES: usize = 8 * 1024 * 1024;

fn within_media_size_cap(base64_data: &str) -> bool {
    base64_data.len() * 3 / 4 <= MAX_INLINE_MEDIA_BYTES
}

/// Turns a tool's raw JSON result into (text fed back to the model, media extracted as
/// attachments). Only tool results shaped like an MCP `CallToolResult` (rmcp) — a `content` array
/// where every item has a recognized `type` (`text`/`image`/`audio`/`resource`/`resource_link`) —
/// are parsed structurally; anything else (`generate_document`, `shell`, `read_file`, ...) falls
/// through to the exact same `value.to_string()` this replaced, unchanged.
///
/// `image`/`audio` blocks and `resource` blocks whose `mimeType` is image/audio/video become an
/// `Attachment` when their base64 payload fits `MAX_INLINE_MEDIA_BYTES` — the model only ever sees
/// a short placeholder for these (never the base64 itself, which would otherwise re-inflate every
/// subsequent turn's context). A `resource_link` (a URI, no inline bytes) is deliberately never
/// auto-fetched — fetching an arbitrary URL an MCP server hands back would be an outbound network
/// call driven by untrusted tool output (SSRF-shaped risk) — so it always falls through as text.
fn extract_media_from_tool_result(value: &Value) -> (String, Vec<Attachment>) {
    let known_type = |item: &Value| matches!(item.get("type").and_then(Value::as_str), Some("text" | "image" | "audio" | "resource" | "resource_link"));
    let Some(content) = value.get("content").and_then(Value::as_array).filter(|blocks| blocks.iter().all(known_type)) else {
        return (value.to_string(), Vec::new());
    };

    let mut text_parts = Vec::new();
    let mut attachments = Vec::new();

    for block in content {
        let block_type = block.get("type").and_then(Value::as_str).unwrap_or("");
        match block_type {
            "text" => {
                if let Some(text) = block.get("text").and_then(Value::as_str) {
                    text_parts.push(text.to_string());
                }
            }
            "image" | "audio" => {
                let data = block.get("data").and_then(Value::as_str);
                let mime_type = block.get("mimeType").and_then(Value::as_str);
                match (data, mime_type) {
                    (Some(data), Some(mime_type)) if within_media_size_cap(data) => {
                        attachments.push(Attachment { mime_type: mime_type.to_string(), data: data.to_string() });
                        text_parts.push(format!("[{block_type} attached: {mime_type}]"));
                    }
                    _ => text_parts.push(block.to_string()),
                }
            }
            "resource" => {
                let resource = block.get("resource");
                let blob = resource.and_then(|r| r.get("blob")).and_then(Value::as_str);
                let mime_type = resource.and_then(|r| r.get("mimeType")).and_then(Value::as_str);
                match (blob, mime_type) {
                    (Some(data), Some(mime_type))
                        if (mime_type.starts_with("image/") || mime_type.starts_with("audio/") || mime_type.starts_with("video/")) && within_media_size_cap(data) =>
                    {
                        attachments.push(Attachment { mime_type: mime_type.to_string(), data: data.to_string() });
                        text_parts.push(format!("[resource attached: {mime_type}]"));
                    }
                    _ => text_parts.push(block.to_string()),
                }
            }
            // "resource_link" (a URI, never auto-fetched) and any other block type.
            _ => text_parts.push(block.to_string()),
        }
    }

    (text_parts.join("\n"), attachments)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use serde_json::json;

    use super::*;
    use crate::model::{ChatStream, Response, Role, response_stream};
    use crate::tool::ToolSpec;

    struct MockModel {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl ModelProvider for MockModel {
        async fn chat_stream(&self, messages: Vec<Message>, _tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            if call == 0 {
                Ok(response_stream(Response {
                    content: String::new(),
                    tool_calls: vec![ToolCall { id: "call_1".to_string(), name: "echo".to_string(), arguments: json!({ "text": "hi" }), thought_signature: None }],
                    usage: None,
                }))
            } else {
                let last = messages.last().expect("tool result should have been appended");
                assert_eq!(last.role, Role::Tool);
                assert!(last.content.contains("hi"));
                Ok(response_stream(Response { content: "done".to_string(), tool_calls: Vec::new(), usage: None }))
            }
        }
    }

    struct EchoTool;

    #[async_trait]
    impl Tool for EchoTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec { name: "echo".to_string(), description: "echoes text".to_string(), parameters: json!({}) }
        }

        async fn call(&self, args: serde_json::Value) -> anyhow::Result<serde_json::Value> {
            Ok(args)
        }
    }

    fn temp_vault() -> Arc<Vault> {
        Arc::new(Vault::new(std::env::temp_dir().join(format!(
            "warden-orch-test-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ))))
    }

    #[tokio::test]
    async fn executes_tool_calls_and_returns_final_answer() {
        let model = Arc::new(MockModel { calls: AtomicUsize::new(0) });
        let mut orchestrator = Orchestrator::new(model, temp_vault());
        orchestrator.register_tool(Arc::new(EchoTool));

        let result = orchestrator.handle_message(&[], "say hi").await.unwrap();
        assert_eq!(result.content, "done");
    }

    struct AlwaysToolCallModel;

    #[async_trait]
    impl ModelProvider for AlwaysToolCallModel {
        async fn chat_stream(&self, _messages: Vec<Message>, _tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
            Ok(response_stream(Response {
                content: String::new(),
                tool_calls: vec![ToolCall { id: "call_x".to_string(), name: "echo".to_string(), arguments: json!({}), thought_signature: None }],
                usage: None,
            }))
        }
    }

    #[tokio::test]
    async fn gives_up_after_max_iterations() {
        let mut orchestrator = Orchestrator::new(Arc::new(AlwaysToolCallModel), temp_vault());
        orchestrator.register_tool(Arc::new(EchoTool));

        let result = orchestrator.handle_message(&[], "loop forever").await;
        assert!(result.is_err());
    }

    struct TwoToolProvider;

    #[async_trait]
    impl ToolProvider for TwoToolProvider {
        async fn tools(&self) -> anyhow::Result<Vec<Arc<dyn Tool>>> {
            Ok(vec![Arc::new(EchoTool), Arc::new(EchoTool)])
        }
    }

    #[tokio::test]
    async fn register_provider_registers_every_tool_it_yields() {
        let model = Arc::new(MockModel { calls: AtomicUsize::new(0) });
        let mut orchestrator = Orchestrator::new(model, temp_vault());

        orchestrator.register_provider(&TwoToolProvider).await.unwrap();

        let result = orchestrator.handle_message(&[], "say hi").await.unwrap();
        assert_eq!(result.content, "done");
    }

    /// Echoes the first message's role/content back as its answer — lets a test assert on
    /// exactly what `handle_turn` built without needing a separate recorder/mutex.
    struct EchoesFirstMessageModel;

    #[async_trait]
    impl ModelProvider for EchoesFirstMessageModel {
        async fn chat_stream(&self, messages: Vec<Message>, _tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
            let first = messages.first().expect("at least one message");
            Ok(response_stream(Response { content: format!("{:?}:{}", first.role, first.content), tool_calls: Vec::new(), usage: None }))
        }
    }

    #[tokio::test]
    async fn handle_turn_prepends_the_persona_as_the_first_message() {
        let orchestrator = Orchestrator::new(Arc::new(EchoesFirstMessageModel), temp_vault());

        let result = orchestrator.handle_turn(&[], "hi", Vec::new(), Some("You are a pirate.")).await.unwrap();
        assert_eq!(result.content, "System:You are a pirate.");
    }

    #[tokio::test]
    async fn handle_turn_ignores_a_blank_persona() {
        let orchestrator = Orchestrator::new(Arc::new(EchoesFirstMessageModel), temp_vault());

        let result = orchestrator.handle_turn(&[], "hi", Vec::new(), Some("   ")).await.unwrap();
        assert_eq!(result.content, "User:hi");
    }

    /// Joins every message's role/content into one string, "|"-separated — lets a test assert on
    /// the full ordering `handle_turn` built, not just the first message.
    struct EchoesAllMessagesModel;

    #[async_trait]
    impl ModelProvider for EchoesAllMessagesModel {
        async fn chat_stream(&self, messages: Vec<Message>, _tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
            let joined = messages.iter().map(|m| format!("{:?}:{}", m.role, m.content)).collect::<Vec<_>>().join("|");
            Ok(response_stream(Response { content: joined, tool_calls: Vec::new(), usage: None }))
        }
    }

    #[tokio::test]
    async fn standing_memory_is_injected_between_persona_and_history() {
        let vault = temp_vault();
        vault.write("_profile.md", "Name: Ada.").unwrap();
        let orchestrator = Orchestrator::new(Arc::new(EchoesAllMessagesModel), vault);

        let history = [Message::user("earlier message".to_string())];
        let result =
            orchestrator.handle_turn(&history, "hi", Vec::new(), Some("You are a pirate.")).await.unwrap();

        let parts: Vec<&str> = result.content.split('|').collect();
        assert_eq!(parts[0], "System:You are a pirate.");
        assert_eq!(parts[1], "System:Standing memory from the user's vault (always included, not relevance-dependent):\n\n## User profile\n\nName: Ada.");
        assert_eq!(parts[2], "User:earlier message");
        assert_eq!(parts[3], "User:hi");
    }

    #[tokio::test]
    async fn no_standing_memory_message_when_vault_has_none_of_the_fixed_files() {
        let orchestrator = Orchestrator::new(Arc::new(EchoesAllMessagesModel), temp_vault());

        let result = orchestrator.handle_turn(&[], "hi", Vec::new(), None).await.unwrap();

        assert_eq!(result.content, "User:hi");
    }

    struct NamedModel {
        name: &'static str,
    }

    #[async_trait]
    impl ModelProvider for NamedModel {
        async fn chat_stream(&self, _messages: Vec<Message>, _tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
            Ok(response_stream(Response { content: self.name.to_string(), tool_calls: Vec::new(), usage: None }))
        }
    }

    #[tokio::test]
    async fn with_model_swaps_the_model_used_without_touching_the_original() {
        let orchestrator = Orchestrator::new(Arc::new(NamedModel { name: "a" }), temp_vault());
        let swapped = orchestrator.with_model(Arc::new(NamedModel { name: "b" }));

        assert_eq!(orchestrator.handle_message(&[], "hi").await.unwrap().content, "a");
        assert_eq!(swapped.handle_message(&[], "hi").await.unwrap().content, "b");
    }

    /// Emits its events directly (not via `response_stream`) so tests can assert on the exact
    /// sequence `handle_turn_streaming` forwards to a live callback, not just the final result.
    struct MultiDeltaModel;

    #[async_trait]
    impl ModelProvider for MultiDeltaModel {
        async fn chat_stream(&self, _messages: Vec<Message>, _tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
            let events: Vec<anyhow::Result<StreamEvent>> = vec![
                Ok(StreamEvent::ContentDelta("Hel".to_string())),
                Ok(StreamEvent::ContentDelta("lo".to_string())),
                Ok(StreamEvent::Usage(Usage { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 })),
            ];
            Ok(Box::pin(futures_util::stream::iter(events)))
        }
    }

    #[tokio::test]
    async fn handle_message_streaming_forwards_every_event_in_order() {
        let orchestrator = Orchestrator::new(Arc::new(MultiDeltaModel), temp_vault());
        let observed = std::sync::Mutex::new(Vec::new());

        let result = orchestrator.handle_message_streaming(&[], "hi", |event| observed.lock().unwrap().push(event.clone())).await.unwrap();

        assert_eq!(result.content, "Hello");
        assert_eq!(result.usage, Some(Usage { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 }));

        let observed = observed.into_inner().unwrap();
        assert_eq!(observed.len(), 3);
        assert!(matches!(&observed[0], StreamEvent::ContentDelta(s) if s == "Hel"));
        assert!(matches!(&observed[1], StreamEvent::ContentDelta(s) if s == "lo"));
        assert!(matches!(&observed[2], StreamEvent::Usage(_)));
    }

    /// A non-streaming caller (`handle_message`, no-op sink) must still get exactly the same
    /// `MessageOutcome` as before streaming existed — proof the two paths haven't drifted apart.
    #[tokio::test]
    async fn handle_message_still_returns_the_full_accumulated_outcome() {
        let orchestrator = Orchestrator::new(Arc::new(MultiDeltaModel), temp_vault());
        let result = orchestrator.handle_message(&[], "hi").await.unwrap();
        assert_eq!(result.content, "Hello");
        assert_eq!(result.usage, Some(Usage { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 }));
    }

    struct MidStreamErrorModel;

    #[async_trait]
    impl ModelProvider for MidStreamErrorModel {
        async fn chat_stream(&self, _messages: Vec<Message>, _tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
            let events: Vec<anyhow::Result<StreamEvent>> = vec![Ok(StreamEvent::ContentDelta("partial".to_string())), Err(anyhow::anyhow!("connection dropped"))];
            Ok(Box::pin(futures_util::stream::iter(events)))
        }
    }

    #[tokio::test]
    async fn a_mid_stream_error_propagates_as_an_err() {
        let orchestrator = Orchestrator::new(Arc::new(MidStreamErrorModel), temp_vault());
        let result = orchestrator.handle_message(&[], "hi").await;
        assert!(result.is_err());
    }

    // --- P64 frente 2: media extracted from an MCP-shaped tool result ---

    /// Returns whatever `serde_json::Value` is handed to it in its constructor — lets a test drive
    /// an arbitrary MCP `CallToolResult`-shaped (or not) payload through the orchestrator's tool
    /// loop without needing a real MCP server.
    struct FixedResultTool(Value);

    #[async_trait]
    impl Tool for FixedResultTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec { name: "mcp_tool".to_string(), description: "returns a fixed value".to_string(), parameters: json!({}) }
        }

        async fn call(&self, _args: Value) -> anyhow::Result<Value> {
            Ok(self.0.clone())
        }
    }

    /// Calls `mcp_tool` once, then asserts the tool-result message text (what the model itself
    /// would see next) matches `expects`, before returning a final plain answer.
    struct AssertsToolResultTextModel {
        calls: AtomicUsize,
        expects: &'static str,
    }

    #[async_trait]
    impl ModelProvider for AssertsToolResultTextModel {
        async fn chat_stream(&self, messages: Vec<Message>, _tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            if call == 0 {
                Ok(response_stream(Response {
                    content: String::new(),
                    tool_calls: vec![ToolCall { id: "call_1".to_string(), name: "mcp_tool".to_string(), arguments: json!({}), thought_signature: None }],
                    usage: None,
                }))
            } else {
                let last = messages.last().expect("tool result should have been appended");
                assert_eq!(last.role, Role::Tool);
                assert!(last.content.contains(self.expects), "expected tool-result text to contain {:?}, got {:?}", self.expects, last.content);
                Ok(response_stream(Response { content: "done".to_string(), tool_calls: Vec::new(), usage: None }))
            }
        }
    }

    #[tokio::test]
    async fn extracts_an_image_block_as_an_attachment_and_keeps_base64_out_of_the_models_context() {
        let tool_result = json!({
            "content": [
                { "type": "text", "text": "here is your image" },
                { "type": "image", "data": "aGVsbG8=", "mimeType": "image/png" }
            ]
        });
        let model = Arc::new(AssertsToolResultTextModel { calls: AtomicUsize::new(0), expects: "[image attached: image/png]" });
        let mut orchestrator = Orchestrator::new(model, temp_vault());
        orchestrator.register_tool(Arc::new(FixedResultTool(tool_result)));

        let result = orchestrator.handle_message(&[], "make an image").await.unwrap();

        assert_eq!(result.content, "done");
        assert_eq!(result.attachments, vec![Attachment { mime_type: "image/png".to_string(), data: "aGVsbG8=".to_string() }]);
    }

    #[tokio::test]
    async fn extracts_a_video_resource_block_the_same_way_as_image_audio() {
        let tool_result = json!({
            "content": [
                { "type": "resource", "resource": { "uri": "file:///clip.mp4", "mimeType": "video/mp4", "blob": "dmlkZW8=" } }
            ]
        });
        let model = Arc::new(AssertsToolResultTextModel { calls: AtomicUsize::new(0), expects: "[resource attached: video/mp4]" });
        let mut orchestrator = Orchestrator::new(model, temp_vault());
        orchestrator.register_tool(Arc::new(FixedResultTool(tool_result)));

        let result = orchestrator.handle_message(&[], "make a video").await.unwrap();

        assert_eq!(result.attachments, vec![Attachment { mime_type: "video/mp4".to_string(), data: "dmlkZW8=".to_string() }]);
    }

    #[tokio::test]
    async fn a_resource_link_is_never_auto_fetched_into_an_attachment() {
        let tool_result = json!({
            "content": [
                { "type": "resource_link", "uri": "https://example.com/report.pdf", "mimeType": "application/pdf" }
            ]
        });
        let model = Arc::new(AssertsToolResultTextModel { calls: AtomicUsize::new(0), expects: "resource_link" });
        let mut orchestrator = Orchestrator::new(model, temp_vault());
        orchestrator.register_tool(Arc::new(FixedResultTool(tool_result)));

        let result = orchestrator.handle_message(&[], "fetch a report").await.unwrap();

        assert!(result.attachments.is_empty());
    }

    #[tokio::test]
    async fn a_non_mcp_shaped_tool_result_falls_back_to_the_old_plain_stringify() {
        // No "content" array at all — the shape every non-MCP tool (generate_document, shell, ...)
        // actually returns.
        let tool_result = json!({ "status": "ok", "path": "/tmp/relatorio.pdf" });
        let model = Arc::new(AssertsToolResultTextModel { calls: AtomicUsize::new(0), expects: "/tmp/relatorio.pdf" });
        let mut orchestrator = Orchestrator::new(model, temp_vault());
        orchestrator.register_tool(Arc::new(FixedResultTool(tool_result)));

        let result = orchestrator.handle_message(&[], "generate a report").await.unwrap();

        assert!(result.attachments.is_empty());
    }

    #[test]
    fn within_media_size_cap_accepts_small_payloads_and_rejects_large_ones() {
        let small = "A".repeat(1_000);
        assert!(within_media_size_cap(&small));

        // Comfortably over MAX_INLINE_MEDIA_BYTES once decoded (~0.75 bytes per base64 char) —
        // not an exact-boundary check, since base64 only encodes in whole groups of 3 bytes/4
        // chars and `within_media_size_cap` itself is a byte-length approximation, not an exact
        // decode.
        let large = "A".repeat((MAX_INLINE_MEDIA_BYTES + 1_000_000) * 4 / 3);
        assert!(!within_media_size_cap(&large));
    }
}
