use std::sync::Arc;

use crate::memory::Vault;
use crate::model::{Message, ModelProvider, ToolCall};
use crate::tool::Tool;

/// Caps how many rounds of tool calls a single `handle_message` will chase before
/// giving up, so a model stuck requesting tools can't loop forever.
const MAX_TOOL_ITERATIONS: usize = 8;

/// Central coordinator: owns the model, the vault, and the registered tools.
/// Channels (CLI, Telegram, WhatsApp, ...) call `handle_message` and don't
/// know anything about which model or tools are behind it.
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

    pub fn vault(&self) -> &Arc<Vault> {
        &self.vault
    }

    /// `history` is the prior turns of this conversation (user/assistant pairs, oldest first),
    /// as tracked by the caller — the orchestrator itself is stateless across calls. Pass `&[]`
    /// for a fresh conversation or a one-off sub-agent task.
    pub async fn handle_message(&self, history: &[Message], user_input: &str) -> anyhow::Result<String> {
        let mut messages = Vec::new();

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
        messages.push(Message::user(user_input));

        let tool_specs = self.tools.iter().map(|t| t.spec()).collect::<Vec<_>>();

        for _ in 0..MAX_TOOL_ITERATIONS {
            let response = self.model.chat(messages.clone(), tool_specs.clone()).await?;

            if response.tool_calls.is_empty() {
                return Ok(response.content);
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
    use crate::model::{Response, Role};
    use crate::tool::ToolSpec;

    struct MockModel {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl ModelProvider for MockModel {
        async fn chat(&self, messages: Vec<Message>, _tools: Vec<ToolSpec>) -> anyhow::Result<Response> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            if call == 0 {
                Ok(Response {
                    content: String::new(),
                    tool_calls: vec![ToolCall { id: "call_1".to_string(), name: "echo".to_string(), arguments: json!({ "text": "hi" }) }],
                })
            } else {
                let last = messages.last().expect("tool result should have been appended");
                assert_eq!(last.role, Role::Tool);
                assert!(last.content.contains("hi"));
                Ok(Response { content: "done".to_string(), tool_calls: Vec::new() })
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
        assert_eq!(result, "done");
    }

    struct AlwaysToolCallModel;

    #[async_trait]
    impl ModelProvider for AlwaysToolCallModel {
        async fn chat(&self, _messages: Vec<Message>, _tools: Vec<ToolSpec>) -> anyhow::Result<Response> {
            Ok(Response {
                content: String::new(),
                tool_calls: vec![ToolCall { id: "call_x".to_string(), name: "echo".to_string(), arguments: json!({}) }],
            })
        }
    }

    #[tokio::test]
    async fn gives_up_after_max_iterations() {
        let mut orchestrator = Orchestrator::new(Arc::new(AlwaysToolCallModel), temp_vault());
        orchestrator.register_tool(Arc::new(EchoTool));

        let result = orchestrator.handle_message(&[], "loop forever").await;
        assert!(result.is_err());
    }
}
