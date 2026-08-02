use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Message, ModelProvider, Response, Role};
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

#[derive(Serialize)]
struct Part {
    text: String,
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
    text: String,
}

#[async_trait]
impl ModelProvider for GeminiProvider {
    async fn chat(&self, messages: Vec<Message>, tools: Vec<ToolSpec>) -> anyhow::Result<Response> {
        let mut system_instruction = None;
        let mut contents = Vec::new();

        for message in messages {
            match message.role {
                Role::System => {
                    system_instruction = Some(Content { role: None, parts: vec![Part { text: message.content }] });
                }
                Role::User => contents.push(Content { role: Some("user"), parts: vec![Part { text: message.content }] }),
                Role::Assistant => contents.push(Content { role: Some("model"), parts: vec![Part { text: message.content }] }),
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
        let content = parsed
            .candidates
            .into_iter()
            .next()
            .and_then(|c| c.content.parts.into_iter().next())
            .map(|p| p.text)
            .unwrap_or_default();

        Ok(Response { content })
    }
}
