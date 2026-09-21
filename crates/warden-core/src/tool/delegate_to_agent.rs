use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::budget::TurnBudget;
use crate::orchestrator::Orchestrator;
use crate::tool::{Tool, ToolSpec};

/// One addressable target for `DelegateToAgentTool` (P46's opt-in "chief" mechanism) — a
/// specific configured agent, not an anonymous scoped sub-agent like `DelegateTool`'s
/// `delegate_task`. `warden-core` stays agnostic of `AgentConfig`/`config.toml`: the caller
/// (`warden_bootstrap::build_delegate_to_agent_tool`) resolves persona/provider ahead of time and
/// hands over a ready-to-use list.
#[derive(Clone)]
pub struct NamedSubAgent {
    /// Matches the configured agent's id — what the model must pass as `agent_id`.
    pub id: String,
    /// Shown in the tool spec's description so the calling model knows what this agent is for.
    /// Free text — callers typically pass the agent's persona as-is.
    pub description: String,
    /// Already carries this agent's own model (via `with_model`, if its `provider_id` differs
    /// from the caller's) — everything else (base tools, delegation depth) is inherited from
    /// whichever orchestrator this was built from.
    pub orchestrator: Orchestrator,
    /// Passed to `handle_turn` as the system prompt, same as any other named-agent turn.
    pub persona: Option<String>,
}

/// Bumped whenever the set of configured agents changes mid-turn (`manage_agents` creating,
/// editing or deleting one), so a `DelegateToAgentTool` built earlier in the same turn knows its
/// target list went stale. A plain counter shared by `Arc`: whoever changes agents calls `bump`,
/// the tool compares it against the value it last saw.
#[derive(Clone, Default)]
pub struct AgentsRevision(Arc<AtomicU64>);

impl AgentsRevision {
    pub fn bump(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }

