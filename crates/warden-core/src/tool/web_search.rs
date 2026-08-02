use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::tool::{Tool, ToolSpec};

const API_URL: &str = "https://api.tavily.com/search";

/// Web search via the Tavily API (tavily.com) — built for LLM agent tool use, so
/// results come back as short content snippets instead of raw HTML to scrape.
pub struct WebSearchTool {
    api_key: String,
    client: reqwest::Client,
}

impl WebSearchTool {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self { api_key: api_key.into(), client: reqwest::Client::new() }
    }
}

#[derive(Serialize)]
struct SearchRequest<'a> {
    api_key: &'a str,
    query: &'a str,
    max_results: u32,
}

#[derive(Deserialize, Default)]
struct SearchResponse {
    #[serde(default)]
    results: Vec<SearchResult>,
}

#[derive(Deserialize, Serialize)]
struct SearchResult {
    title: String,
    url: String,
    content: String,
}

#[async_trait]
impl Tool for WebSearchTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "web_search".to_string(),
            description:
                "Search the web for up-to-date information. Returns a short list of results (title, url, content snippet)."
                    .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "What to search for" }
                },
                "required": ["query"]
            }),
        }
    }

    async fn call(&self, args: Value) -> anyhow::Result<Value> {
        let query = args
            .get("query")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("missing required 'query' argument"))?;

        let request = SearchRequest { api_key: &self.api_key, query, max_results: 5 };

        let response = self.client.post(API_URL).json(&request).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Tavily API error ({status}): {body}");
        }

        let parsed: SearchResponse = response.json().await?;
        Ok(json!({ "results": parsed.results }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn requires_query_argument() {
        let tool = WebSearchTool::new("fake-key");
        let err = tool.call(json!({})).await.unwrap_err();
        assert!(err.to_string().contains("query"));
    }
}
