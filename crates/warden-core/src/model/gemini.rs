use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{Message, ModelProvider, Response, Role, ToolCall};
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
    #[serde(skip_serializing_if = "Option::is_none", rename = "functionCall")]
    function_call: Option<FunctionCallPart>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "functionResponse")]
    function_response: Option<FunctionResponsePart>,
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
        Role::User => Content { role: Some("user"), parts: vec![Part::text(message.content)] },
        Role::Assistant if !message.tool_calls.is_empty() => Content {
            role: Some("model"),
            parts: message
                .tool_calls
                .into_iter()
                .map(|tc| Part { function_call: Some(FunctionCallPart { name: tc.name, args: tc.arguments }), ..Default::default() })
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
        let parts = parsed.candidates.into_iter().next().map(|c| c.content.parts).unwrap_or_default();

        let mut content = String::new();
        let mut tool_calls = Vec::new();
        for (i, part) in parts.into_iter().enumerate() {
            if let Some(text) = part.text {
                content.push_str(&text);
            } else if let Some(call) = part.function_call {
                tool_calls.push(ToolCall { id: format!("call_{i}"), name: call.name, arguments: call.args });
            }
        }

        Ok(Response { content, tool_calls })
    }
}
