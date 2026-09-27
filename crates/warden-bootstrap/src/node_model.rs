//! A node's own model as a provider on the hub (P93, fatia 3): `[[providers]]` with `kind = "node"`,
//! the node's device id and the id of the provider *on the node* ("casa-llama" = the Ollama of the
//! PC at home). It's picked like any provider — active, an agent's default, a combo member.
//!
//! The provider itself can't reach a node: only a hub knows who's connected. So it asks the
//! process's router, which a hub installs when it starts serving (`set_node_model_router`). Without
//! one — the CLI, the desktop with its hub off — or with the node out of reach, the call fails with
//! `ProviderUnavailable`, which a combo treats like a 503 and moves on.

use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use warden_core::model::{ChatStream, Message, ModelProvider, ProviderUnavailable};
use warden_core::tool::ToolSpec;

/// Sends one model call to a node and streams its answer back — implemented by the hub.
#[async_trait]
pub trait NodeModelRouter: Send + Sync {
    /// `agent`: who is asking, checked against the node's allowed agents.
    async fn chat_stream(&self, node: &str, model: &str, agent: Option<&str>, messages: Vec<Message>, tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream>;
}

static ROUTER: RwLock<Option<Arc<dyn NodeModelRouter>>> = RwLock::new(None);

/// Installs (or, with `None`, removes) this process's router — the hub does it when it starts.
pub fn set_node_model_router(router: Option<Arc<dyn NodeModelRouter>>) {
    *ROUTER.write().unwrap_or_else(|e| e.into_inner()) = router;
}

fn router() -> Option<Arc<dyn NodeModelRouter>> {
    ROUTER.read().unwrap_or_else(|e| e.into_inner()).clone()
}

#[derive(Clone)]
pub struct NodeModelProvider {
    node: String,
    model: String,
    agent: Option<String>,
}

impl NodeModelProvider {
    pub fn new(node: impl Into<String>, model: impl Into<String>) -> Self {
        Self { node: node.into(), model: model.into(), agent: None }
    }
}

#[async_trait]
impl ModelProvider for NodeModelProvider {
    fn model_id(&self) -> &str {
        &self.model
    }

    fn for_agent(&self, agent: Option<&str>) -> Option<Arc<dyn ModelProvider>> {
        Some(Arc::new(Self { agent: agent.map(str::to_string), ..self.clone() }))
    }

    async fn chat_stream(&self, messages: Vec<Message>, tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
        let Some(router) = router() else {
            return Err(ProviderUnavailable(format!("the model on node '{}' only answers through a running hub", self.node)).into());
        };
        router.chat_stream(&self.node, &self.model, self.agent.as_deref(), messages, tools).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn without_a_hub_it_is_unavailable_so_a_combo_moves_on() {
        set_node_model_router(None);
        let err = match NodeModelProvider::new("node-casa", "llama").chat_stream(vec![Message::user("hi")], Vec::new()).await {
            Err(err) => err,
            Ok(_) => panic!("no router, no answer"),
        };
        assert!(err.downcast_ref::<ProviderUnavailable>().is_some(), "{err:#}");
        assert!(format!("{err:#}").contains("node-casa"));
    }
}
