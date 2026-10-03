use std::collections::BTreeMap;
use std::pin::Pin;

use async_trait::async_trait;
use futures_core::Stream;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::tool::ToolSpec;

pub mod anthropic;
pub mod fallback;
pub mod gemini;
pub mod key_check;
pub mod labeled;
pub mod openai;

pub use fallback::{FallbackProvider, ProviderFallback};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    System,
    User,
    Assistant,
    /// Result of a tool call, fed back to the model.
    Tool,
}

/// A tool invocation requested by the model — either present on an assistant
/// `Message` (what the model asked to run) or standalone in a `Response`
/// (what the provider just parsed out of the model's reply).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
    /// Gemini's "thinking" models (e.g. `gemini-3.x`) attach an opaque signature to a
    /// function-call part and then require it echoed back on that same part in the next turn's
    /// request — omitting it makes the API reject the request outright (400 INVALID_ARGUMENT)
    /// once the conversation has more than one turn involving a tool call. Always `None` for
    /// OpenAI/Anthropic, which have no equivalent concept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thought_signature: Option<String>,
}

/// Inline media attached to a user message: an image (P28) or, since P78, a PDF
/// (`PDF_MIME_TYPE`), which all three providers now take inline as base64 (Anthropic's `document`
/// block, OpenAI's `file` part, Gemini's `inlineData`). `data` is raw base64, without a
/// `data:...;base64,` prefix.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    pub mime_type: String,
    pub data: String,
}

pub const PDF_MIME_TYPE: &str = "application/pdf";

/// The mime types a user may attach to a turn — what every provider accepts inline.
pub const USER_ATTACHMENT_MIME_TYPES: &[&str] = &["image/png", "image/jpeg", "image/webp", "image/gif", PDF_MIME_TYPE];

/// Serializable so a hub can send a turn to a node's own model (P93) — the wire shape is this one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
    /// Only set on `Role::Assistant` messages that requested tool calls.
    pub tool_calls: Vec<ToolCall>,
    /// Only set on `Role::Tool` messages: which call this is answering.
    pub tool_call_id: Option<String>,
    /// Only set on `Role::Tool` messages: the name of the tool that ran
    /// (some providers, e.g. Gemini, key tool results by name rather than id).
    pub tool_name: Option<String>,
    /// Only meaningful on `Role::User` messages — images attached to that turn (P28).
    pub attachments: Vec<Attachment>,
}

impl Message {
    pub fn system(content: impl Into<String>) -> Self {
        Self { role: Role::System, content: content.into(), tool_calls: Vec::new(), tool_call_id: None, tool_name: None, attachments: Vec::new() }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self { role: Role::User, content: content.into(), tool_calls: Vec::new(), tool_call_id: None, tool_name: None, attachments: Vec::new() }
    }

    pub fn user_with_attachments(content: impl Into<String>, attachments: Vec<Attachment>) -> Self {
        Self { role: Role::User, content: content.into(), tool_calls: Vec::new(), tool_call_id: None, tool_name: None, attachments }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self { role: Role::Assistant, content: content.into(), tool_calls: Vec::new(), tool_call_id: None, tool_name: None, attachments: Vec::new() }
    }

    pub fn assistant_tool_calls(tool_calls: Vec<ToolCall>) -> Self {
        Self { role: Role::Assistant, content: String::new(), tool_calls, tool_call_id: None, tool_name: None, attachments: Vec::new() }
    }

    pub fn tool_result(tool_call: &ToolCall, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: Some(tool_call.id.clone()),
            tool_name: Some(tool_call.name.clone()),
            attachments: Vec::new(),
        }
    }
}

/// Token accounting from one `chat` call, when the provider reports it. Shared between the
/// providers (which parse it out of their own response shape) and `warden_bootstrap::
/// ConversationMessage` (which persists it) — `camelCase` on the wire so it matches the rest of
/// bootstrap's persisted JSON convention.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

