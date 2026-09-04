use std::sync::Arc;

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
                return Ok(MessageOutcome { content: response.content, usage: has_usage.then_some(usage) });
            }

            messages.push(Message::assistant_tool_calls(response.tool_calls.clone()));

            for tool_call in &response.tool_calls {
                let content = match self.run_tool(tool_call).await {
                    Ok(value) => value.to_string(),
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
}
