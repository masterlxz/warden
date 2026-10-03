//! Falling back to another provider when the one a turn uses is down (P79). The alternative to
//! routing through an external gateway (9Router and the like), decided with the user in Sessão
//! 105: the reserves are providers already configured in Warden, in the order the user picked.
//!
//! Only an error that says "try again elsewhere" moves on: an HTTP 429 or 5xx, or the connection
//! failing or timing out. Anything else from the turn's own provider (a 400, a rejected key) comes
//! back as is, since another provider would only hide a problem the person needs to see. Once
//! falling back, a reserve that fails for any reason just hands over to the next one. It only
//! happens before the stream starts: once part of an answer has arrived, a dropped stream fails the
//! turn as it always did.

use std::sync::Arc;

use async_trait::async_trait;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};

use super::{ChatStream, Message, ModelProvider, ProviderHttpError, ProviderUnavailable, StreamEvent};
use crate::tool::ToolSpec;

/// One switch: `from` failed (`reason`), `to` answered with `model`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderFallback {
    pub from: String,
    pub to: String,
    /// What `to` sent the request to — the model the spend is recorded under.
    pub model: String,
    /// Short, e.g. "503 Service Unavailable" or "connection failed".
    pub reason: String,
}

/// The turn's provider first, then the reserves — each with the id the user knows it by.
pub struct FallbackProvider {
    providers: Vec<(String, Arc<dyn ModelProvider>)>,
}

impl FallbackProvider {
    /// `providers` must not be empty; the first is the one the turn is meant for.
    pub fn new(providers: Vec<(String, Arc<dyn ModelProvider>)>) -> Self {
        assert!(!providers.is_empty(), "FallbackProvider needs at least one provider");
        Self { providers }
    }
}

/// Whether `err` says the provider is busy or unreachable, not that the request is wrong.
pub fn is_transient(err: &anyhow::Error) -> bool {
    for cause in err.chain() {
        if let Some(http) = cause.downcast_ref::<ProviderHttpError>() {
            return http.status == 408 || http.status == 429 || http.status >= 500;
        }
        if let Some(net) = cause.downcast_ref::<reqwest::Error>() {
            return net.is_connect() || net.is_timeout();
        }
        if cause.downcast_ref::<ProviderUnavailable>().is_some() {
            return true;
        }
    }
    false
}

fn short_reason(err: &anyhow::Error) -> String {
    for cause in err.chain() {
        if let Some(http) = cause.downcast_ref::<ProviderHttpError>() {
            return http.reason.clone();
        }
        if let Some(net) = cause.downcast_ref::<reqwest::Error>() {
            return if net.is_timeout() { "timed out".to_string() } else { "connection failed".to_string() };
        }
        if let Some(unavailable) = cause.downcast_ref::<ProviderUnavailable>() {
            return unavailable.0.chars().take(80).collect();
        }
    }
    let text = err.to_string();
    text.chars().take(80).collect()
}

#[async_trait]
impl ModelProvider for FallbackProvider {
    /// The turn's own provider's — a call that fell back says which model answered in its
    /// `StreamEvent::ProviderFallback`.
    fn model_id(&self) -> &str {
        self.providers[0].1.model_id()
    }

    /// The first member's id: it answers unless a fallback says otherwise (`StreamEvent::ProviderFallback`
    /// names the one that took over), which is how the spend ledger learns who served a call (P10).
    fn provider_id(&self) -> &str {
        &self.providers[0].0
    }

    /// Passes the agent on to every provider in the chain (P93: a node's model in a combo).
    fn for_agent(&self, agent: Option<&str>) -> Option<Arc<dyn ModelProvider>> {
        let mut changed = false;
        let providers = self
            .providers
            .iter()
            .map(|(id, provider)| {
                let scoped = provider.for_agent(agent);
                changed |= scoped.is_some();
                (id.clone(), scoped.unwrap_or_else(|| provider.clone()))
            })
            .collect();
        changed.then(|| Arc::new(Self { providers }) as Arc<dyn ModelProvider>)
    }