/// Accumulates one call's usage into a running total — shared by the `warden-cli` REPL's
/// `/usage` (session-only) and `warden-bootstrap`'s cross-conversation aggregation, so both add
/// up the same three fields the same way.
impl std::ops::AddAssign<&Usage> for Usage {
    fn add_assign(&mut self, other: &Usage) {
        self.prompt_tokens += other.prompt_tokens;
        self.completion_tokens += other.completion_tokens;
        self.total_tokens += other.total_tokens;
    }
}

#[derive(Debug, Clone, Default)]
pub struct Response {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub usage: Option<Usage>,
}

/// One fragment of a streaming `chat_stream` call. Providers emit these as their HTTP response
/// arrives incrementally (SSE); `ResponseAccumulator` reassembles them into the same `Response`
/// shape a non-streaming call would have produced.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum StreamEvent {
    /// A fragment of the assistant's text content, in arrival order.
    ContentDelta(String),
    /// A fragment of one tool call, keyed by `index` (its position among the tool calls in this
    /// turn — OpenAI/Anthropic split a single call's `id`/`name`/arguments across several of
    /// these). `id`/`name` typically arrive once, on the first fragment for that index;
    /// `arguments_delta` fragments are concatenated in arrival order and parsed as JSON only once
    /// the stream ends. Gemini has no notion of a partial function call (its `args` is a native
    /// JSON object on the wire, not a string, so it can't be chunked) — its provider emits one
    /// whole `ToolCallDelta` per call instead, which is just a degenerate case of the same shape.
    /// `thought_signature` is Gemini-only, see `ToolCall::thought_signature`.
    ToolCallDelta { index: usize, id: Option<String>, name: Option<String>, arguments_delta: Option<String>, thought_signature: Option<String> },
    /// Token accounting for the whole turn. Providers that report it emit this once, at or near
    /// the end of the stream.
    Usage(Usage),
    /// The provider this call was meant for failed and another one answered (P79) — emitted
    /// first, by `FallbackProvider`, only when that happened. Carries no content.
    ProviderFallback(ProviderFallback),
}

/// A provider's API answered with an HTTP error status. Its `Display` is the message the
/// providers always bailed with; the fields let `FallbackProvider` tell a busy provider (429,
/// 5xx) from a request that would fail anywhere (400, a bad key).
#[derive(Debug, Clone)]
pub struct ProviderHttpError {
    /// "Gemini", "OpenAI", "Anthropic".
    pub provider: &'static str,
    pub status: u16,
    /// The status line, e.g. "503 Service Unavailable".
    pub reason: String,
    pub body: String,
}

impl std::fmt::Display for ProviderHttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} API error ({}): {}", self.provider, self.reason, self.body)
    }
}

impl std::error::Error for ProviderHttpError {}

/// The provider can't be reached right now — a node (P93) that is offline, not approved, switched off
/// or closed to this agent, or a node model asked for outside a hub. `FallbackProvider` treats it
/// like a 503: a combo moves on to its next provider.
#[derive(Debug, Clone)]
pub struct ProviderUnavailable(pub String);

impl std::fmt::Display for ProviderUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ProviderUnavailable {}

pub type ChatStream = Pin<Box<dyn Stream<Item = anyhow::Result<StreamEvent>> + Send>>;

#[derive(Default)]
struct PartialToolCall {
    id: String,
    name: String,
    arguments: String,
    thought_signature: Option<String>,
}

/// Reassembles a sequence of `StreamEvent`s back into one `Response` — the single piece of logic
/// shared by `ModelProvider::chat`'s default implementation (drains a whole stream, non-streaming
/// callers never see the deltas) and `Orchestrator::handle_turn_streaming` (forwards each event to
/// a live callback *and* accumulates them the same way, so the two paths can never drift apart).
#[derive(Default)]
struct ResponseAccumulator {
    content: String,
    tool_calls: BTreeMap<usize, PartialToolCall>,
    usage: Option<Usage>,
}

