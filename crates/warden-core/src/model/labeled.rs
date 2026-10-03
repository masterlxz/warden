//! A provider that knows the id the person gave it (P10). The spend ledger keeps that id with every call, so the
//! usage screens can show dollars per provider: the ledger can't recover it from the model alone, since two
//! providers can serve the same model and a combo can answer with any of its members.

use std::sync::Arc;

use async_trait::async_trait;

use super::key_check::KeyCheck;
use super::{ChatStream, Message, ModelProvider, Response, ToolSpec};

/// `inner` under the id `id` (a `[[providers]]` id): everything is passed on, only `provider_id` is added.
pub struct Labeled {
    id: String,
    inner: Arc<dyn ModelProvider>,
}

impl Labeled {
    pub fn wrap(id: impl Into<String>, inner: Arc<dyn ModelProvider>) -> Arc<dyn ModelProvider> {
        Arc::new(Self { id: id.into(), inner })
    }
}

#[async_trait]
impl ModelProvider for Labeled {
    fn model_id(&self) -> &str {
        self.inner.model_id()
    }

    fn provider_id(&self) -> &str {
        &self.id
    }

    /// A node's model is scoped to the agent asking (P93): the scoped copy keeps the label, or the ledger would
    /// lose it for every call made on behalf of an agent.
    fn for_agent(&self, agent: Option<&str>) -> Option<Arc<dyn ModelProvider>> {
        self.inner.for_agent(agent).map(|scoped| Labeled::wrap(self.id.clone(), scoped))
    }

    async fn check_key(&self) -> KeyCheck {
        self.inner.check_key().await
    }

    async fn chat_stream(&self, messages: Vec<Message>, tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
        self.inner.chat_stream(messages, tools).await
    }

    async fn chat(&self, messages: Vec<Message>, tools: Vec<ToolSpec>) -> anyhow::Result<Response> {
        self.inner.chat(messages, tools).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::response_stream;

    struct Plain;

    #[async_trait]
    impl ModelProvider for Plain {
        fn model_id(&self) -> &str {
            "gpt-x"
        }

        async fn chat_stream(&self, _messages: Vec<Message>, _tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
            Ok(response_stream(Response { content: "hi".into(), tool_calls: Vec::new(), usage: None }))
        }
    }

    /// A provider that is only usable by one agent, like a node's model: its scoped copy is a different object.
    struct PerAgent(Option<String>);

    #[async_trait]
    impl ModelProvider for PerAgent {
        fn for_agent(&self, agent: Option<&str>) -> Option<Arc<dyn ModelProvider>> {
            Some(Arc::new(Self(agent.map(str::to_string))))
        }

        async fn chat_stream(&self, _messages: Vec<Message>, _tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
            Ok(response_stream(Response { content: self.0.clone().unwrap_or_default(), tool_calls: Vec::new(), usage: None }))
        }
    }

    #[tokio::test]
    async fn the_label_is_added_and_everything_else_passes_through() {
        let labeled = Labeled::wrap("main", Arc::new(Plain));
        assert_eq!((labeled.provider_id(), labeled.model_id()), ("main", "gpt-x"));
        assert_eq!(Plain.provider_id(), "", "a provider nobody labelled says so");
        assert_eq!(labeled.chat(Vec::new(), Vec::new()).await.unwrap().content, "hi");
        assert!(labeled.for_agent(Some("writer")).is_none(), "nothing to scope, nothing to copy");
    }

    #[tokio::test]
    async fn a_copy_scoped_to_an_agent_keeps_the_label() {
        let labeled = Labeled::wrap("casa", Arc::new(PerAgent(None)));
        let scoped = labeled.for_agent(Some("writer")).expect("the inner provider scopes");
        assert_eq!(scoped.provider_id(), "casa");
        assert_eq!(scoped.chat(Vec::new(), Vec::new()).await.unwrap().content, "writer", "and it is the scoped copy that answers");
    }
}
