use std::sync::Arc;

use crate::memory::Vault;
use crate::model::{Message, ModelProvider, Role};
use crate::tool::Tool;

/// Central coordinator: owns the model, the vault, and the registered tools.
/// Channels (CLI, Telegram, WhatsApp, ...) call `handle_message` and don't
/// know anything about which model or tools are behind it.
pub struct Orchestrator {
    model: Arc<dyn ModelProvider>,
    vault: Vault,
    tools: Vec<Arc<dyn Tool>>,
}

impl Orchestrator {
    pub fn new(model: Arc<dyn ModelProvider>, vault: Vault) -> Self {
        Self { model, vault, tools: Vec::new() }
    }

    pub fn register_tool(&mut self, tool: Arc<dyn Tool>) {
        self.tools.push(tool);
    }

    pub fn vault(&self) -> &Vault {
        &self.vault
    }

    pub async fn handle_message(&self, user_input: &str) -> anyhow::Result<String> {
        let mut messages = Vec::new();

        let hits = self.vault.search(user_input, 8).unwrap_or_default();
        if !hits.is_empty() {
            let context = hits
                .iter()
                .map(|h| format!("[{}:{}] {}", h.path, h.line_number, h.line.trim()))
                .collect::<Vec<_>>()
                .join("\n");
            messages.push(Message {
                role: Role::System,
                content: format!(
                    "Relevant context found in the user's memory vault (may or may not be relevant — use your judgment):\n{context}"
                ),
            });
        }

        messages.push(Message { role: Role::User, content: user_input.to_string() });

        let tool_specs = self.tools.iter().map(|t| t.spec()).collect();
        let response = self.model.chat(messages, tool_specs).await?;
        Ok(response.content)
    }
}