impl ResponseAccumulator {
    fn apply(&mut self, event: StreamEvent) {
        match event {
            StreamEvent::ContentDelta(delta) => self.content.push_str(&delta),
            StreamEvent::ToolCallDelta { index, id, name, arguments_delta, thought_signature } => {
                let entry = self.tool_calls.entry(index).or_default();
                if let Some(id) = id {
                    entry.id = id;
                }
                if let Some(name) = name {
                    entry.name = name;
                }
                if let Some(delta) = arguments_delta {
                    entry.arguments.push_str(&delta);
                }
                if thought_signature.is_some() {
                    entry.thought_signature = thought_signature;
                }
            }
            StreamEvent::Usage(usage) => self.usage = Some(usage),
            // Who answered is the orchestrator's concern (it sees every event), not the response's.
            StreamEvent::ProviderFallback(_) => {}
        }
    }

    fn finish(self) -> Response {
        let tool_calls = self
            .tool_calls
            .into_values()
            .map(|partial| ToolCall {
                id: partial.id,
                name: partial.name,
                arguments: if partial.arguments.is_empty() {
                    Value::Null
                } else {
                    serde_json::from_str(&partial.arguments).unwrap_or(Value::Null)
                },
                thought_signature: partial.thought_signature,
            })
            .collect();

        Response { content: self.content, tool_calls, usage: self.usage }
    }
}

/// Drains a `ChatStream` into one `Response`, calling `on_event` with each event as it arrives —
/// the single piece of accumulation logic shared by `ModelProvider::chat`'s default
/// implementation (passes a no-op sink) and `Orchestrator::handle_turn_streaming` (passes a sink
/// that forwards to its caller's live callback), so the two paths can never drift apart.
pub(crate) async fn drain_chat_stream(mut stream: ChatStream, mut on_event: impl FnMut(&StreamEvent)) -> anyhow::Result<Response> {
    let mut acc = ResponseAccumulator::default();
    while let Some(event) = stream.next().await {
        let event = event?;
        on_event(&event);
        acc.apply(event);
    }
    Ok(acc.finish())
}

/// Adapts an already-complete `Response` into a (non-incremental, single-item) `ChatStream` —
/// lets a test double or any other "I already know the whole answer" caller satisfy
/// `ModelProvider::chat_stream` without duplicating the delta-emitting logic real providers use.
pub fn response_stream(response: Response) -> ChatStream {
    let mut events = Vec::new();
    if !response.content.is_empty() {
        events.push(StreamEvent::ContentDelta(response.content));
    }
    for (index, tool_call) in response.tool_calls.into_iter().enumerate() {
        events.push(StreamEvent::ToolCallDelta {
            index,
            id: Some(tool_call.id),
            name: Some(tool_call.name),
            arguments_delta: Some(tool_call.arguments.to_string()),
            thought_signature: tool_call.thought_signature,
        });
    }
    if let Some(usage) = response.usage {
        events.push(StreamEvent::Usage(usage));
    }
    Box::pin(futures_util::stream::iter(events.into_iter().map(Ok)))
}

/// Abstraction implemented by each AI provider (OpenAI, Anthropic, Gemini, local).
/// The orchestrator only ever talks to this trait — never to a concrete provider.
///
/// `chat_stream` is the one method a provider must implement — it should call the API's
/// streaming variant and emit `StreamEvent`s as they arrive. `chat` is a convenience default
/// for callers that don't care about incremental output (Telegram, WhatsApp, the desktop app,
/// `DelegateTool`): it just drains `chat_stream` into one `Response`, so those callers see
/// identical behavior to before this trait had streaming at all. One new failure mode versus the
/// old buffered-JSON world: a connection can now drop *mid-stream*, after some content already
/// arrived — that partial content is discarded and the call still surfaces as a plain `Err`,
/// exactly as a connection failure would have looked before.
#[async_trait]
pub trait ModelProvider: Send + Sync {
    /// The model id this provider sends (`gpt-4o`, ...), which is what a price is looked up by
    /// (`spend::PriceTable`). Empty for a provider that doesn't say — its calls still count against
    /// token limits, just not dollar ones.
    fn model_id(&self) -> &str {
        ""
    }

