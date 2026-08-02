use async_trait::async_trait;
use serde_json::{json, Value};

use crate::orchestrator::Orchestrator;
use crate::tool::{Tool, ToolSpec};

/// Delegates a scoped, self-contained task to a fresh sub-agent — its own `Orchestrator`
/// instance (same model, same vault, a caller-chosen subset of tools). This is the
/// "invocação leve" v1 mechanism from ARCHITECTURE.md's "Sub-agentes" section: the sub-agent
/// has no visibility into the parent conversation, runs the existing tool-calling loop to
/// completion on its own, and returns a single final answer. No job queue, no persistence,
/// no autonomous recursion — the `orchestrator` passed to `new` must never itself have a
/// `DelegateTool` registered, or this would allow unbounded recursive sub-agent spawning
/// (explicitly out of v1 scope per ARCHITECTURE.md).
pub struct DelegateTool {
    orchestrator: Orchestrator,
}

impl DelegateTool {
    pub fn new(orchestrator: Orchestrator) -> Self {
        Self { orchestrator }
    }
}

#[async_trait]
impl Tool for DelegateTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "delegate_task".to_string(),
            description: "Delegate a scoped, self-contained task to a fresh sub-agent that runs \
                independently and reports back only its final answer. Use this to offload a \
                bounded chunk of work (e.g. summarizing a file, researching a topic, drafting a \
                note) so its intermediate tool calls don't clutter the current conversation. \
                IMPORTANT: the sub-agent does NOT see this conversation's history — the 'task' \
                argument must be a fully self-contained description including any facts, file \
                paths, or constraints it needs. The sub-agent cannot ask you follow-up questions \
                and cannot itself delegate further; it runs to completion and returns one result."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "task": {
                        "type": "string",
                        "description": "A complete, self-contained description of the task for \
                            the sub-agent to perform. Include everything it needs to know, since \
                            it starts with no memory of the current conversation."
                    }
                },
                "required": ["task"]
            }),
        }
    }

    async fn call(&self, args: Value) -> anyhow::Result<Value> {
        let task = args
            .get("task")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("missing required 'task' argument"))?;

        let result = self.orchestrator.handle_message(&[], task).await?;
        Ok(json!({ "result": result }))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use serde_json::json;

    use super::*;
    use crate::memory::Vault;
    use crate::model::{Message, ModelProvider, Response};

    struct FixedAnswerModel {
        answer: String,
    }

    #[async_trait]
    impl ModelProvider for FixedAnswerModel {
        async fn chat(&self, _messages: Vec<Message>, _tools: Vec<ToolSpec>) -> anyhow::Result<Response> {
            Ok(Response { content: self.answer.clone(), tool_calls: Vec::new() })
        }
    }

    fn temp_vault() -> Arc<Vault> {
        Arc::new(Vault::new(std::env::temp_dir().join(format!(
            "warden-delegate-test-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ))))
    }

    #[tokio::test]
    async fn requires_task_argument() {
        let model = Arc::new(FixedAnswerModel { answer: String::new() });
        let tool = DelegateTool::new(Orchestrator::new(model, temp_vault()));

        let err = tool.call(json!({})).await.unwrap_err();
        assert!(err.to_string().contains("task"));
    }

    #[tokio::test]
    async fn delegates_and_returns_sub_agent_final_answer() {
        let model = Arc::new(FixedAnswerModel { answer: "sub-agent done".to_string() });
        let tool = DelegateTool::new(Orchestrator::new(model, temp_vault()));

        let result = tool.call(json!({ "task": "summarize X" })).await.unwrap();
        assert_eq!(result, json!({ "result": "sub-agent done" }));
    }
}
