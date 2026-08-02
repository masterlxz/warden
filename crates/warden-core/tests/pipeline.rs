//! Integration tests for the full agent pipeline (Phase 1.9): vault + tools + orchestrator
//! wired together the same way `warden-cli`'s `main.rs` assembles them, exercised only
//! through `warden-core`'s public API — no real model API calls, so this stays hermetic
//! and fast. Individual pieces (each tool, the orchestrator's tool-calling loop in isolation)
//! already have their own unit tests; this file's job is to catch wiring mistakes that only
//! show up when everything runs together.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;

use warden_core::memory::Vault;
use warden_core::model::{Message, ModelProvider, Response, Role, ToolCall};
use warden_core::orchestrator::Orchestrator;
use warden_core::tool::delegate::DelegateTool;
use warden_core::tool::file_tools::{ReadFileTool, WriteFileTool};
use warden_core::tool::{Tool, ToolSpec};

fn temp_vault(name: &str) -> Arc<Vault> {
    Arc::new(Vault::new(std::env::temp_dir().join(format!(
        "warden-pipeline-test-{name}-{}",
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ))))
}

/// Scripts a fixed sequence of responses, one per call to `chat`, and lets each step
/// assert on the message history it was handed before answering.
struct ScriptedModel<F> {
    calls: AtomicUsize,
    step: F,
}

#[async_trait]
impl<F> ModelProvider for ScriptedModel<F>
where
    F: Fn(usize, &[Message]) -> Response + Send + Sync,
{
    async fn chat(&self, messages: Vec<Message>, _tools: Vec<ToolSpec>) -> anyhow::Result<Response> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        Ok((self.step)(call, &messages))
    }
}

#[tokio::test]
async fn vault_context_and_read_file_tool_round_trip() {
    let vault = temp_vault("read-file");
    vault.write("notes/dentist.md", "Dentist appointment on Friday at 3pm").unwrap();

    let model = Arc::new(ScriptedModel {
        calls: AtomicUsize::new(0),
        step: |call, messages: &[Message]| match call {
            0 => {
                let has_vault_context = messages
                    .iter()
                    .any(|m| m.role == Role::System && m.content.contains("Dentist appointment"));
                assert!(has_vault_context, "expected vault search hit injected as system context");

                Response {
                    content: String::new(),
                    tool_calls: vec![ToolCall {
                        id: "call_1".to_string(),
                        name: "read_file".to_string(),
                        arguments: json!({ "path": "notes/dentist.md" }),
                    }],
                }
            }
            1 => {
                let last = messages.last().unwrap();
                assert_eq!(last.role, Role::Tool);
                assert!(last.content.contains("Dentist appointment on Friday at 3pm"));

                Response { content: "Your dentist appointment is Friday at 3pm.".to_string(), tool_calls: Vec::new() }
            }
            other => panic!("unexpected extra call to the model: {other}"),
        },
    });

    let mut orchestrator = Orchestrator::new(model, vault.clone());
    orchestrator.register_tool(Arc::new(ReadFileTool::new(vault.clone())));
    orchestrator.register_tool(Arc::new(WriteFileTool::new(vault)));

    let result = orchestrator.handle_message("When is my dentist appointment?").await.unwrap();
    assert_eq!(result, "Your dentist appointment is Friday at 3pm.");
}

#[tokio::test]
async fn delegate_task_round_trip_through_full_wiring() {
    let vault = temp_vault("delegate");

    let model = Arc::new(ScriptedModel {
        calls: AtomicUsize::new(0),
        step: |call, messages: &[Message]| match call {
            0 => Response {
                content: String::new(),
                tool_calls: vec![ToolCall {
                    id: "call_1".to_string(),
                    name: "delegate_task".to_string(),
                    arguments: json!({ "task": "say hi" }),
                }],
            },
            1 => Response { content: "sub says hi".to_string(), tool_calls: Vec::new() },
            2 => {
                let last = messages.last().unwrap();
                assert_eq!(last.role, Role::Tool);
                assert!(last.content.contains("sub says hi"));

                Response { content: "delegation complete".to_string(), tool_calls: Vec::new() }
            }
            other => panic!("unexpected extra call to the model: {other}"),
        },
    });

    let base_tools: Vec<Arc<dyn Tool>> =
        vec![Arc::new(ReadFileTool::new(vault.clone())), Arc::new(WriteFileTool::new(vault.clone()))];

    let mut sub_orchestrator = Orchestrator::new(model.clone(), vault.clone());
    for tool in &base_tools {
        sub_orchestrator.register_tool(tool.clone());
    }

    let mut orchestrator = Orchestrator::new(model, vault);
    for tool in base_tools {
        orchestrator.register_tool(tool);
    }
    orchestrator.register_tool(Arc::new(DelegateTool::new(sub_orchestrator)));

    let result = orchestrator.handle_message("please delegate").await.unwrap();
    assert_eq!(result, "delegation complete");
}