    pub fn current(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

/// Rebuilds the target list from whatever is configured *now*. `None` (config unreadable, or
/// nothing resolvable) keeps the previous list — a hiccup must not take delegation away mid-turn.
pub type AgentResolver = Arc<dyn Fn() -> Option<Vec<NamedSubAgent>> + Send + Sync>;

#[derive(Clone)]
struct LiveAgents {
    revision: AgentsRevision,
    resolver: AgentResolver,
}

struct Targets {
    /// The `AgentsRevision` value these `agents` were resolved at.
    seen: u64,
    agents: Arc<Vec<NamedSubAgent>>,
}

/// Delegates a scoped, self-contained task to one specific *configured* agent, addressed by id —
/// unlike `DelegateTool`, which spins up an anonymous helper with no persona of its own. Only
/// attached to a turn whose active agent has opted in (`AgentConfig.can_delegate_to_agents`); see
/// `warden_bootstrap::build_delegate_to_agent_tool` and `Orchestrator::with_tool`.
///
/// Built with `live`, the target list is refreshed when the shared `AgentsRevision` moves — that is
/// how an agent created by `manage_agents` earlier in the *same turn* becomes a valid `agent_id`
/// right away instead of from the next turn.
pub struct DelegateToAgentTool {
    targets: Mutex<Targets>,
    live: Option<LiveAgents>,
    budget: Option<Arc<TurnBudget>>,
}

impl DelegateToAgentTool {
    pub fn new(agents: Vec<NamedSubAgent>) -> Self {
        Self { targets: Mutex::new(Targets { seen: 0, agents: Arc::new(agents) }), live: None, budget: None }
    }

    /// Like `new`, but re-resolves the targets through `resolver` whenever `revision` has moved
    /// since the last look.
    pub fn live(agents: Vec<NamedSubAgent>, revision: AgentsRevision, resolver: AgentResolver) -> Self {
        let seen = revision.current();
        Self { targets: Mutex::new(Targets { seen, agents: Arc::new(agents) }), live: Some(LiveAgents { revision, resolver }), budget: None }
    }

    /// The targets to use right now, re-resolved first if agents changed since the last look. Held
    /// only briefly and never across an `.await`.
    fn current(&self) -> Arc<Vec<NamedSubAgent>> {
        let mut targets = self.targets.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(live) = &self.live {
            let revision = live.revision.current();
            if revision != targets.seen {
                targets.seen = revision;
                if let Some(mut fresh) = (live.resolver)().filter(|list| !list.is_empty()) {
                    if let Some(budget) = &self.budget {
                        for agent in &mut fresh {
                            agent.orchestrator = agent.orchestrator.charged_to(budget.clone());
                        }
                    }
                    targets.agents = Arc::new(fresh);
                }
            }
        }
        targets.agents.clone()
    }
}

#[async_trait]
impl Tool for DelegateToAgentTool {
    fn spec(&self) -> ToolSpec {
        let agents = self.current();
        let listing = agents.iter().map(|a| format!("- {}: {}", a.id, a.description)).collect::<Vec<_>>().join("\n");
        ToolSpec {
            name: "delegate_to_agent".to_string(),
            description: format!(
                "Delegate a scoped, self-contained task to one of your specific configured \
                 sub-agents, addressed by id — each has its own persona/model, unlike \
                 delegate_task's anonymous helper. IMPORTANT: same as delegate_task, the \
                 sub-agent does NOT see this conversation's history and cannot ask follow-up \
                 questions — the 'task' argument must be a complete, self-contained description. \
                 Available agents:\n{listing}"
            ),
            parameters: json!({
                "type": "object",
                "properties": {
                    "agent_id": {
                        "type": "string",
                        "enum": agents.iter().map(|a| a.id.clone()).collect::<Vec<_>>(),
                        "description": "Which configured agent to delegate to."
                    },
                    "task": {
                        "type": "string",
                        "description": "A complete, self-contained description of the task for \
                            the chosen agent to perform. Include everything it needs to know, \
                            since it starts with no memory of the current conversation."
                    }
                },
                "required": ["agent_id", "task"]
            }),
        }
    }

    fn with_budget(&self, budget: &Arc<TurnBudget>) -> Option<Arc<dyn Tool>> {
        let agents = self.current().iter().map(|a| NamedSubAgent { orchestrator: a.orchestrator.charged_to(budget.clone()), ..a.clone() }).collect();
        let seen = self.targets.lock().unwrap_or_else(|e| e.into_inner()).seen;
        Some(Arc::new(Self { targets: Mutex::new(Targets { seen, agents: Arc::new(agents) }), live: self.live.clone(), budget: Some(budget.clone()) }))
    }

    async fn call(&self, args: Value) -> anyhow::Result<Value> {
        let agent_id = args.get("agent_id").and_then(Value::as_str).ok_or_else(|| anyhow::anyhow!("missing required 'agent_id' argument"))?;
        let task = args.get("task").and_then(Value::as_str).ok_or_else(|| anyhow::anyhow!("missing required 'task' argument"))?;

        let agents = self.current();
        let agent = agents.iter().find(|a| a.id == agent_id).ok_or_else(|| {
            let available = agents.iter().map(|a| a.id.as_str()).collect::<Vec<_>>().join(", ");
            anyhow::anyhow!("unknown agent_id '{agent_id}' — must be one of: {available}")
        })?;

        let result = agent.orchestrator.handle_turn(&[], task, Vec::new(), agent.persona.as_deref()).await?;
        Ok(json!({ "result": result.content }))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use serde_json::json;

    use super::*;
    use crate::memory::Vault;
    use crate::model::{ChatStream, Message, ModelProvider, Response, Role, response_stream};

    fn temp_vault() -> Arc<Vault> {
        Arc::new(Vault::new(std::env::temp_dir().join(format!(
            "warden-delegate-to-agent-test-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ))))
    }

    struct FixedAnswerModel {
        answer: String,
    }

    #[async_trait]
    impl ModelProvider for FixedAnswerModel {
        async fn chat_stream(&self, _messages: Vec<Message>, _tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
            Ok(response_stream(Response { content: self.answer.clone(), tool_calls: Vec::new(), usage: None }))
        }
    }

    fn agent(id: &str, answer: &str, persona: Option<&str>) -> NamedSubAgent {
        let model = Arc::new(FixedAnswerModel { answer: answer.to_string() });
        NamedSubAgent {
            id: id.to_string(),
            description: persona.unwrap_or("").to_string(),
            orchestrator: Orchestrator::new(model, temp_vault()),
            persona: persona.map(str::to_string),
        }
    }

    #[tokio::test]
    async fn requires_agent_id_and_task_arguments() {
        let tool = DelegateToAgentTool::new(vec![agent("pirate", "arr", None)]);

        let err = tool.call(json!({ "task": "do something" })).await.unwrap_err();
        assert!(err.to_string().contains("agent_id"));

        let err = tool.call(json!({ "agent_id": "pirate" })).await.unwrap_err();
        assert!(err.to_string().contains("task"));
    }

    #[tokio::test]
    async fn dispatches_to_the_named_agent_by_id() {
        let tool = DelegateToAgentTool::new(vec![agent("pirate", "arr matey", None), agent("robot", "beep boop", None)]);

        let result = tool.call(json!({ "agent_id": "robot", "task": "say hi" })).await.unwrap();
        assert_eq!(result, json!({ "result": "beep boop" }));

        let result = tool.call(json!({ "agent_id": "pirate", "task": "say hi" })).await.unwrap();
        assert_eq!(result, json!({ "result": "arr matey" }));
    }

    #[tokio::test]
    async fn errors_clearly_on_unknown_agent_id() {
        let tool = DelegateToAgentTool::new(vec![agent("pirate", "arr", None)]);

        let err = tool.call(json!({ "agent_id": "ghost", "task": "boo" })).await.unwrap_err();
        assert!(err.to_string().contains("ghost"));
        assert!(err.to_string().contains("pirate"));
    }

    #[tokio::test]
    async fn the_target_agents_persona_is_sent_as_a_system_message() {
        struct EchoesWhetherPersonaWasSeen;

        #[async_trait]
        impl ModelProvider for EchoesWhetherPersonaWasSeen {
            async fn chat_stream(&self, messages: Vec<Message>, _tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
                let saw_persona = messages.iter().any(|m| m.role == Role::System && m.content.contains("PERSONA_MARKER"));
                Ok(response_stream(Response { content: format!("saw persona: {saw_persona}"), tool_calls: Vec::new(), usage: None }))
            }
        }

        let orchestrator = Orchestrator::new(Arc::new(EchoesWhetherPersonaWasSeen), temp_vault());
        let tool = DelegateToAgentTool::new(vec![NamedSubAgent {
            id: "pirate".to_string(),
            description: "You are a pirate. PERSONA_MARKER".to_string(),
            orchestrator,
            persona: Some("You are a pirate. PERSONA_MARKER".to_string()),
        }]);

        let result = tool.call(json!({ "agent_id": "pirate", "task": "say hi" })).await.unwrap();
        assert_eq!(result, json!({ "result": "saw persona: true" }));
    }

    #[test]
    fn spec_lists_every_agent_id_and_description() {
        let tool = DelegateToAgentTool::new(vec![agent("pirate", "arr", Some("A pirate.")), agent("robot", "beep", Some("A robot."))]);
        let spec = tool.spec();
        assert!(spec.description.contains("pirate: A pirate."));
        assert!(spec.description.contains("robot: A robot."));
        assert_eq!(spec.parameters["properties"]["agent_id"]["enum"], json!(["pirate", "robot"]));
    }

    #[test]
    fn a_live_tool_picks_up_an_agent_added_after_it_was_built() {
        let revision = AgentsRevision::default();
        let resolver: AgentResolver = Arc::new(|| Some(vec![agent("pirate", "arr", None), agent("robot", "beep", None)]));
        let tool = DelegateToAgentTool::live(vec![agent("pirate", "arr", None)], revision.clone(), resolver);
        assert_eq!(tool.spec().parameters["properties"]["agent_id"]["enum"], json!(["pirate"]));

        // Nothing moved yet: the old list stays, the resolver isn't consulted.
        assert_eq!(tool.spec().parameters["properties"]["agent_id"]["enum"], json!(["pirate"]));

        revision.bump();
        assert_eq!(tool.spec().parameters["properties"]["agent_id"]["enum"], json!(["pirate", "robot"]));
    }

    #[tokio::test]
    async fn a_live_tool_can_call_an_agent_added_after_it_was_built() {
        let revision = AgentsRevision::default();
        let resolver: AgentResolver = Arc::new(|| Some(vec![agent("pirate", "arr", None), agent("robot", "beep boop", None)]));
        let tool = DelegateToAgentTool::live(vec![agent("pirate", "arr", None)], revision.clone(), resolver);

        assert!(tool.call(json!({ "agent_id": "robot", "task": "hi" })).await.is_err());
        revision.bump();
        let result = tool.call(json!({ "agent_id": "robot", "task": "hi" })).await.unwrap();
        assert_eq!(result, json!({ "result": "beep boop" }));
    }

    #[test]
    fn a_live_tool_keeps_its_list_when_the_resolver_has_nothing() {
        let revision = AgentsRevision::default();
        let resolver: AgentResolver = Arc::new(|| None);
        let tool = DelegateToAgentTool::live(vec![agent("pirate", "arr", None)], revision.clone(), resolver);

        revision.bump();
        assert_eq!(tool.spec().parameters["properties"]["agent_id"]["enum"], json!(["pirate"]));
    }

    #[test]
    fn the_resolver_runs_once_per_change_not_once_per_look() {
        let revision = AgentsRevision::default();
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = calls.clone();
        let resolver: AgentResolver = Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            Some(vec![agent("pirate", "arr", None)])
        });
        let tool = DelegateToAgentTool::live(vec![agent("pirate", "arr", None)], revision.clone(), resolver);

        for _ in 0..5 {
            tool.spec();
        }
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        revision.bump();
        for _ in 0..5 {
            tool.spec();
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_budgeted_live_tool_still_refreshes() {
        let revision = AgentsRevision::default();
        let resolver: AgentResolver = Arc::new(|| Some(vec![agent("pirate", "arr", None), agent("robot", "beep", None)]));
        let tool = DelegateToAgentTool::live(vec![agent("pirate", "arr", None)], revision.clone(), resolver);

        let charged = tool.with_budget(&TurnBudget::new(10)).unwrap();
        revision.bump();
        assert_eq!(charged.spec().parameters["properties"]["agent_id"]["enum"], json!(["pirate", "robot"]));
    }
}
