use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{Attachment, Message, ModelProvider, Response, Role, ToolCall, Usage};
use crate::tool::ToolSpec;

const API_BASE: &str = "https://generativelanguage.googleapis.com/v1beta/models";

pub struct GeminiProvider {
    api_key: String,
    model: String,
    client: reqwest::Client,
}

impl GeminiProvider {
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            model: model.into(),
            client: reqwest::Client::new(),
        }
    }
}

#[derive(Serialize, Default)]
struct Part {
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "inlineData")]
    inline_data: Option<InlineData>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "functionCall")]
    function_call: Option<FunctionCallPart>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "functionResponse")]
    function_response: Option<FunctionResponsePart>,
    /// Sibling of `functionCall` within the same part, not nested inside it — see
    /// `ToolCall::thought_signature` for why this needs to round-trip at all.
    #[serde(skip_serializing_if = "Option::is_none", rename = "thoughtSignature")]
    thought_signature: Option<String>,
}

#[derive(Serialize)]
struct InlineData {
    #[serde(rename = "mimeType")]
    mime_type: String,
    data: String,
}

fn attachment_part(attachment: Attachment) -> Part {
    Part { inline_data: Some(InlineData { mime_type: attachment.mime_type, data: attachment.data }), ..Default::default() }
}

#[derive(Serialize)]
struct FunctionCallPart {
    name: String,
    args: Value,
}

#[derive(Serialize)]
struct FunctionResponsePart {
    name: String,
    response: Value,
}

impl Part {
    fn text(text: String) -> Self {
        Self { text: Some(text), ..Default::default() }
    }
}

#[derive(Serialize)]
struct Content {
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<&'static str>,
    parts: Vec<Part>,
}

#[derive(Serialize)]
struct FunctionDeclaration {
    name: String,
    description: String,
    parameters: Value,
}

#[derive(Serialize)]
struct GeminiTool {
    function_declarations: Vec<FunctionDeclaration>,
}

#[derive(Serialize)]
struct GenerateRequest {
    contents: Vec<Content>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system_instruction: Option<Content>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<GeminiTool>,
}

#[derive(Deserialize, Default)]
struct GenerateResponse {
    #[serde(default)]
    candidates: Vec<Candidate>,
    #[serde(default, rename = "usageMetadata")]
    usage_metadata: Option<UsageMetadata>,
}

#[derive(Deserialize)]
struct UsageMetadata {
    #[serde(rename = "promptTokenCount", default)]
    prompt_token_count: u32,
    #[serde(rename = "candidatesTokenCount", default)]
    candidates_token_count: u32,
    #[serde(rename = "totalTokenCount", default)]
    total_token_count: u32,
}

#[derive(Deserialize)]
struct Candidate {
    content: ResponseContent,
}

#[derive(Deserialize, Default)]
struct ResponseContent {
    #[serde(default)]
    parts: Vec<ResponsePart>,
}

#[derive(Deserialize, Default)]
struct ResponsePart {
    #[serde(default)]
    text: Option<String>,
    #[serde(default, rename = "functionCall")]
    function_call: Option<IncomingFunctionCall>,
    #[serde(default, rename = "thoughtSignature")]
    thought_signature: Option<String>,
}

#[derive(Deserialize)]
struct IncomingFunctionCall {
    name: String,
    #[serde(default)]
    args: Value,
}

fn to_content(message: Message) -> Content {
    match message.role {
        Role::System => unreachable!("system messages are pulled out into system_instruction before this point"),
        Role::User => {
            let mut parts: Vec<Part> = message.attachments.into_iter().map(attachment_part).collect();
            parts.push(Part::text(message.content));
            Content { role: Some("user"), parts }
        }
        Role::Assistant if !message.tool_calls.is_empty() => Content {
            role: Some("model"),
            parts: message
                .tool_calls
                .into_iter()
                .map(|tc| Part {
                    function_call: Some(FunctionCallPart { name: tc.name, args: tc.arguments }),
                    thought_signature: tc.thought_signature,
                    ..Default::default()
                })
                .collect(),
        },
        Role::Assistant => Content { role: Some("model"), parts: vec![Part::text(message.content)] },
        Role::Tool => Content {
            role: Some("function"),
            parts: vec![Part {
                function_response: Some(FunctionResponsePart {
                    name: message.tool_name.unwrap_or_default(),
                    response: json!({ "content": message.content }),
                }),
                ..Default::default()
            }],
        },
    }
}