    /// The id the person gave this provider (a `[[providers]]` id, or a combo's), which the spend ledger keeps so
    /// dollars can be shown per provider (P10). Empty for a provider that isn't labelled. A combo that falls back
    /// reports the member that answered through `StreamEvent::ProviderFallback`, not this.
    fn provider_id(&self) -> &str {
        ""
    }

    /// Checks the key against the provider without spending a conversation (P10): a model-list request, answered
    /// only to a key the provider accepts. A provider with no key to check (a node's model, a combo) says so, which
    /// is the default. It never goes through an `Orchestrator`, so it books nothing in the spend ledger.
    async fn check_key(&self) -> key_check::KeyCheck {
        key_check::KeyCheck::Unsupported("This kind of provider has no key to check.".to_string())
    }

    async fn chat_stream(&self, messages: Vec<Message>, tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream>;

    /// A copy that knows which agent is asking (P93: a node's model is only for the agents that
    /// node allows), or `None` when the agent makes no difference — the default. Applied by
    /// `Orchestrator::with_agent`/`with_model`, like `Tool::scoped_to_agent`.
    fn for_agent(&self, _agent: Option<&str>) -> Option<std::sync::Arc<dyn ModelProvider>> {
        None
    }

    async fn chat(&self, messages: Vec<Message>, tools: Vec<ToolSpec>) -> anyhow::Result<Response> {
        let stream = self.chat_stream(messages, tools).await?;
        drain_chat_stream(stream, |_| {}).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn response_accumulator_reassembles_interleaved_tool_call_deltas() {
        let mut acc = ResponseAccumulator::default();
        acc.apply(StreamEvent::ContentDelta("Hel".to_string()));
        acc.apply(StreamEvent::ToolCallDelta { index: 0, id: Some("call_0".to_string()), name: Some("search".to_string()), arguments_delta: Some(r#"{"q":"#.to_string()), thought_signature: None });
        acc.apply(StreamEvent::ContentDelta("lo".to_string()));
        acc.apply(StreamEvent::ToolCallDelta { index: 1, id: Some("call_1".to_string()), name: Some("read_file".to_string()), arguments_delta: Some(r#"{"path":"a"}"#.to_string()), thought_signature: None });
        acc.apply(StreamEvent::ToolCallDelta { index: 0, id: None, name: None, arguments_delta: Some(r#""x"}"#.to_string()), thought_signature: None });
        acc.apply(StreamEvent::Usage(Usage { prompt_tokens: 1, completion_tokens: 2, total_tokens: 3 }));

        let response = acc.finish();
        assert_eq!(response.content, "Hello");
        assert_eq!(response.tool_calls.len(), 2);
        assert_eq!(response.tool_calls[0].id, "call_0");
        assert_eq!(response.tool_calls[0].arguments, serde_json::json!({"q": "x"}));
        assert_eq!(response.tool_calls[1].id, "call_1");
        assert_eq!(response.tool_calls[1].arguments, serde_json::json!({"path": "a"}));
        assert_eq!(response.usage, Some(Usage { prompt_tokens: 1, completion_tokens: 2, total_tokens: 3 }));
    }

    #[tokio::test]
    async fn chat_default_impl_drains_chat_stream() {
        struct StubProvider;
        #[async_trait]
        impl ModelProvider for StubProvider {
            async fn chat_stream(&self, _messages: Vec<Message>, _tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
                Ok(response_stream(Response { content: "hi".to_string(), tool_calls: Vec::new(), usage: None }))
            }
        }

        let response = StubProvider.chat(Vec::new(), Vec::new()).await.unwrap();
        assert_eq!(response.content, "hi");
    }
}
