use async_trait::async_trait;
use serde_json::{json, Value};

use crate::orchestrator::Orchestrator;
use crate::tool::{Tool, ToolSpec};

/// Delegates a scoped, self-contained task to a fresh sub-agent — its own `Orchestrator`
/// instance (same model, same vault, a caller-chosen subset of tools). Originally the
/// "invocação leve" v1 mechanism from ARCHITECTURE.md's "Sub-agentes" section (single level,
/// no recursion); P46 extended it to support **bounded recursive delegation** — the
/// `orchestrator` passed to `new` may itself have its own `DelegateTool` registered, wrapping a
/// further sub-orchestrator, and so on. The stopping criterion isn't a runtime depth check here:
/// it's structural — whoever builds the chain (`warden-bootstrap::build_delegating_orchestrator`)
/// stops registering a `DelegateTool` past a fixed depth, so the terminal orchestrator simply
/// never advertises `delegate_task` in its tool specs and a model calling it has nothing further
/// to delegate to. Still no job queue, no persistence, no cost control, no tool isolation beyond
/// the caller-chosen subset — those remain out of scope (see `PENDING.md` P46/P60).
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

        // Sub-agent's token usage is dropped here, not rolled up into the parent conversation's
        // total — Tool::call only returns serde_json::Value, not a MessageOutcome.
        let result = self.orchestrator.handle_message(&[], task).await?;
        Ok(json!({ "result": result.content }))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use async_trait::async_trait;
    use serde_json::json;

    use super::*;
    use crate::memory::Vault;
    use crate::model::{ChatStream, Message, ModelProvider, Response, ToolCall, response_stream};

    struct FixedAnswerModel {
        answer: String,
    }

    #[async_trait]
    impl ModelProvider for FixedAnswerModel {
        async fn chat_stream(&self, _messages: Vec<Message>, _tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
            Ok(response_stream(Response { content: self.answer.clone(), tool_calls: Vec::new(), usage: None }))
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

    /// Proves P46's core claim end-to-end: a chain of orchestrators three deep (root → level-1 →
    /// leaf), each wrapping the next via `DelegateTool`, actually lets a sub-agent delegate
    /// further — not just the root. Scripted purely by call order since all three orchestrators
    /// share one model instance (mirrors `warden-bootstrap`'s real wiring: same `model`/`vault`
    /// cloned at every depth) and each `chat_stream` call blocks on any nested delegation before
    /// the next one happens, so the sequence below is deterministic. Also asserts the actual
    /// stopping criterion: the leaf orchestrator's tool specs never include `delegate_task`.
    #[tokio::test]
    async fn supports_bounded_recursive_delegation() {
        struct ScriptedModel {
            call_count: AtomicUsize,
        }

        #[async_trait]
        impl ModelProvider for ScriptedModel {
            async fn chat_stream(&self, _messages: Vec<Message>, tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
                let has_delegate = tools.iter().any(|t| t.name == "delegate_task");
                let response = match self.call_count.fetch_add(1, Ordering::SeqCst) {
                    0 => {
                        assert!(has_delegate, "root must be offered delegate_task");
                        Response {
                            content: String::new(),
                            tool_calls: vec![ToolCall {
                                id: "1".to_string(),
                                name: "delegate_task".to_string(),
                                arguments: json!({ "task": "level 2" }),
                                thought_signature: None,
                            }],
                            usage: None,
                        }
                    }
                    1 => {
                        assert!(has_delegate, "level-1 sub-agent must still be offered delegate_task");
                        Response {
                            content: String::new(),
                            tool_calls: vec![ToolCall {
                                id: "2".to_string(),
                                name: "delegate_task".to_string(),
                                arguments: json!({ "task": "level 3" }),
                                thought_signature: None,
                            }],
                            usage: None,
                        }
                    }
                    2 => {
                        assert!(!has_delegate, "leaf orchestrator must NOT be offered delegate_task");
                        Response { content: "leaf answer".to_string(), tool_calls: Vec::new(), usage: None }
                    }
                    3 => Response { content: "level-1 wraps up".to_string(), tool_calls: Vec::new(), usage: None },
                    4 => Response { content: "root wraps up".to_string(), tool_calls: Vec::new(), usage: None },
                    n => panic!("unexpected extra model call {n}"),
                };
                Ok(response_stream(response))
            }
        }

        let model = Arc::new(ScriptedModel { call_count: AtomicUsize::new(0) });
        let vault = temp_vault();

        let leaf = Orchestrator::new(model.clone(), vault.clone());

        let mut level1 = Orchestrator::new(model.clone(), vault.clone());
        level1.register_tool(Arc::new(DelegateTool::new(leaf)));

        let mut root = Orchestrator::new(model.clone(), vault.clone());
        root.register_tool(Arc::new(DelegateTool::new(level1)));

        let outcome = root.handle_message(&[], "level 1").await.unwrap();
        assert_eq!(outcome.content, "root wraps up");
        assert_eq!(model.call_count.load(Ordering::SeqCst), 5);
    }
}