#[async_trait]
impl ModelProvider for GeminiProvider {
    async fn chat(&self, messages: Vec<Message>, tools: Vec<ToolSpec>) -> anyhow::Result<Response> {
        let mut system_instruction = None;
        let mut contents = Vec::new();

        for message in messages {
            if message.role == Role::System {
                system_instruction = Some(Content { role: None, parts: vec![Part::text(message.content)] });
            } else {
                contents.push(to_content(message));
            }
        }

        let gemini_tools = if tools.is_empty() {
            Vec::new()
        } else {
            vec![GeminiTool {
                function_declarations: tools
                    .into_iter()
                    .map(|t| FunctionDeclaration {
                        name: t.name,
                        description: t.description,
                        parameters: t.parameters,
                    })
                    .collect(),
            }]
        };

        let request = GenerateRequest { contents, system_instruction, tools: gemini_tools };
        let url = format!("{API_BASE}/{}:generateContent", self.model);

        let response = self
            .client
            .post(url)
            .header("x-goog-api-key", &self.api_key)
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Gemini API error ({status}): {body}");
        }

        let parsed: GenerateResponse = response.json().await?;
        let usage = parsed.usage_metadata.map(|u| Usage {
            prompt_tokens: u.prompt_token_count,
            completion_tokens: u.candidates_token_count,
            total_tokens: u.total_token_count,
        });
        let parts = parsed.candidates.into_iter().next().map(|c| c.content.parts).unwrap_or_default();

        let mut content = String::new();
        let mut tool_calls = Vec::new();
        for (i, part) in parts.into_iter().enumerate() {
            if let Some(text) = part.text {
                content.push_str(&text);
            } else if let Some(call) = part.function_call {
                tool_calls.push(ToolCall {
                    id: format!("call_{i}"),
                    name: call.name,
                    arguments: call.args,
                    thought_signature: part.thought_signature,
                });
            }
        }

        Ok(Response { content, tool_calls, usage })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_user_message_has_a_single_text_part() {
        let json = serde_json::to_value(to_content(Message::user("hi"))).unwrap();
        let parts = json["parts"].as_array().unwrap();
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0]["text"], "hi");
    }

    #[test]
    fn attachments_become_inline_data_parts_before_the_text_part() {
        let message = Message::user_with_attachments("what's this?", vec![Attachment { mime_type: "image/webp".to_string(), data: "AAAA".to_string() }]);
        let json = serde_json::to_value(to_content(message)).unwrap();

        let parts = json["parts"].as_array().unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0]["inlineData"]["mimeType"], "image/webp");
        assert_eq!(parts[0]["inlineData"]["data"], "AAAA");
        assert_eq!(parts[1]["text"], "what's this?");
    }

    /// Gemini's "thinking" models reject a follow-up request that's missing this on a
    /// function-call part it previously returned it on (400 INVALID_ARGUMENT) — confirms it
    /// round-trips as a sibling of `functionCall`, not nested inside it.
    #[test]
    fn a_tool_calls_thought_signature_is_echoed_back_as_a_sibling_of_function_call() {
        let tool_call = ToolCall {
            id: "call_1".to_string(),
            name: "delegate_task".to_string(),
            arguments: json!({ "task": "say hi" }),
            thought_signature: Some("opaque-blob".to_string()),
        };
        let message = Message::assistant_tool_calls(vec![tool_call]);
        let json = serde_json::to_value(to_content(message)).unwrap();

        let parts = json["parts"].as_array().unwrap();
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0]["functionCall"]["name"], "delegate_task");
        assert_eq!(parts[0]["thoughtSignature"], "opaque-blob");
    }

    /// When the provider never returned a signature (older/non-thinking models), nothing new
    /// should appear on the wire — same shape as before this field existed.
    #[test]
    fn a_missing_thought_signature_is_omitted_from_the_wire_payload() {
        let tool_call = ToolCall { id: "call_1".to_string(), name: "read_file".to_string(), arguments: json!({}), thought_signature: None };
        let message = Message::assistant_tool_calls(vec![tool_call]);
        let json = serde_json::to_value(to_content(message)).unwrap();

        assert!(json["parts"][0].get("thoughtSignature").is_none());
    }
}
