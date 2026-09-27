//! Turning a channel's orchestrator into "agent X's orchestrator" for one turn (P46). The desktop, the
//! CLI and the hub all do exactly this, so it lives in one place: the skill scope, the opt-in tools
//! (`delegate_to_agent`, `manage_agents`, `message_agent`) and the agent's own tool list, in the one
//! order that keeps each rule intact.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use warden_core::orchestrator::Orchestrator;
use warden_core::tool::delegate_to_agent::AgentsRevision;
use warden_core::tool::Tool;

use crate::message_agent::{ConversationsChanged, MessageAgentTool};
use crate::{build_delegate_to_agent_tool, build_live_delegate_to_agent_tool, FileConfig, ManageAgentsTool, ManageTasksTool};

/// What differs per channel when scoping an agent.
#[derive(Clone, Default)]
pub struct AgentExtras {
    /// Where this channel keeps its conversations. Without one the agent gets no `message_agent`:
    /// the message has to land in a conversation the person can open.
    pub conversations_dir: Option<PathBuf>,
    /// Told when `message_agent` creates or updates a conversation, so the UI can reload its list.
    pub on_conversation_changed: Option<ConversationsChanged>,
}

/// An orchestrator ready to speak as one configured agent.
pub struct ScopedAgent {
    /// Scoped to the agent's skills and tool list, with the opt-in tools it has flags for. No
    /// model swap and no approver yet: the caller picks the model (an explicit choice may beat the
    /// agent's own `provider_id`) and attaches the approver its channel has, if any.
    pub orchestrator: Orchestrator,
    pub persona: String,
    /// The agent's default model provider, for the caller to apply.
    pub provider_id: Option<String>,
}

/// Scopes `base` to the agent `agent_id` of `config`, or `None` when there's no such agent.
///
/// `base` must be the channel's orchestrator *before* any narrowing, with the spend context already
/// set (P4): delegation targets and `message_agent` recipients are cloned from it, each then narrowed
/// to its own tool list rather than this agent's. `config_path` is the file `config` came from;
/// without it `manage_agents` and `message_agent` are left out (there's nothing to edit or re-read)
/// and `delegate_to_agent` keeps a fixed target list.
pub fn scope_to_agent(base: &Orchestrator, config: &FileConfig, config_path: Option<&Path>, agent_id: &str, extras: AgentExtras) -> Option<ScopedAgent> {
    let agent = config.agents.iter().find(|a| a.id == agent_id)?;
    // Skills (P72 c) follow the agent.
    let orchestrator = base.with_agent(Some(agent.id.clone()));

    // Shared by both tools: an agent `manage_agents` creates mid-turn shows up in `delegate_to_agent` at once.
    let agents_revision = AgentsRevision::default();
    let mut extra: Vec<Arc<dyn Tool>> = Vec::new();
    if agent.can_delegate_to_agents {
        extra.extend(match config_path {
            Some(path) => build_live_delegate_to_agent_tool(path, config, &orchestrator, agents_revision.clone()),
            None => build_delegate_to_agent_tool(config, &orchestrator),
        });
    }
    if let Some(path) = config_path {
        // Lets a "chief" create/edit other agents; every change waits for a person's yes.
        if agent.can_manage_agents {
            let known_tools: Vec<String> = base.tools().iter().map(|t| t.spec().name).collect();
            extra.push(Arc::new(
                ManageAgentsTool::new(path)
                    .with_known_tools(known_tools)
                    .with_caller_limit(agent.allowed_tools.clone())
                    .with_agents_revision(agents_revision),
            ));
        }
        // P92: scheduled tasks, every change waiting for a person's yes.
        if agent.can_manage_tasks {
            extra.push(Arc::new(ManageTasksTool::new(path).with_caller_limit(agent.allowed_tools.clone())));
        }
        if agent.can_message_agents {
            if let Some(dir) = &extras.conversations_dir {
                extra.push(Arc::new(
                    MessageAgentTool::new(agent.id.clone(), path, dir.clone(), base.clone()).on_changed(extras.on_conversation_changed.clone()),
                ));
            }
        }
    }

    // The agent's own tool list (P46) applies to what it had; the opt-in tools follow their flags.
    let orchestrator = extra.into_iter().fold(orchestrator.with_allowed_tools(agent.allowed_tools.as_deref()), |o, tool| o.with_tool(tool));
    Some(ScopedAgent { orchestrator, persona: agent.persona.clone(), provider_id: agent.provider_id.clone() })
}
