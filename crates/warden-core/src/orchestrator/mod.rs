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
        let messages = vec![Message { role: Role::User, content: user_input.to_string() }];
        let tool_specs = self.tools.iter().map(|t| t.spec()).collect();
        let response = self.model.chat(messages, tool_specs).await?;
        Ok(response.content)
    }
}