    async fn chat_stream(&self, messages: Vec<Message>, tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
        let (first_id, first) = &self.providers[0];
        let first_err = match first.chat_stream(messages.clone(), tools.clone()).await {
            Ok(stream) => return Ok(stream),
            Err(err) if is_transient(&err) && self.providers.len() > 1 => err,
            Err(err) => return Err(err),
        };
        eprintln!("model: provider '{first_id}' failed ({}), trying the fallbacks", short_reason(&first_err));

        let mut others = Vec::new();
        for (id, provider) in &self.providers[1..] {
            match provider.chat_stream(messages.clone(), tools.clone()).await {
                Ok(stream) => {
                    let switch = ProviderFallback {
                        from: first_id.clone(),
                        to: id.clone(),
                        model: provider.model_id().to_string(),
                        reason: short_reason(&first_err),
                    };
                    let head = futures_util::stream::once(async move { Ok(StreamEvent::ProviderFallback(switch)) });
                    return Ok(Box::pin(head.chain(stream)));
                }
                Err(err) => {
                    eprintln!("model: fallback provider '{id}' failed too ({})", short_reason(&err));
                    others.push(format!("{id}: {}", short_reason(&err)));
                }
            }
        }
        Err(first_err.context(format!("'{first_id}' failed, and so did every fallback provider ({})", others.join("; "))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{drain_chat_stream, response_stream, Response, Role};
    use std::sync::atomic::{AtomicUsize, Ordering};

    enum Outcome {
        Reply(&'static str),
        Http(u16),
        Other,
    }

    struct Scripted {
        model: &'static str,
        outcome: Outcome,
        calls: AtomicUsize,
    }

    fn scripted(model: &'static str, outcome: Outcome) -> Arc<Scripted> {
        Arc::new(Scripted { model, outcome, calls: AtomicUsize::new(0) })
    }

    #[async_trait]
    impl ModelProvider for Scripted {
        fn model_id(&self) -> &str {
            self.model
        }

        async fn chat_stream(&self, _messages: Vec<Message>, _tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            match self.outcome {
                Outcome::Reply(text) => Ok(response_stream(Response { content: text.to_string(), tool_calls: Vec::new(), usage: None })),
                Outcome::Http(status) => {
                    Err(ProviderHttpError { provider: "Test", status, reason: format!("{status} Status"), body: "busy".to_string() }.into())
                }
                Outcome::Other => anyhow::bail!("malformed request"),
            }
        }
    }

    fn chain(providers: Vec<(&str, Arc<Scripted>)>) -> FallbackProvider {
        FallbackProvider::new(providers.into_iter().map(|(id, p)| (id.to_string(), p as Arc<dyn ModelProvider>)).collect())
    }

    async fn run(provider: &FallbackProvider) -> anyhow::Result<(String, Vec<StreamEvent>)> {
        let mut events = Vec::new();
        let stream = provider.chat_stream(vec![Message::user("hi")], Vec::new()).await?;
        let response = drain_chat_stream(stream, |e| events.push(e.clone())).await?;
        Ok((response.content, events))
    }

    /// Answers only for the agent it was scoped to; unscoped, or for anyone else, it's unreachable —
    /// how a node's model behaves for an agent off the node's list (P93).
    struct OnlyFor {
        allowed: &'static str,
        agent: Option<String>,
    }

    #[async_trait]
    impl ModelProvider for OnlyFor {
        fn for_agent(&self, agent: Option<&str>) -> Option<Arc<dyn ModelProvider>> {
            Some(Arc::new(Self { allowed: self.allowed, agent: agent.map(str::to_string) }))
        }

        async fn chat_stream(&self, _messages: Vec<Message>, _tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
            if self.agent.as_deref() == Some(self.allowed) {
                Ok(response_stream(Response { content: "from the node".into(), tool_calls: Vec::new(), usage: None }))
            } else {
                Err(ProviderUnavailable("node 'casa' isn't open to this agent".into()).into())
            }
        }
    }

    #[tokio::test]
    async fn an_unreachable_provider_falls_back_like_a_503_and_the_agent_reaches_every_link() {
        let node: Arc<dyn ModelProvider> = Arc::new(OnlyFor { allowed: "ops", agent: None });
        let combo = FallbackProvider::new(vec![("casa".into(), node), ("gemini".into(), scripted("g", Outcome::Reply("from gemini")) as Arc<dyn ModelProvider>)]);

        let as_ops = combo.for_agent(Some("ops")).expect("the node link cares about the agent");
        let stream = as_ops.chat_stream(vec![Message::user("hi")], Vec::new()).await.unwrap();
        assert_eq!(drain_chat_stream(stream, |_| {}).await.unwrap().content, "from the node");

        let as_other = combo.for_agent(Some("other")).unwrap();
        let mut events = Vec::new();
        let stream = as_other.chat_stream(vec![Message::user("hi")], Vec::new()).await.unwrap();
        let reply = drain_chat_stream(stream, |e| events.push(e.clone())).await.unwrap();
        assert_eq!(reply.content, "from gemini");
        let switched = switches(&events);
        assert_eq!((switched[0].from.as_str(), switched[0].to.as_str()), ("casa", "gemini"));
        assert!(switched[0].reason.contains("isn't open"), "{}", switched[0].reason);

        // A chain where nothing cares about the agent stays as it is.
        assert!(chain(vec![("a", scripted("a", Outcome::Reply("x")))]).for_agent(Some("ops")).is_none());
    }

    #[test]
    fn messages_and_events_cross_the_wire_unchanged() {
        let message = Message {
            role: Role::Assistant,
            content: "calling".into(),
            tool_calls: vec![crate::model::ToolCall { id: "c1".into(), name: "shell".into(), arguments: serde_json::json!({ "command": "ls" }), thought_signature: None }],
            tool_call_id: None,
            tool_name: None,
            attachments: Vec::new(),
        };
        let back: Message = serde_json::from_str(&serde_json::to_string(&message).unwrap()).unwrap();
        assert_eq!((back.role, back.tool_calls[0].name.as_str(), back.tool_calls[0].arguments.clone()), (Role::Assistant, "shell", serde_json::json!({ "command": "ls" })));

        for event in [
            StreamEvent::ContentDelta("hel".into()),
            StreamEvent::ToolCallDelta { index: 0, id: Some("c1".into()), name: Some("shell".into()), arguments_delta: Some("{}".into()), thought_signature: None },
            StreamEvent::Usage(crate::model::Usage { prompt_tokens: 3, completion_tokens: 4, total_tokens: 7 }),
        ] {
            let text = serde_json::to_string(&event).unwrap();
            let back: StreamEvent = serde_json::from_str(&text).unwrap();
            assert_eq!(format!("{back:?}"), format!("{event:?}"), "{text}");
        }
    }

    fn switches(events: &[StreamEvent]) -> Vec<ProviderFallback> {
        events.iter().filter_map(|e| if let StreamEvent::ProviderFallback(f) = e { Some(f.clone()) } else { None }).collect()
    }

    #[tokio::test]
    async fn a_working_provider_answers_without_touching_the_reserves() {
        let reserve = scripted("r", Outcome::Reply("reserve"));
        let provider = chain(vec![("main", scripted("m", Outcome::Reply("main"))), ("spare", reserve.clone())]);
        let (content, events) = run(&provider).await.unwrap();
        assert_eq!(content, "main");
        assert!(switches(&events).is_empty());
        assert_eq!(reserve.calls.load(Ordering::SeqCst), 0);
        assert_eq!(provider.model_id(), "m");
    }

    #[tokio::test]
    async fn a_busy_provider_hands_over_and_says_so() {
        for status in [503, 429, 529, 500] {
            let provider = chain(vec![("main", scripted("m", Outcome::Http(status))), ("spare", scripted("r", Outcome::Reply("reserve")))]);
            let (content, events) = run(&provider).await.unwrap();
            assert_eq!(content, "reserve", "{status}");
            assert_eq!(
                switches(&events),
                vec![ProviderFallback { from: "main".into(), to: "spare".into(), model: "r".into(), reason: format!("{status} Status") }]
            );
        }
    }

    #[tokio::test]
    async fn a_request_that_would_fail_anywhere_is_not_retried() {
        for outcome in [Outcome::Http(400), Outcome::Http(401), Outcome::Other] {
            let reserve = scripted("r", Outcome::Reply("reserve"));
            let provider = chain(vec![("main", scripted("m", outcome)), ("spare", reserve.clone())]);
            assert!(run(&provider).await.is_err());
            assert_eq!(reserve.calls.load(Ordering::SeqCst), 0);
        }
    }

    #[tokio::test]
    async fn a_broken_reserve_passes_to_the_next_and_all_failing_keeps_the_first_error() {
        let provider = chain(vec![
            ("main", scripted("m", Outcome::Http(503))),
            ("bad-key", scripted("b", Outcome::Http(401))),
            ("local", scripted("l", Outcome::Reply("local"))),
        ]);
        let (content, events) = run(&provider).await.unwrap();
        assert_eq!(content, "local");
        assert_eq!(switches(&events)[0].to, "local");

        let provider = chain(vec![("main", scripted("m", Outcome::Http(503))), ("spare", scripted("r", Outcome::Http(502)))]);
        let err = format!("{:#}", run(&provider).await.unwrap_err());
        assert!(err.contains("every fallback provider") && err.contains("spare: 502") && err.contains("503"), "{err}");
    }

    #[tokio::test]
    async fn a_connection_failure_hands_over() {
        // A real refused connection, so the error is a real `reqwest::Error`.
        let refused = reqwest::Client::new().get("http://127.0.0.1:1/").send().await.unwrap_err();
        let err = anyhow::Error::from(refused);
        assert!(is_transient(&err));
        assert_eq!(short_reason(&err), "connection failed");
    }

    #[tokio::test]
    async fn a_single_provider_is_passed_through() {
        let provider = chain(vec![("main", scripted("m", Outcome::Http(503)))]);
        assert!(format!("{:#}", run(&provider).await.unwrap_err()).contains("503 Status"));
    }
}
