use async_stream::try_stream;
use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Attachment, ChatStream, Message, ModelProvider, Role, StreamEvent, Usage};
use crate::tool::ToolSpec;

const API_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Anthropic's Messages API has no concept of an open-ended response — every request must name
/// a cap. There's no per-request config for it yet (same "no rate limiting/spend cap" scope as
/// the rest of Fase 5.8's `Usage` tracking), so this is just a generous fixed value.
const MAX_TOKENS: u32 = 4096;

pub struct AnthropicProvider {
    api_key: String,
    model: String,
    client: reqwest::Client,
}

impl AnthropicProvider {
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self { api_key: api_key.into(), model: model.into(), client: reqwest::Client::new() }
    }
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ContentBlock {
    Text { text: String },
    Image { source: ImageSource },
    ToolUse { id: String, name: String, input: Value },
    ToolResult { tool_use_id: String, content: String },
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
struct ImageSource {
    #[serde(rename = "type")]
    kind: &'static str,
    media_type: String,
    data: String,
}

fn attachment_block(attachment: Attachment) -> ContentBlock {
    ContentBlock::Image { source: ImageSource { kind: "base64", media_type: attachment.mime_type, data: attachment.data } }
}

#[derive(Serialize)]
struct AnthropicMessage {
    role: &'static str,
    content: Vec<ContentBlock>,
}

#[derive(Serialize)]
struct AnthropicTool {
    name: String,
    description: String,
    input_schema: Value,
}

#[derive(Serialize)]
struct MessagesRequest {
    model: String,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<AnthropicTool>,
    stream: bool,
}

/// One SSE event's `data:` payload, tagged by its own `type` field (mirrors the `event:` line,
/// so there's no need to also thread `Event::event` through — see `map_stream_event`). Variants
/// this integration doesn't act on (`ping`, block-stop markers) still need to parse successfully
/// so an unremarkable event doesn't fail the whole stream.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum StreamMessage {
    MessageStart { message: MessageStartMessage },
    ContentBlockStart { index: usize, content_block: StreamContentBlock },
    ContentBlockDelta { index: usize, delta: StreamDelta },
    ContentBlockStop,
    MessageDelta { #[serde(default)] usage: Option<DeltaUsage> },
    MessageStop,
    Ping,
    Error { error: StreamError },
}

#[derive(Deserialize)]
struct MessageStartMessage {
    #[serde(default)]
    usage: Option<StartUsage>,
}

#[derive(Deserialize)]
struct StartUsage {
    #[serde(default)]
    input_tokens: u32,
}

#[derive(Deserialize)]
struct DeltaUsage {
    #[serde(default)]
    output_tokens: u32,
}

#[derive(Deserialize)]
struct StreamError {
    #[serde(default)]
    message: String,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum StreamContentBlock {
    ToolUse { id: String, name: String },
    /// Catches block types this integration doesn't act on at start time (e.g. plain `text`,
    /// which needs no setup — its content arrives entirely via `content_block_delta`).
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum StreamDelta {
    TextDelta { text: String },
    InputJsonDelta { partial_json: String },
    #[serde(other)]
    Other,
}

/// Maps one SSE `data:` payload to zero or more `StreamEvent`s. `input_tokens` threads across
/// calls for one stream — Anthropic reports it once on `message_start` and only reports
/// `output_tokens` later on `message_delta`, so the two halves of `Usage` have to be combined
/// across two otherwise-unrelated events.
fn map_stream_event(data: &str, input_tokens: &mut u32) -> anyhow::Result<Vec<StreamEvent>> {
    let message: StreamMessage = serde_json::from_str(data)?;
    let mut events = Vec::new();

    match message {
        StreamMessage::MessageStart { message } => {
            if let Some(usage) = message.usage {
                *input_tokens = usage.input_tokens;
            }
        }
        StreamMessage::ContentBlockStart { index, content_block: StreamContentBlock::ToolUse { id, name } } => {
            events.push(StreamEvent::ToolCallDelta { index, id: Some(id), name: Some(name), arguments_delta: None, thought_signature: None });
        }
        StreamMessage::ContentBlockStart { .. } => {}
        StreamMessage::ContentBlockDelta { delta: StreamDelta::TextDelta { text }, .. } => {
            events.push(StreamEvent::ContentDelta(text));
        }
        StreamMessage::ContentBlockDelta { index, delta: StreamDelta::InputJsonDelta { partial_json } } => {
            events.push(StreamEvent::ToolCallDelta { index, id: None, name: None, arguments_delta: Some(partial_json), thought_signature: None });
        }
        StreamMessage::ContentBlockDelta { .. } => {}
        StreamMessage::ContentBlockStop => {}
        StreamMessage::MessageDelta { usage: Some(usage) } => {
            events.push(StreamEvent::Usage(Usage {
                prompt_tokens: *input_tokens,
                completion_tokens: usage.output_tokens,
                total_tokens: *input_tokens + usage.output_tokens,
            }));
        }
        StreamMessage::MessageDelta { usage: None } => {}
        StreamMessage::MessageStop | StreamMessage::Ping => {}
        StreamMessage::Error { error } => anyhow::bail!("Anthropic stream error: {}", error.message),
    }

    Ok(events)
}

fn to_anthropic_message(message: Message) -> AnthropicMessage {
    match message.role {
        Role::System => unreachable!("system messages are pulled out into `system` before this point"),
        // Anthropic has no dedicated tool-result role — a tool result is a `user` message
        // carrying a `tool_result` content block instead.
        Role::Tool => AnthropicMessage {
            role: "user",
            content: vec![ContentBlock::ToolResult { tool_use_id: message.tool_call_id.unwrap_or_default(), content: message.content }],
        },
        Role::Assistant if !message.tool_calls.is_empty() => AnthropicMessage {
            role: "assistant",
            content: message
                .tool_calls
                .into_iter()
                .map(|tc| ContentBlock::ToolUse { id: tc.id, name: tc.name, input: tc.arguments })
                .collect(),
        },
        Role::Assistant => AnthropicMessage { role: "assistant", content: vec![ContentBlock::Text { text: message.content }] },
        Role::User => {
            let mut content: Vec<ContentBlock> = message.attachments.into_iter().map(attachment_block).collect();
            content.push(ContentBlock::Text { text: message.content });
            AnthropicMessage { role: "user", content }
        }
    }
}

#[async_trait]
impl ModelProvider for AnthropicProvider {
    async fn chat_stream(&self, messages: Vec<Message>, tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
        let mut system = None;
        let mut anthropic_messages = Vec::new();
        for message in messages {
            if message.role == Role::System {
                system = Some(message.content);
            } else {
                anthropic_messages.push(to_anthropic_message(message));
            }
        }

        let request = MessagesRequest {
            model: self.model.clone(),
            max_tokens: MAX_TOKENS,
            system,
            messages: anthropic_messages,
            tools: tools.into_iter().map(|t| AnthropicTool { name: t.name, description: t.description, input_schema: t.parameters }).collect(),
            stream: true,
        };

        let response = self
            .client
            .post(API_URL)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Anthropic API error ({status}): {body}");
        }

        Ok(Box::pin(try_stream! {
            let mut input_tokens = 0u32;
            let mut frames = response.bytes_stream().eventsource();
            while let Some(frame) = frames.next().await {
                let frame = frame?;
                for event in map_stream_event(&frame.data, &mut input_tokens)? {
                    yield event;
                }
            }
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_user_message_has_a_single_text_block() {
        let json = serde_json::to_value(to_anthropic_message(Message::user("hi"))).unwrap();
        let content = json["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "text");
    }

    #[test]
    fn attachments_become_image_blocks_before_the_text_block() {
        let message = Message::user_with_attachments("what's this?", vec![Attachment { mime_type: "image/jpeg".to_string(), data: "AAAA".to_string() }]);
        let json = serde_json::to_value(to_anthropic_message(message)).unwrap();

        let content = json["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["type"], "image");
        assert_eq!(content[0]["source"]["type"], "base64");
        assert_eq!(content[0]["source"]["media_type"], "image/jpeg");
        assert_eq!(content[0]["source"]["data"], "AAAA");
        assert_eq!(content[1]["type"], "text");
        assert_eq!(content[1]["text"], "what's this?");
    }

    #[test]
    fn a_text_delta_maps_to_one_content_delta_event() {
        let mut input_tokens = 0;
        let events = map_stream_event(r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hi"}}"#, &mut input_tokens).unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], StreamEvent::ContentDelta(s) if s == "Hi"));
    }

    #[test]
    fn a_tool_use_block_start_carries_id_and_name_with_no_arguments_yet() {
        let mut input_tokens = 0;
        let events = map_stream_event(r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_1","name":"search","input":{}}}"#, &mut input_tokens).unwrap();
        assert_eq!(events.len(), 1);
        match &events[0] {
            StreamEvent::ToolCallDelta { index, id, name, arguments_delta, .. } => {
                assert_eq!(*index, 1);
                assert_eq!(id.as_deref(), Some("toolu_1"));
                assert_eq!(name.as_deref(), Some("search"));
                assert_eq!(*arguments_delta, None);
            }
            other => panic!("expected ToolCallDelta, got {other:?}"),
        }
    }

    #[test]
    fn an_input_json_delta_carries_only_the_arguments_fragment() {
        let mut input_tokens = 0;
        let events = map_stream_event(r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"q\":"}}"#, &mut input_tokens).unwrap();
        match &events[0] {
            StreamEvent::ToolCallDelta { id, name, arguments_delta, .. } => {
                assert_eq!(*id, None);
                assert_eq!(*name, None);
                assert_eq!(arguments_delta.as_deref(), Some(r#"{"q":"#));
            }
            other => panic!("expected ToolCallDelta, got {other:?}"),
        }
    }

    #[test]
    fn usage_is_combined_from_message_start_and_message_delta() {
        let mut input_tokens = 0;
        let start = map_stream_event(r#"{"type":"message_start","message":{"usage":{"input_tokens":25}}}"#, &mut input_tokens).unwrap();
        assert!(start.is_empty());
        assert_eq!(input_tokens, 25);

        let delta = map_stream_event(r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":15}}"#, &mut input_tokens).unwrap();
        assert_eq!(delta.len(), 1);
        match &delta[0] {
            StreamEvent::Usage(usage) => {
                assert_eq!(usage.prompt_tokens, 25);
                assert_eq!(usage.completion_tokens, 15);
                assert_eq!(usage.total_tokens, 40);
            }
            other => panic!("expected Usage, got {other:?}"),
        }
    }

    #[test]
    fn ping_and_stop_events_produce_no_events_and_do_not_error() {
        let mut input_tokens = 0;
        assert!(map_stream_event(r#"{"type":"ping"}"#, &mut input_tokens).unwrap().is_empty());
        assert!(map_stream_event(r#"{"type":"content_block_stop","index":0}"#, &mut input_tokens).unwrap().is_empty());
        assert!(map_stream_event(r#"{"type":"message_stop"}"#, &mut input_tokens).unwrap().is_empty());
    }

    #[test]
    fn an_error_event_bails_with_the_api_message() {
        let mut input_tokens = 0;
        let err = map_stream_event(r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#, &mut input_tokens).unwrap_err();
        assert!(err.to_string().contains("Overloaded"));
    }
}
