//! `manage_agents` (P46): lets a "chief" agent list, create and edit the other named agents in
//! `config.toml`. Lives here rather than in `warden-core` because it works on `AgentConfig` and
//! reads/writes the config file, exactly like `usage::UsageStatsTool` does for its data.
//!
//! The safety story is four rules, all enforced here and not left to the model:
//! 1. Only an agent that opted in (`AgentConfig.can_manage_agents`) is ever given the tool — the
//!    channels (desktop, CLI) attach it per turn, the same way they do for `delegate_to_agent`.
//! 2. Every `create`/`update`/`delete` waits for a human "yes" through the `Approver`, and the request
//!    shows the *whole* persona that would be saved or lost. Without an approver it refuses.
//! 3. No agent can grant a permission: created agents always start with `can_manage_agents` and
//!    `can_delegate_to_agents` off, and `update`/`delete` refuse to touch an agent that has either one
//!    (that includes the caller itself). Those flags are switched on by a person only.
//! 4. Tool isolation: an agent created without a tool list gets `SAFE_AGENT_TOOLS` (read-only), a
//!    requested list may only name tools that exist (never `delegate_to_agent`/`manage_agents`) and
//!    can't exceed the tools the calling agent itself is limited to, and the approval card shows it.
//!
//! `delete` was left out at first (same reasoning as `manage_skill`: a mistake should only overwrite,
//! never silently lose, something the user set up). It exists now because the approval card shows the
//! whole persona being lost and every delete needs the person's "yes". Deleting also cleans up the SSH
//! hosts that named the agent (`remove_agent_from`): a host left with no agent is switched off, never
//! opened to everyone.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use warden_core::autonomy::Category;
use warden_core::tool::delegate_to_agent::AgentsRevision;
use warden_core::tool::{ApprovalRequest, Approver, Tool, ToolSpec};

use crate::agent_changes::{self, AgentChange};
use crate::{load_config_from_path, org, remove_agent_from, save_config, AgentConfig, FileConfig, SshHostConfig, SAFE_AGENT_TOOLS};

const MAX_ID_CHARS: usize = 64;
const MAX_ROLE_CHARS: usize = 80;
/// Tools that follow `AgentConfig.can_delegate_to_agents`/`can_manage_agents`/`can_message_agents`/
/// `can_manage_tasks`, never a tool list.
const FLAG_GATED_TOOLS: [&str; 4] = ["delegate_to_agent", "manage_agents", "message_agent", "manage_tasks"];
/// Small enough that the approval card can show the whole persona — a reviewer must see everything
/// that will be saved, not a truncated preview.
const MAX_PERSONA_CHARS: usize = 4000;
const LIST_PERSONA_PREVIEW_CHARS: usize = 200;
const APPROVAL_TIMEOUT: Duration = Duration::from_secs(120);

/// What the model asked for, parsed once so it can be checked before asking a human and then
/// re-applied to a freshly read config after they answer.
#[derive(Debug, Clone, PartialEq)]
enum Change {
    /// `allowed_tools: None` = the safe default. `reports_to: None` = the caller (P120).
    Create { id: String, persona: String, provider_id: Option<String>, allowed_tools: Option<Vec<String>>, role: Option<String>, reports_to: Option<String> },
    /// `None` = leave as is. `provider_id: Some(None)` = clear it (an empty string from the model); so is `role`.
    Update {
        id: String,
        persona: Option<String>,
        provider_id: Option<Option<String>>,
        allowed_tools: Option<Vec<String>>,
        role: Option<Option<String>>,
        reports_to: Option<String>,
    },
    Delete { id: String },
}

/// What `change` would leave in the config: the new agent list and SSH hosts (only a delete touches
/// the hosts), plus what a human needs to read before approving.
struct Planned {
    agents: Vec<AgentConfig>,
    ssh_hosts: Vec<SshHostConfig>,
    detail: String,
}

impl Change {
    fn action(&self) -> &'static str {
        match self {
            Change::Create { .. } => "create_agent",
            Change::Update { .. } => "update_agent",
            Change::Delete { .. } => "delete_agent",
        }
    }

    fn id(&self) -> &str {
        match self {
            Change::Create { id, .. } | Change::Update { id, .. } | Change::Delete { id } => id,
        }
    }
}

/// Names a created/edited agent's tool list is checked against.
#[derive(Clone, Default)]
struct ToolRules {
    /// Every tool that exists. `None` = not told, so unknown names aren't caught (tests only).
    known: Option<Vec<String>>,
    /// The calling agent's own `allowed_tools`: nobody may hand out more than they have.
    caller_limit: Option<Vec<String>>,
    /// The calling agent's own `autonomy` (P122): an agent it creates never gets more.
    caller_autonomy: Option<u8>,
    /// The calling agent (P120): its scope of authority is the agents that report to it, directly or not. `None` (a use
    /// with no organization, as before) keeps the old rule: nothing with the power to delegate or manage is touched.
    caller: Option<String>,
}

/// The `autonomy` an agent made by another agent starts at (P122): it asks before every change.
pub(crate) const ASK_FIRST_LEVEL: u8 = 3;

#[derive(Clone)]
pub struct ManageAgentsTool {
    config_path: PathBuf,
    approver: Option<Arc<dyn Approver>>,
    approval_timeout: Duration,
    rules: ToolRules,
    /// Bumped after every applied change, so a `delegate_to_agent` built earlier in the same turn
    /// (given the same handle) sees the new agent list right away.
    revision: Option<AgentsRevision>,
}

impl ManageAgentsTool {
    pub fn new(config_path: impl Into<PathBuf>) -> Self {
        Self { config_path: config_path.into(), approver: None, approval_timeout: APPROVAL_TIMEOUT, rules: ToolRules::default(), revision: None }
    }

    /// Tells a `delegate_to_agent` sharing `revision` that the agent list changed.
    pub fn with_agents_revision(mut self, revision: AgentsRevision) -> Self {
        self.revision = Some(revision);
        self
    }

    /// The names of every tool the running orchestrator has (`Orchestrator::tools`), so a made-up
    /// name is refused before the user is asked anything.
    pub fn with_known_tools(mut self, names: Vec<String>) -> Self {
        self.rules.known = Some(names);
        self
    }

    /// The calling agent's own `allowed_tools` (`None` = it has every tool, so no cap).
    pub fn with_caller_limit(mut self, limit: Option<Vec<String>>) -> Self {
        self.rules.caller_limit = limit;
        self
    }

    /// The calling agent's own `autonomy` (P122), so what it creates starts no freer than it is.
    pub fn with_caller_autonomy(mut self, level: u8) -> Self {
        self.rules.caller_autonomy = Some(level);
        self
    }

    /// The agent that is calling (P120): it manages only the ones that report to it, directly or not, and never itself.
    pub fn with_caller(mut self, id: impl Into<String>) -> Self {
        self.rules.caller = Some(id.into());
        self
    }

    /// Only tests wait less than the real deadline.
    #[cfg(test)]
    fn with_approval_timeout(mut self, timeout: Duration) -> Self {
        self.approval_timeout = timeout;
        self
    }

    fn load(&self) -> anyhow::Result<FileConfig> {
        load_config_from_path(&self.config_path, false)
    }

    fn list(&self) -> anyhow::Result<Value> {
        let config = self.load()?;
        // P120: a caller in an organization sees (and manages) only the agents below it.
        let scope: Option<Vec<String>> = self.rules.caller.as_deref().map(|caller| org::subordinates_of(&config.agents, caller));
        let agents: Vec<Value> = config
            .agents
            .iter()
            // P84: members' own agents are theirs, not the owner's chief's to see or change.
            .filter(|a| a.owner.is_none())
            .filter(|a| scope.as_ref().is_none_or(|scope| scope.contains(&a.id)))
            .map(|a| {
                json!({
                    "id": a.id,
                    "role": a.role,
                    "reports_to": a.reports_to,
                    "persona": preview(&a.persona, LIST_PERSONA_PREVIEW_CHARS),
                    "provider_id": a.provider_id,
                    "can_delegate_to_agents": a.can_delegate_to_agents,
                    "can_manage_agents": a.can_manage_agents,
                    "can_message_agents": a.can_message_agents,
                    "can_manage_tasks": a.can_manage_tasks,
                    "allowed_tools": a.allowed_tools,
                })
            })
            .collect();
        let providers: Vec<&str> = config.providers.iter().map(|p| p.id.as_str()).collect();
        Ok(json!({
            "agents": agents,
            "available_provider_ids": providers,
            "grantable_tool_names": self.rules.grantable(),
            "default_tools_for_new_agents": self.rules.default_tools(),
        }))
    }

    /// Whether the agent using this tool is at autonomy 5. Only an agent with a place in the organization counts: with no
    /// caller (the old use, no scope of authority) every change still waits for a person.
    fn manages_alone(&self) -> bool {
        self.rules.caller.is_some() && self.rules.caller_autonomy == Some(warden_core::autonomy::Autonomy::Manager.level())
    }

    async fn change(&self, change: Change) -> anyhow::Result<Value> {
        // Check first, so a request that can't succeed never costs the user a prompt.
        let Planned { detail, .. } = plan(&self.load()?, &change, &self.rules)?;

        // Level 5 (P122): a manager changes the agents under it without a person's yes. `plan` has already held the change to
        // the manager's scope and ceiling, so what is skipped is only the question; a kind of action the person ticked for this
        // agent still asks, in the orchestrator, before this tool runs.
        if !self.manages_alone() {
            let Some(approver) = &self.approver else {
                anyhow::bail!(
                    "creating, changing or deleting agents needs the user's approval, and this channel can't ask for it \
                     (use the desktop app or the interactive CLI)"
                );
            };
            let request = ApprovalRequest::new(change.id(), change.action(), detail);
            let approved = tokio::time::timeout(self.approval_timeout, approver.approve(request)).await.unwrap_or(false);
            if !approved {
                anyhow::bail!("the user did not approve this change to agent '{}'", change.id());
            }
        }

        // The prompt can sit open for a while and the user may have edited agents meanwhile, so
        // re-check and apply against a fresh read rather than the one from before the wait.
        let mut config = self.load()?;
        let planned = plan(&config, &change, &self.rules)?;
        config.agents = planned.agents;
        config.ssh_hosts = planned.ssh_hosts;
        save_config(&self.config_path, &config)?;
        if let Some(revision) = &self.revision {
            revision.bump();
        }
        self.record_change(&change);
        let message = match change {
            Change::Delete { .. } => format!("Agent '{}' deleted.", change.id()),
            Change::Create { .. } | Change::Update { .. } => format!(
                "Agent '{}' {}. It can be picked as the conversation's agent from the user's next message{}.",
                change.id(),
                if matches!(change, Change::Create { .. }) { "created" } else { "updated" },
                if self.revision.is_some() { "; delegate_to_agent already lists it" } else { "" }
            ),
        };
        Ok(json!({ "status": "ok", "message": message }))
    }

    /// P121: an agent created or removed goes to the feed of activity, next to the config it was saved in. An edit does not.
    fn record_change(&self, change: &Change) {
        let (kind, detail) = match change {
            Change::Create { persona, role, .. } => ("created", role.clone().unwrap_or_else(|| persona.chars().take(LIST_PERSONA_PREVIEW_CHARS).collect())),
            Change::Delete { .. } => ("removed", String::new()),
            Change::Update { .. } => return,
        };
        let at_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
        let actor = self.rules.caller.clone().unwrap_or_default();
        agent_changes::record(&agent_changes::beside(&self.config_path), &AgentChange { at_ms, kind: kind.into(), actor, agent: change.id().to_string(), detail });
    }
}

/// What `config` would look like after `change`, or why that can't be done — plus what a human
/// needs to see to approve it. Pure (no I/O) so the same checks run before and after the prompt.
fn plan(config: &FileConfig, change: &Change, rules: &ToolRules) -> anyhow::Result<Planned> {
    let mut updated = config.agents.clone();
    let mut ssh_hosts = config.ssh_hosts.clone();
    let detail = match change {
        Change::Create { id, persona, provider_id, allowed_tools, role, reports_to } => {
            check_id(id)?;
            check_persona(persona)?;
            check_provider(config, provider_id.as_deref())?;
            let role = check_role(role.as_deref())?;
            // P120: a new agent reports to whoever created it, or to someone below it; never from outside its scope.
            let superior = match (&rules.caller, reports_to) {
                (Some(caller), Some(wanted)) => {
                    check_superior_in_scope(config, caller, wanted)?;
                    Some(wanted.clone())
                }
                (Some(caller), None) => Some(caller.clone()),
                (None, wanted) => wanted.clone(),
            };
            let tools = match allowed_tools {
                Some(requested) => rules.check(requested)?,
                None => rules.default_tools(),
            };
            if config.agents.iter().any(|a| &a.id == id) {
                anyhow::bail!("an agent named '{id}' already exists — pick another name, or use action 'update'");
            }
            let level = rules.caller_autonomy.map_or(ASK_FIRST_LEVEL, |caller| caller.min(ASK_FIRST_LEVEL));
            updated.push(AgentConfig {
                id: id.clone(),
                persona: persona.clone(),
                provider_id: provider_id.clone(),
                // Never granted from here; only a person turns these on.
                can_delegate_to_agents: false,
                can_manage_agents: false,
                can_message_agents: false,
                can_manage_tasks: false,
                allowed_tools: Some(tools.clone()),
                // Careful by default: it asks before every change until a person raises it.
                autonomy: level,
                // Every kind of risky action needs a person's yes until the person says otherwise.
                approval_required: Category::ALL.to_vec(),
                role: role.clone(),
                reports_to: superior.clone(),
                owner: None,
                shared_with: Vec::new(),
                // Never granted from here either: a person limits the models of an agent.
                delegation_models: Vec::new(),
                // Like its autonomy level and its categories: the new agent is the cautious one, and only a person turns these on.
                can_start_tasks: false,
                can_create_workers: false,
                can_message_user: false,
                can_choose_models: false,
            });
            org::check_hierarchy(&updated).map_err(|why| anyhow::anyhow!(why))?;
            format!(
                "New agent '{id}'\nRole: {}\nReports to: {}\nModel provider: {}\nTools: {}\nAutonomy {}: {}\nCannot delegate or manage agents (only you can turn that on).\n\nPersona:\n{persona}",
                role.as_deref().unwrap_or("(none)"),
                superior.as_deref().unwrap_or("(nobody)"),
                provider_label(provider_id.as_deref()),
                tools_label(Some(&tools)),
                level,
                autonomy_label(level)
            )
        }
        Change::Update { id, persona, provider_id, allowed_tools, role, reports_to } => {
            let Some(index) = config.agents.iter().position(|a| &a.id == id && a.owner.is_none()) else {
                anyhow::bail!("no agent named '{id}' — use action 'list' to see the existing ones");
            };
            let current = &config.agents[index];
            check_authority(config, rules, current, "edit")?;
            if persona.is_none() && provider_id.is_none() && allowed_tools.is_none() && role.is_none() && reports_to.is_none() {
                anyhow::bail!("nothing to change — pass 'persona', 'provider_id', 'allowed_tools', 'role' and/or 'reports_to'");
            }
            let mut lines = vec![format!("Change agent '{id}'")];
            if let Some(new_role) = role {
                let new_role = check_role(new_role.as_deref())?;
                lines.push(format!("Role: {} → {}", current.role.as_deref().unwrap_or("(none)"), new_role.as_deref().unwrap_or("(none)")));
                updated[index].role = new_role;
            }
            if let Some(new_superior) = reports_to {
                // Moving an agent: under the caller or someone below it, and never under the agent itself or its own branch.
                if let Some(caller) = &rules.caller {
                    check_superior_in_scope(config, caller, new_superior)?;
                }
                if new_superior == id || org::subordinates_of(&config.agents, id).contains(new_superior) {
                    anyhow::bail!("'{id}' can't report to '{new_superior}': that would put it under itself or one of its own subordinates");
                }
                lines.push(format!("Reports to: {} → {new_superior}", current.reports_to.as_deref().unwrap_or("(nobody)")));
                updated[index].reports_to = Some(new_superior.clone());
            }
            if let Some(new_provider) = provider_id {
                check_provider(config, new_provider.as_deref())?;
                lines.push(format!(
                    "Model provider: {} → {}",
                    provider_label(current.provider_id.as_deref()),
                    provider_label(new_provider.as_deref())
                ));
                updated[index].provider_id = new_provider.clone();
            }
            if let Some(requested) = allowed_tools {
                let tools = rules.check(requested)?;
                lines.push(format!("Tools: {} → {}", tools_label(current.allowed_tools.as_deref()), tools_label(Some(&tools))));
                updated[index].allowed_tools = Some(tools);
            }
            if let Some(new_persona) = persona {
                check_persona(new_persona)?;
                lines.push(format!("\nOld persona:\n{}\n\nNew persona:\n{new_persona}", current.persona));
                updated[index].persona = new_persona.clone();
            }
            org::check_hierarchy(&updated).map_err(|why| anyhow::anyhow!(why))?;
            lines.join("\n")
        }
        Change::Delete { id } => {
            let Some(current) = config.agents.iter().find(|a| &a.id == id && a.owner.is_none()) else {
                anyhow::bail!("no agent named '{id}' — use action 'list' to see the existing ones");
            };
            check_authority(config, rules, current, "delete")?;
            let effects = remove_agent_from(&mut updated, &mut ssh_hosts, id);
            let mut lines = vec![
                format!("Delete agent '{id}' — this cannot be undone"),
                format!("Model provider: {}", provider_label(current.provider_id.as_deref())),
                format!("Tools: {}", tools_label(current.allowed_tools.as_deref())),
            ];
            let touches_ssh = !effects.is_empty();
            for effect in effects {
                lines.push(if effect.switched_off {
                    format!("SSH server '{}' was only for this agent, so it will be switched off.", effect.host_id)
                } else {
                    format!("SSH server '{}' will stop listing this agent (other agents keep their access).", effect.host_id)
                });
            }
            if touches_ssh {
                lines.push("(SSH servers are read when Warden starts, so those changes apply from the next launch.)".to_string());
            }
            lines.push(format!("\nPersona that will be lost:\n{}", current.persona));
            lines.join("\n")
        }
    };
    Ok(Planned { agents: updated, ssh_hosts, detail })
}

impl ToolRules {
    /// The tools an agent may be given from here: all that exist (or, when not told, the safe set),
    /// within the caller's own limit, and never the two that follow the `can_*` flags.
    fn grantable(&self) -> Vec<String> {
        let base: Vec<String> = match &self.known {
            Some(known) => known.clone(),
            None => SAFE_AGENT_TOOLS.iter().map(|t| t.to_string()).collect(),
        };
        base.into_iter().filter(|t| !FLAG_GATED_TOOLS.contains(&t.as_str()) && self.within_caller_limit(t)).collect()
    }

    /// `SAFE_AGENT_TOOLS` that actually exist and that the caller could hand out itself.
    fn default_tools(&self) -> Vec<String> {
        SAFE_AGENT_TOOLS
            .iter()
            .map(|t| t.to_string())
            .filter(|t| self.within_caller_limit(t) && self.known.as_ref().is_none_or(|k| k.contains(t)))
            .collect()
    }

    fn within_caller_limit(&self, tool: &str) -> bool {
        self.caller_limit.as_ref().is_none_or(|limit| limit.iter().any(|t| t == tool))
    }

    /// `requested` cleaned up (trimmed, de-duplicated), or why it can't be granted.
    fn check(&self, requested: &[String]) -> anyhow::Result<Vec<String>> {
        let mut tools: Vec<String> = Vec::new();
        for name in requested.iter().map(|t| t.trim()) {
            if FLAG_GATED_TOOLS.contains(&name) {
                anyhow::bail!("'{name}' can't be put in a tool list — only the user can give an agent that power (Settings or /agents)");
            }
            if let Some(known) = &self.known {
                if !known.iter().any(|t| t == name) {
                    anyhow::bail!("unknown tool '{name}' (available: {}) — see 'grantable_tool_names' in 'list'", self.grantable().join(", "));
                }
            }
            if !self.within_caller_limit(name) {
                anyhow::bail!("you can't give another agent the tool '{name}': it isn't one of yours");
            }
            if !tools.iter().any(|t| t == name) {
                tools.push(name.to_string());
            }
        }
        Ok(tools)
    }
}

/// Whether the caller may `verb` (edit or delete) `target` (P120). With a caller it has to be one of the agents below
/// it, and one with the power to delegate or manage agents only if the caller has that power too (the ceiling is the
/// caller itself); with no caller, nothing that has either power is touched (the rule from before the hierarchy).
fn check_authority(config: &FileConfig, rules: &ToolRules, target: &AgentConfig, verb: &str) -> anyhow::Result<()> {
    let id = &target.id;
    let Some(caller) = &rules.caller else {
        if target.can_delegate_to_agents || target.can_manage_agents {
            anyhow::bail!(
                "agent '{id}' can delegate or manage agents, so only the user can {verb} it (Settings or /agents) — \
                 an agent may not change one with more power than a plain agent"
            );
        }
        return Ok(());
    };
    if !org::subordinates_of(&config.agents, caller).contains(id) {
        anyhow::bail!(
            "agent '{id}' is not yours to {verb}: it doesn't report to you, directly or indirectly — you manage only your own \
             subordinates (see 'list'); anyone else's agents are for the user or their manager"
        );
    }
    let power = config.agents.iter().find(|a| &a.id == caller);
    if target.can_delegate_to_agents && !power.is_some_and(|c| c.can_delegate_to_agents) {
        anyhow::bail!("agent '{id}' can delegate to other agents and you can't, so you can't {verb} it — only someone with that power, or the user, can");
    }
    if target.can_manage_agents && !power.is_some_and(|c| c.can_manage_agents) {
        anyhow::bail!("agent '{id}' can manage agents and you can't, so you can't {verb} it — only someone with that power, or the user, can");
    }
    Ok(())
}

/// `wanted` as somebody's superior: the caller itself or one of the agents below it, so an agent is never moved out of
/// the caller's reach or put under someone the caller has no authority over.
fn check_superior_in_scope(config: &FileConfig, caller: &str, wanted: &str) -> anyhow::Result<()> {
    if wanted == caller || org::subordinates_of(&config.agents, caller).iter().any(|a| a == wanted) {
        return Ok(());
    }
    anyhow::bail!("'{wanted}' can't be a superior here: only you or one of your own subordinates can (see 'list')")
}

/// A role as the model wrote it: trimmed, blank is none, short and on one line.
pub(crate) fn check_role(role: Option<&str>) -> anyhow::Result<Option<String>> {
    let Some(role) = role.map(str::trim).filter(|r| !r.is_empty()) else { return Ok(None) };
    if role.chars().count() > MAX_ROLE_CHARS || role.chars().any(char::is_control) {
        anyhow::bail!("role must be at most {MAX_ROLE_CHARS} characters, on one line");
    }
    Ok(Some(role.to_string()))
}

fn autonomy_label(level: u8) -> &'static str {
    match level {
        1 => "only answers, no tools",
        2 => "suggests, never changes anything itself",
        3 => "asks before every change",
        _ => "acts on its own",
    }
}

fn tools_label(tools: Option<&[String]>) -> String {
    match tools {
        None => "(all tools)".to_string(),
        Some([]) => "(none)".to_string(),
        Some(list) => list.join(", "),
    }
}

pub(crate) fn check_id(id: &str) -> anyhow::Result<()> {
    if id.trim().is_empty() || id != id.trim() {
        anyhow::bail!("agent id must not be empty or start/end with spaces");
    }
    if id.chars().count() > MAX_ID_CHARS || id.chars().any(char::is_control) {
        anyhow::bail!("agent id must be at most {MAX_ID_CHARS} characters, with no control characters");
    }
    Ok(())
}

pub(crate) fn check_persona(persona: &str) -> anyhow::Result<()> {
    if persona.trim().is_empty() {
        anyhow::bail!("persona must not be empty");
    }
    if persona.chars().count() > MAX_PERSONA_CHARS {
        anyhow::bail!("persona is over {MAX_PERSONA_CHARS} characters — shorten it");
    }
    Ok(())
}

fn check_provider(config: &FileConfig, provider_id: Option<&str>) -> anyhow::Result<()> {
    match provider_id {
        Some(id) if !config.providers.iter().any(|p| p.id == id) && !config.combos.iter().any(|c| c.id == id) => {
            let known: Vec<&str> = config.providers.iter().map(|p| p.id.as_str()).chain(config.combos.iter().map(|c| c.id.as_str())).collect();
            anyhow::bail!("unknown provider_id '{id}' (available: {}) — or leave it out to use the default", known.join(", "))
        }
        _ => Ok(()),
    }
}

fn provider_label(provider_id: Option<&str>) -> &str {
    provider_id.unwrap_or("(default model)")
}

fn preview(text: &str, max_chars: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > max_chars {
        format!("{}…", flat.chars().take(max_chars).collect::<String>())
    } else {
        flat
    }
}

/// A text argument (`role`, `reports_to`) from the model: absent = don't touch, empty string = clear, anything else = set.
fn text_arg(args: &Value, name: &str) -> Option<Option<String>> {
    args.get(name).and_then(Value::as_str).map(|s| Some(s.trim().to_string()).filter(|s| !s.is_empty()))
}

/// `provider_id` from the model: absent = don't touch, empty string = clear, anything else = set.
fn provider_arg(args: &Value) -> Option<Option<String>> {
    args.get("provider_id").and_then(Value::as_str).map(|s| Some(s.trim().to_string()).filter(|s| !s.is_empty()))
}

/// `allowed_tools` from the model: absent = the default/leave alone, an array (even empty) = that list.
fn allowed_tools_arg(args: &Value) -> anyhow::Result<Option<Vec<String>>> {
    let Some(value) = args.get("allowed_tools").filter(|v| !v.is_null()) else { return Ok(None) };
    let items = value.as_array().ok_or_else(|| anyhow::anyhow!("'allowed_tools' must be an array of tool names"))?;
    items
        .iter()
        .map(|item| item.as_str().map(str::to_string).ok_or_else(|| anyhow::anyhow!("'allowed_tools' must contain only strings")))
        .collect::<anyhow::Result<Vec<_>>>()
        .map(Some)
}

#[async_trait]
impl Tool for ManageAgentsTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "manage_agents".to_string(),
            description: "List, create, edit or delete the user's named agents (each is a persona, optionally with its \
                          own model). Use it only when the user asks to create, change or remove an agent. Every \
                          create/update/delete is shown to the user, who must approve it (a delete shows the whole \
                          persona that will be lost); you cannot give any agent the power to delegate or manage other \
                          agents. You manage only the agents that report to you, directly or through someone else \
                          (see 'list'), never yourself, your superior or your peers; one of them that can delegate or \
                          manage agents is yours to change only if you have that power too. A new agent reports to \
                          you (or to one of your subordinates, with 'reports_to') and starts with read-only tools \
                          unless you list others. It can be picked as the conversation's agent from the user's next \
                          message; if you have delegate_to_agent, you can delegate to it right away. Start with \
                          'list' to see what exists."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["list", "create", "update", "delete"] },
                    "id": {
                        "type": "string",
                        "description": "The agent's name (create: must be new; update: must exist)."
                    },
                    "persona": {
                        "type": "string",
                        "description": "The agent's full system prompt: who it is, what it is good at, how it should \
                                        behave. Required for create; for update it replaces the old one."
                    },
                    "provider_id": {
                        "type": "string",
                        "description": "Which configured model provider it uses (see 'available_provider_ids' in \
                                        'list'). Leave out for the default; on update, an empty string clears it."
                    },
                    "role": {
                        "type": "string",
                        "description": "The agent's role in the organization, e.g. 'PostgreSQL specialist'. Optional; \
                                        on update an empty string clears it."
                    },
                    "reports_to": {
                        "type": "string",
                        "description": "Who the agent reports to: you (the default on create) or one of your own \
                                        subordinates. On update it moves the agent there."
                    },
                    "allowed_tools": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "The only tools this agent may use, by name (see 'grantable_tool_names' in \
                                        'list'). Leave out on create for a read-only default \
                                        ('default_tools_for_new_agents'); on update, leave out to keep the current \
                                        list. Ask for as few as the job needs: the user sees the list and must approve."
                    }
                },
                "required": ["action"]
            }),
        }
    }

    fn with_approver(&self, approver: Arc<dyn Approver>) -> Option<Arc<dyn Tool>> {
        Some(Arc::new(Self { approver: Some(approver), ..self.clone() }))
    }

    async fn call(&self, args: Value) -> anyhow::Result<Value> {
        let action = args.get("action").and_then(Value::as_str).ok_or_else(|| anyhow::anyhow!("missing required 'action' argument"))?;
        if action == "list" {
            return self.list();
        }
        if !matches!(action, "create" | "update" | "delete") {
            anyhow::bail!("unknown action '{action}' — use 'list', 'create', 'update' or 'delete'");
        }
        let id = args.get("id").and_then(Value::as_str).ok_or_else(|| anyhow::anyhow!("missing required 'id' argument"))?.to_string();
        let persona = args.get("persona").and_then(Value::as_str).map(str::to_string);
        let change = match action {
            "delete" => Change::Delete { id },
            "create" => {
                let persona = persona.ok_or_else(|| anyhow::anyhow!("missing required 'persona' argument"))?;
                Change::Create {
                    id,
                    persona,
                    provider_id: provider_arg(&args).flatten(),
                    allowed_tools: allowed_tools_arg(&args)?,
                    role: text_arg(&args, "role").flatten(),
                    reports_to: text_arg(&args, "reports_to").flatten(),
                }
            }
            _ => Change::Update {
                id,
                persona,
                provider_id: provider_arg(&args),
                allowed_tools: allowed_tools_arg(&args)?,
                role: text_arg(&args, "role"),
                reports_to: text_arg(&args, "reports_to").flatten(),
            },
        };
        self.change(change).await
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Mutex;

    use super::*;
    use crate::{Provider, ProviderConfig};

    fn temp_config_path() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "warden-manage-agents-test-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("config.toml")
    }

    fn agent(id: &str, delegate: bool, manage: bool) -> AgentConfig {
        AgentConfig {
            id: id.into(),
            persona: format!("persona of {id}"),
            provider_id: None,
            can_delegate_to_agents: delegate,
            can_manage_agents: manage,
            can_message_agents: false,
            can_manage_tasks: false,
            allowed_tools: None,
            autonomy: crate::default_autonomy(),
            approval_required: Vec::new(),
            role: None,
            reports_to: None,
            owner: None,
            shared_with: Vec::new(),
            delegation_models: Vec::new(),
            can_start_tasks: true,
            can_create_workers: true,
            can_message_user: true,
            can_choose_models: true,
        }
    }

    /// Writes a config with a provider `local` and the given agents, and returns the path.
    fn write_config(agents: Vec<AgentConfig>) -> PathBuf {
        let path = temp_config_path();
        let config = FileConfig {
            providers: vec![ProviderConfig {
                id: "local".into(),
                kind: Provider::OpenaiCompatible,
                api_key: None,
                base_url: Some("http://localhost:11434/v1".into()),
                model: Some("m".into()),
                node: None,
            }],
            agents,
            ..FileConfig::default()
        };
        save_config(&path, &config).unwrap();
        path
    }

    struct Scripted {
        answer: bool,
        asked: Mutex<Vec<ApprovalRequest>>,
    }

    #[async_trait]
    impl Approver for Scripted {
        async fn approve(&self, request: ApprovalRequest) -> bool {
            self.asked.lock().unwrap().push(request);
            self.answer
        }
    }

    struct NeverAnswers;

    #[async_trait]
    impl Approver for NeverAnswers {
        async fn approve(&self, _request: ApprovalRequest) -> bool {
            std::future::pending().await
        }
    }

    fn tool_with(path: &Path, answer: bool) -> (Arc<dyn Tool>, Arc<Scripted>) {
        let approver = Arc::new(Scripted { answer, asked: Mutex::new(Vec::new()) });
        (ManageAgentsTool::new(path).with_approver(approver.clone()).unwrap(), approver)
    }

    fn agents_on_disk(path: &Path) -> Vec<AgentConfig> {
        load_config_from_path(path, true).unwrap().agents
    }

    #[tokio::test]
    async fn create_saves_the_agent_with_no_special_powers_after_approval() {
        let path = write_config(vec![agent("chief", true, true)]);
        let (tool, approver) = tool_with(&path, true);

        let result = tool
            .call(json!({ "action": "create", "id": "linux-admin", "persona": "You administer Linux servers.", "provider_id": "local" }))
            .await
            .unwrap();

        assert_eq!(result["status"], json!("ok"));
        let created = agents_on_disk(&path).into_iter().find(|a| a.id == "linux-admin").unwrap();
        assert_eq!(created.persona, "You administer Linux servers.");
        assert_eq!(created.provider_id.as_deref(), Some("local"));
        assert!(!created.can_delegate_to_agents && !created.can_manage_agents);
        // The chief itself is untouched.
        assert!(agents_on_disk(&path).iter().any(|a| a.id == "chief" && a.can_manage_agents));

        let asked = approver.asked.lock().unwrap();
        assert_eq!((asked[0].action.as_str(), asked[0].target.as_str()), ("create_agent", "linux-admin"));
        assert!(asked[0].detail.contains("You administer Linux servers.") && asked[0].detail.contains("local"));
    }

    #[tokio::test]
    async fn an_agent_created_or_removed_is_written_for_the_feed_and_a_refused_or_edited_one_is_not() {
        let path = write_config(vec![agent("chief", true, true)]);
        let log = crate::agent_changes::beside(&path);
        let with = |answer: bool| {
            let approver = Arc::new(Scripted { answer, asked: Mutex::new(Vec::new()) });
            ManageAgentsTool::new(&path).with_caller("chief").with_approver(approver).unwrap()
        };

        with(false).call(json!({ "action": "create", "id": "poet", "persona": "You write poems." })).await.unwrap_err();
        assert!(crate::agent_changes::read_agent_changes(&log).is_empty(), "a refused change is not written");

        with(true).call(json!({ "action": "create", "id": "poet", "persona": "You write poems.", "role": "Poet" })).await.unwrap();
        with(true).call(json!({ "action": "update", "id": "poet", "persona": "You write short poems." })).await.unwrap();
        with(true).call(json!({ "action": "delete", "id": "poet" })).await.unwrap();

        let changes = crate::agent_changes::read_agent_changes(&log);
        let seen: Vec<(&str, &str, &str, &str)> = changes.iter().map(|c| (c.kind.as_str(), c.actor.as_str(), c.agent.as_str(), c.detail.as_str())).collect();
        assert_eq!(seen, vec![("created", "chief", "poet", "Poet"), ("removed", "chief", "poet", "")], "the edit is not written");
    }

    #[tokio::test]
    async fn the_agents_revision_moves_only_when_a_change_is_applied() {
        let path = write_config(vec![]);
        let revision = AgentsRevision::default();
        let with = |answer: bool| {
            let approver = Arc::new(Scripted { answer, asked: Mutex::new(Vec::new()) });
            ManageAgentsTool::new(&path).with_agents_revision(revision.clone()).with_approver(approver).unwrap()
        };
        let before = revision.current();

        // Refused, invalid, and read-only calls leave it alone.
        with(false).call(json!({ "action": "create", "id": "x", "persona": "p" })).await.unwrap_err();
        with(true).call(json!({ "action": "delete", "id": "ghost" })).await.unwrap_err();
        with(true).call(json!({ "action": "list" })).await.unwrap();
        assert_eq!(revision.current(), before);

        with(true).call(json!({ "action": "create", "id": "x", "persona": "p" })).await.unwrap();
        assert_ne!(revision.current(), before);
    }

    #[tokio::test]
    async fn an_edit_keeps_the_scheduled_tasks_flag_a_person_set() {
        let mut planner = agent("planner", false, false);
        planner.can_manage_tasks = true;
        let path = write_config(vec![planner]);
        let (tool, _) = tool_with(&path, true);
        tool.call(json!({ "action": "update", "id": "planner", "persona": "new persona" })).await.unwrap();
        let saved = &agents_on_disk(&path)[0];
        assert_eq!((saved.persona.as_str(), saved.can_manage_tasks), ("new persona", true));
    }

    #[tokio::test]
    async fn a_model_cannot_smuggle_flags_in_through_extra_arguments() {
        let path = write_config(vec![]);
        let (tool, _) = tool_with(&path, true);
        tool.call(json!({
            "action": "create",
            "id": "sneaky",
            "persona": "p",
            "can_manage_agents": true,
            "can_delegate_to_agents": true,
            "can_message_agents": true,
            "can_manage_tasks": true
        }))
        .await
        .unwrap();
        let created = &agents_on_disk(&path)[0];
        assert!(!created.can_delegate_to_agents && !created.can_manage_agents && !created.can_message_agents && !created.can_manage_tasks);
    }

    #[tokio::test]
    async fn a_denied_change_writes_nothing() {
        let path = write_config(vec![]);
        let (tool, approver) = tool_with(&path, false);

        let err = tool.call(json!({ "action": "create", "id": "x", "persona": "p" })).await.unwrap_err();

        assert!(err.to_string().contains("did not approve"));
        assert!(agents_on_disk(&path).is_empty());
        assert_eq!(approver.asked.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn without_an_approver_it_refuses_and_writes_nothing() {
        let path = write_config(vec![]);
        let tool = ManageAgentsTool::new(&path);
        let err = tool.call(json!({ "action": "create", "id": "x", "persona": "p" })).await.unwrap_err();
        assert!(err.to_string().contains("can't ask"));
        assert!(agents_on_disk(&path).is_empty());
    }

    #[tokio::test]
    async fn an_unanswered_prompt_counts_as_a_refusal() {
        let path = write_config(vec![]);
        let tool = ManageAgentsTool::new(&path).with_approval_timeout(Duration::from_millis(50)).with_approver(Arc::new(NeverAnswers)).unwrap();
        let err = tool.call(json!({ "action": "create", "id": "x", "persona": "p" })).await.unwrap_err();
        assert!(err.to_string().contains("did not approve"));
        assert!(agents_on_disk(&path).is_empty());
    }

    #[tokio::test]
    async fn invalid_requests_are_refused_before_asking_anyone() {
        let path = write_config(vec![agent("taken", false, false)]);
        let (tool, approver) = tool_with(&path, true);
        let too_long = "x".repeat(MAX_PERSONA_CHARS + 1);
        let cases = [
            (json!({ "action": "create", "id": "taken", "persona": "p" }), "already exists"),
            (json!({ "action": "create", "id": "  ", "persona": "p" }), "must not be empty"),
            (json!({ "action": "create", "id": " padded", "persona": "p" }), "start/end with spaces"),
            (json!({ "action": "create", "id": "a\nb", "persona": "p" }), "control characters"),
            (json!({ "action": "create", "id": "new", "persona": "   " }), "persona must not be empty"),
            (json!({ "action": "create", "id": "new", "persona": too_long }), "shorten"),
            (json!({ "action": "create", "id": "new", "persona": "p", "provider_id": "ghost" }), "unknown provider_id"),
            (json!({ "action": "create", "id": "new" }), "persona"),
            (json!({ "action": "update", "id": "ghost", "persona": "p" }), "no agent named"),
            (json!({ "action": "update", "id": "taken" }), "nothing to change"),
            (json!({ "action": "frobnicate" }), "unknown action"),
            (json!({ "id": "x" }), "action"),
        ];
        for (args, expected) in cases {
            let err = tool.call(args.clone()).await.unwrap_err();
            assert!(err.to_string().contains(expected), "{args} → {err}");
        }
        assert!(approver.asked.lock().unwrap().is_empty());
        assert_eq!(agents_on_disk(&path).len(), 1);
    }

    #[tokio::test]
    async fn update_changes_a_plain_agent_and_shows_old_and_new() {
        let path = write_config(vec![agent("writer", false, false)]);
        let (tool, approver) = tool_with(&path, true);

        tool.call(json!({ "action": "update", "id": "writer", "persona": "You write poems.", "provider_id": "local" })).await.unwrap();

        let updated = &agents_on_disk(&path)[0];
        assert_eq!((updated.persona.as_str(), updated.provider_id.as_deref()), ("You write poems.", Some("local")));
        let detail = approver.asked.lock().unwrap()[0].detail.clone();
        assert!(detail.contains("persona of writer") && detail.contains("You write poems.") && detail.contains("(default model) → local"));

        // An empty provider_id clears it; omitting it leaves it alone.
        tool.call(json!({ "action": "update", "id": "writer", "persona": "Still poems." })).await.unwrap();
        assert_eq!(agents_on_disk(&path)[0].provider_id.as_deref(), Some("local"));
        tool.call(json!({ "action": "update", "id": "writer", "provider_id": "" })).await.unwrap();
        assert_eq!(agents_on_disk(&path)[0].provider_id, None);
    }

    #[tokio::test]
    async fn an_agent_with_extra_powers_can_only_be_edited_by_a_person() {
        let path = write_config(vec![agent("chief", false, true), agent("boss", true, false), agent("plain", false, false)]);
        let (tool, approver) = tool_with(&path, true);

        for id in ["chief", "boss"] {
            let err = tool.call(json!({ "action": "update", "id": id, "persona": "hijacked" })).await.unwrap_err();
            assert!(err.to_string().contains("only the user can edit"), "{id} → {err}");
        }
        assert!(approver.asked.lock().unwrap().is_empty());
        assert!(agents_on_disk(&path).iter().all(|a| a.persona != "hijacked"));
        tool.call(json!({ "action": "update", "id": "plain", "persona": "fine" })).await.unwrap();
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|t| t.to_string()).collect()
    }

    /// A tool that knows the real tool names, optionally called by an agent limited to `limit`.
    fn tool_knowing(path: &Path, limit: Option<&[&str]>) -> (Arc<dyn Tool>, Arc<Scripted>) {
        let approver = Arc::new(Scripted { answer: true, asked: Mutex::new(Vec::new()) });
        let known = names(&["read_file", "write_file", "shell", "use_skill", "read_skill_file", "usage_stats", "budget", "generate_document", "delegate_to_agent", "manage_agents"]);
        let tool = ManageAgentsTool::new(path).with_known_tools(known).with_caller_limit(limit.map(names));
        (tool.with_approver(approver.clone()).unwrap(), approver)
    }

    #[tokio::test]
    async fn a_created_agent_defaults_to_the_read_only_tools_and_the_card_says_so() {
        let path = write_config(vec![]);
        let (tool, approver) = tool_knowing(&path, None);
        tool.call(json!({ "action": "create", "id": "reader", "persona": "p" })).await.unwrap();

        let created = &agents_on_disk(&path)[0];
        assert_eq!(created.allowed_tools, Some(names(&SAFE_AGENT_TOOLS)));
        let detail = approver.asked.lock().unwrap()[0].detail.clone();
        assert!(detail.contains("Tools: read_file, use_skill, read_skill_file, usage_stats, budget, generate_document"), "{detail}");
        assert!(!SAFE_AGENT_TOOLS.iter().any(|t| ["shell", "write_file"].contains(t)));
    }

    #[tokio::test]
    async fn a_created_agent_starts_asking_before_every_change_and_never_freer_than_its_creator() {
        let path = write_config(vec![]);
        let (tool, approver) = tool_with(&path, true);
        tool.call(json!({ "action": "create", "id": "fresh", "persona": "p" })).await.unwrap();
        assert_eq!(agents_on_disk(&path)[0].autonomy, 3);
        assert_eq!(agents_on_disk(&path)[0].approval_required, Category::ALL.to_vec(), "every kind of risky action needs a yes at first");
        assert!(!agents_on_disk(&path)[0].can_message_user && !agents_on_disk(&path)[0].can_choose_models, "only a person lets it message them or pick models");
        assert!(!agents_on_disk(&path)[0].can_start_tasks && !agents_on_disk(&path)[0].can_create_workers, "only a person lets it start background work or create workers");
        let detail = approver.asked.lock().unwrap()[0].detail.clone();
        assert!(detail.contains("Autonomy 3: asks before every change"), "{detail}");

        // A creator that only suggests can't hand out more than that, and one at level 4 still starts it at 3.
        let yes = || -> Arc<dyn Approver> { Arc::new(Scripted { answer: true, asked: Mutex::new(Vec::new()) }) };
        let as_caller = |level: u8| ManageAgentsTool::new(&path).with_caller_autonomy(level).with_approver(yes()).unwrap();
        as_caller(2).call(json!({ "action": "create", "id": "careful", "persona": "p" })).await.unwrap();
        assert_eq!(agents_on_disk(&path)[1].autonomy, 2);
        as_caller(4).call(json!({ "action": "create", "id": "bold", "persona": "p" })).await.unwrap();
        assert_eq!(agents_on_disk(&path)[2].autonomy, 3);
    }

    /// An organization to manage inside: `boss` (delegates, manages) over `a` (manages) and `b`; `a` over `a1`, `a2` (delegates) and, under
    /// `a1`, `a11`; and `solo`, outside it.
    fn organization() -> PathBuf {
        let under = |boss: &str, agent: AgentConfig| AgentConfig { reports_to: Some(boss.into()), ..agent };
        write_config(vec![
            agent("boss", true, true),
            under("boss", agent("a", false, true)),
            under("boss", agent("b", false, false)),
            under("a", agent("a1", false, false)),
            under("a", agent("a2", true, false)),
            under("a1", agent("a11", false, false)),
            agent("solo", false, false),
        ])
    }

    fn as_caller(path: &Path, caller: &str) -> (Arc<dyn Tool>, Arc<Scripted>) {
        let approver = Arc::new(Scripted { answer: true, asked: Mutex::new(Vec::new()) });
        (ManageAgentsTool::new(path).with_caller(caller).with_approver(approver.clone()).unwrap(), approver)
    }

    /// `a` at autonomy `level`, with an approver that says `answer` and remembers whether it was asked.
    fn manager_at(path: &Path, level: u8, answer: bool) -> (Arc<dyn Tool>, Arc<Scripted>) {
        let approver = Arc::new(Scripted { answer, asked: Mutex::new(Vec::new()) });
        (ManageAgentsTool::new(path).with_caller("a").with_caller_autonomy(level).with_approver(approver.clone()).unwrap(), approver)
    }

    #[tokio::test]
    async fn a_level_five_manager_changes_its_own_scope_without_a_yes_and_a_level_four_one_still_asks() {
        let path = organization();
        let (tool, approver) = manager_at(&path, 5, false);
        tool.call(json!({ "action": "update", "id": "a11", "role": "Moved" })).await.unwrap();
        tool.call(json!({ "action": "create", "id": "newbie", "persona": "p" })).await.unwrap();
        tool.call(json!({ "action": "delete", "id": "a11" })).await.unwrap();
        assert!(approver.asked.lock().unwrap().is_empty(), "nobody was asked, and the 'no' of the approver never mattered");
        let agents = agents_on_disk(&path);
        assert!(agents.iter().any(|x| x.id == "newbie" && x.reports_to.as_deref() == Some("a")));
        assert!(!agents.iter().any(|x| x.id == "a11"));

        // The same changes at 4 ask, and the 'no' stops them.
        let path = organization();
        let (tool, approver) = manager_at(&path, 4, false);
        let err = tool.call(json!({ "action": "update", "id": "a11", "role": "Moved" })).await.unwrap_err().to_string();
        assert!(err.contains("did not approve"), "{err}");
        assert_eq!(approver.asked.lock().unwrap().len(), 1);
        assert_eq!(agents_on_disk(&path).iter().find(|x| x.id == "a11").unwrap().role, None);
    }

    #[tokio::test]
    async fn level_five_does_not_widen_the_scope_hand_out_powers_or_work_without_a_place_in_the_organization() {
        let path = organization();
        let (tool, approver) = manager_at(&path, 5, true);
        // Out of reach: a sibling's branch, its own superior, itself.
        for id in ["b", "boss", "a", "solo"] {
            let err = tool.call(json!({ "action": "update", "id": id, "persona": "x" })).await.unwrap_err().to_string();
            assert!(err.contains("is not yours") || err.contains("doesn't report"), "{id}: {err}");
        }
        // A power is never a tool in a list, here or at 4.
        let err = tool.call(json!({ "action": "update", "id": "a1", "allowed_tools": ["manage_agents"] })).await.unwrap_err().to_string();
        assert!(err.contains("only the user can give an agent that power"), "{err}");
        let err = tool.call(json!({ "action": "create", "id": "n", "persona": "p", "allowed_tools": ["delegate_to_agent"] })).await.unwrap_err().to_string();
        assert!(err.contains("only the user can give an agent that power"), "{err}");
        // Refused changes never reach the question either, since they fail before it.
        assert!(approver.asked.lock().unwrap().is_empty());

        // A tool with no caller (the use from before the organization) still waits for a person, whatever the level says.
        let approver = Arc::new(Scripted { answer: false, asked: Mutex::new(Vec::new()) });
        let tool = ManageAgentsTool::new(&path).with_caller_autonomy(5).with_approver(approver.clone()).unwrap();
        let err = tool.call(json!({ "action": "create", "id": "n2", "persona": "p" })).await.unwrap_err().to_string();
        assert!(err.contains("did not approve"), "{err}");
        assert_eq!(approver.asked.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_manager_at_level_five_with_no_way_to_ask_still_works_where_level_four_refuses() {
        let path = organization();
        let no_approver = |level: u8| ManageAgentsTool::new(&path).with_caller("a").with_caller_autonomy(level);
        let err = no_approver(4).call(json!({ "action": "update", "id": "a11", "role": "R" })).await.unwrap_err().to_string();
        assert!(err.contains("this channel can't ask"), "{err}");
        no_approver(5).call(json!({ "action": "update", "id": "a11", "role": "R" })).await.unwrap();
        assert_eq!(agents_on_disk(&path).iter().find(|x| x.id == "a11").unwrap().role.as_deref(), Some("R"));
    }

    #[tokio::test]
    async fn a_manager_changes_and_removes_only_the_agents_below_it_and_never_itself() {
        let path = organization();
        let (tool, approver) = as_caller(&path, "a");
        for (target, why) in [("b", "a peer"), ("boss", "its superior"), ("a", "itself"), ("solo", "someone outside the organization")] {
            for action in ["update", "delete"] {
                let err = tool.call(json!({ "action": action, "id": target, "persona": "p" })).await.unwrap_err().to_string();
                assert!(err.contains("is not yours to") && err.contains("report to you"), "{action} {target} ({why}): {err}");
            }
        }
        assert!(approver.asked.lock().unwrap().is_empty(), "a refused request never costs the person a prompt");

        // Direct and indirect subordinates are its to change.
        tool.call(json!({ "action": "update", "id": "a1", "persona": "new persona" })).await.unwrap();
        tool.call(json!({ "action": "update", "id": "a11", "persona": "deeper" })).await.unwrap();
        tool.call(json!({ "action": "delete", "id": "a11" })).await.unwrap();
        let agents = agents_on_disk(&path);
        assert_eq!(agents.iter().find(|x| x.id == "a1").unwrap().persona, "new persona");
        assert!(!agents.iter().any(|x| x.id == "a11"));
    }

    #[tokio::test]
    async fn a_subordinate_with_a_power_is_the_managers_only_if_the_manager_has_that_power() {
        let path = organization();
        // `a2` can delegate; `a` can't.
        for action in ["update", "delete"] {
            let err = as_caller(&path, "a").0.call(json!({ "action": action, "id": "a2", "persona": "p" })).await.unwrap_err().to_string();
            assert!(err.contains("can delegate to other agents and you can't"), "{err}");
        }
        // `boss` can, and `a2` is below it through `a`.
        as_caller(&path, "boss").0.call(json!({ "action": "update", "id": "a2", "persona": "changed by the boss" })).await.unwrap();
        assert_eq!(agents_on_disk(&path).iter().find(|x| x.id == "a2").unwrap().persona, "changed by the boss");
        // Even then no power is granted: the flags stay as they were.
        assert!(agents_on_disk(&path).iter().find(|x| x.id == "a2").unwrap().can_delegate_to_agents);
    }

    #[tokio::test]
    async fn a_new_agent_reports_to_its_creator_or_to_someone_below_it() {
        let path = organization();
        let (tool, approver) = as_caller(&path, "a");
        tool.call(json!({ "action": "create", "id": "fresh", "persona": "p", "role": "  Tester " })).await.unwrap();
        tool.call(json!({ "action": "create", "id": "deep", "persona": "p", "reports_to": "a1" })).await.unwrap();
        let agents = agents_on_disk(&path);
        let fresh = agents.iter().find(|x| x.id == "fresh").unwrap();
        assert_eq!((fresh.reports_to.as_deref(), fresh.role.as_deref()), (Some("a"), Some("Tester")));
        assert_eq!(agents.iter().find(|x| x.id == "deep").unwrap().reports_to.as_deref(), Some("a1"));
        let card = approver.asked.lock().unwrap()[0].detail.clone();
        assert!(card.contains("Role: Tester") && card.contains("Reports to: a"), "{card}");

        for outside in ["b", "boss", "solo"] {
            let err = tool.call(json!({ "action": "create", "id": "x", "persona": "p", "reports_to": outside })).await.unwrap_err().to_string();
            assert!(err.contains("can't be a superior here"), "{outside}: {err}");
        }
        let long = "r".repeat(81);
        assert!(tool.call(json!({ "action": "create", "id": "x", "persona": "p", "role": long })).await.unwrap_err().to_string().contains("role must be"));
    }

    #[tokio::test]
    async fn moving_an_agent_stays_inside_the_managers_reach_and_never_under_its_own_branch() {
        let path = organization();
        let (tool, approver) = as_caller(&path, "a");
        // `a11` is below `a1`, so `a1` can't be put under it.
        let err = tool.call(json!({ "action": "update", "id": "a1", "reports_to": "a11" })).await.unwrap_err().to_string();
        assert!(err.contains("under itself or one of its own subordinates"), "inside its own branch: {err}");
        tool.call(json!({ "action": "update", "id": "a11", "reports_to": "a2", "role": "Moved" })).await.unwrap();
        let moved = agents_on_disk(&path).into_iter().find(|x| x.id == "a11").unwrap();
        assert_eq!((moved.reports_to.as_deref(), moved.role.as_deref()), (Some("a2"), Some("Moved")));
        let card = approver.asked.lock().unwrap()[0].detail.clone();
        assert!(card.contains("Reports to: a1 → a2") && card.contains("Role: (none) → Moved"), "{card}");

        let err = tool.call(json!({ "action": "update", "id": "a1", "reports_to": "b" })).await.unwrap_err().to_string();
        assert!(err.contains("can't be a superior here"), "out of reach: {err}");
        let err = tool.call(json!({ "action": "update", "id": "a1", "reports_to": "a1" })).await.unwrap_err().to_string();
        assert!(err.contains("under itself"), "{err}");
        // An empty role clears it.
        tool.call(json!({ "action": "update", "id": "a11", "role": "" })).await.unwrap();
        assert_eq!(agents_on_disk(&path).into_iter().find(|x| x.id == "a11").unwrap().role, None);
    }

    #[tokio::test]
    async fn a_manager_lists_only_its_own_scope() {
        let path = organization();
        let listed = |caller: &str| {
            let tool = ManageAgentsTool::new(&path).with_caller(caller);
            async move {
                let result = tool.call(json!({ "action": "list" })).await.unwrap();
                result["agents"].as_array().unwrap().iter().map(|a| a["id"].as_str().unwrap().to_string()).collect::<Vec<_>>()
            }
        };
        assert_eq!(listed("a").await, ["a1", "a2", "a11"]);
        assert_eq!(listed("boss").await, ["a", "b", "a1", "a2", "a11"]);
        assert!(listed("a11").await.is_empty());
        assert!(listed("solo").await.is_empty());
        // With no caller (the rule from before the organization) it lists everything.
        let all = ManageAgentsTool::new(&path).call(json!({ "action": "list" })).await.unwrap();
        assert_eq!(all["agents"].as_array().unwrap().len(), 7);
        assert_eq!(all["agents"][3]["reports_to"], json!("a"));
    }

    #[tokio::test]
    async fn deleting_an_agent_hands_its_reports_to_its_superior() {
        let path = write_config(vec![
            agent("chief", true, true),
            AgentConfig { reports_to: Some("chief".into()), ..agent("lead", false, false) },
            AgentConfig { reports_to: Some("lead".into()), ..agent("dev", false, false) },
        ]);
        let (tool, _) = tool_with(&path, true);
        tool.call(json!({ "action": "delete", "id": "lead" })).await.unwrap();
        let agents = agents_on_disk(&path);
        assert_eq!(agents.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(), ["chief", "dev"]);
        assert_eq!(agents[1].reports_to.as_deref(), Some("chief"));
    }

    #[tokio::test]
    async fn a_requested_tool_list_is_saved_deduplicated_and_shown() {
        let path = write_config(vec![]);
        let (tool, approver) = tool_knowing(&path, None);
        tool.call(json!({ "action": "create", "id": "ops", "persona": "p", "allowed_tools": [" shell ", "read_file", "shell"] })).await.unwrap();
        assert_eq!(agents_on_disk(&path)[0].allowed_tools, Some(names(&["shell", "read_file"])));
        assert!(approver.asked.lock().unwrap()[0].detail.contains("Tools: shell, read_file"));

        // An explicit empty list is a text-only agent, not "all tools".
        tool.call(json!({ "action": "create", "id": "talker", "persona": "p", "allowed_tools": [] })).await.unwrap();
        assert_eq!(agents_on_disk(&path)[1].allowed_tools, Some(Vec::new()));
        assert!(approver.asked.lock().unwrap()[1].detail.contains("Tools: (none)"));
    }

    #[tokio::test]
    async fn bad_tool_lists_are_refused_before_asking_anyone() {
        let path = write_config(vec![agent("plain", false, false)]);
        let (tool, approver) = tool_knowing(&path, Some(&["read_file", "use_skill"]));
        let cases = [
            (json!({ "action": "create", "id": "n", "persona": "p", "allowed_tools": ["teleport"] }), "unknown tool 'teleport'"),
            (json!({ "action": "create", "id": "n", "persona": "p", "allowed_tools": ["manage_agents"] }), "only the user can give"),
            (json!({ "action": "create", "id": "n", "persona": "p", "allowed_tools": ["delegate_to_agent"] }), "only the user can give"),
            // The caller only has read_file/use_skill, so it can't hand out `shell`.
            (json!({ "action": "create", "id": "n", "persona": "p", "allowed_tools": ["shell"] }), "isn't one of yours"),
            (json!({ "action": "update", "id": "plain", "allowed_tools": ["write_file"] }), "isn't one of yours"),
            (json!({ "action": "create", "id": "n", "persona": "p", "allowed_tools": "shell" }), "must be an array"),
            (json!({ "action": "create", "id": "n", "persona": "p", "allowed_tools": [1] }), "only strings"),
        ];
        for (args, expected) in cases {
            let err = tool.call(args.clone()).await.unwrap_err();
            assert!(err.to_string().contains(expected), "{args} → {err}");
        }
        assert!(approver.asked.lock().unwrap().is_empty());
        assert_eq!(agents_on_disk(&path).len(), 1);
    }

    #[tokio::test]
    async fn the_default_tools_never_exceed_what_the_caller_has() {
        let path = write_config(vec![]);
        let (tool, _) = tool_knowing(&path, Some(&["read_file", "shell"]));
        tool.call(json!({ "action": "create", "id": "child", "persona": "p" })).await.unwrap();
        // The safe set is cut down to the caller's own limit: `shell` is not in it, so it isn't added.
        assert_eq!(agents_on_disk(&path)[0].allowed_tools, Some(names(&["read_file"])));
    }

    #[tokio::test]
    async fn update_replaces_the_tool_list_and_shows_old_and_new() {
        let path = write_config(vec![AgentConfig { allowed_tools: Some(names(&["read_file"])), ..agent("writer", false, false) }, agent("open", false, false)]);
        let (tool, approver) = tool_knowing(&path, None);

        tool.call(json!({ "action": "update", "id": "writer", "allowed_tools": ["read_file", "write_file"] })).await.unwrap();
        tool.call(json!({ "action": "update", "id": "open", "allowed_tools": ["read_file"] })).await.unwrap();
        // Leaving it out keeps the list.
        tool.call(json!({ "action": "update", "id": "writer", "persona": "new persona" })).await.unwrap();

        let agents = agents_on_disk(&path);
        assert_eq!(agents[0].allowed_tools, Some(names(&["read_file", "write_file"])));
        assert_eq!(agents[1].allowed_tools, Some(names(&["read_file"])));
        let asked = approver.asked.lock().unwrap();
        assert!(asked[0].detail.contains("Tools: read_file → read_file, write_file"), "{}", asked[0].detail);
        assert!(asked[1].detail.contains("Tools: (all tools) → read_file"), "{}", asked[1].detail);
    }

    #[tokio::test]
    async fn list_shows_each_agents_tools_and_what_can_be_granted() {
        let path = write_config(vec![agent("open", false, false)]);
        let (tool, _) = tool_knowing(&path, Some(&["read_file", "shell", "manage_agents"]));
        let result = tool.call(json!({ "action": "list" })).await.unwrap();
        assert_eq!(result["agents"][0]["allowed_tools"], Value::Null);
        // Only what the caller has, and never the two flag-gated tools.
        assert_eq!(result["grantable_tool_names"], json!(["read_file", "shell"]));
        assert_eq!(result["default_tools_for_new_agents"], json!(["read_file"]));
    }

    fn ssh_host(id: &str, agents: &[&str]) -> SshHostConfig {
        SshHostConfig {
            id: id.into(),
            host: "example.com".into(),
            user: "deploy".into(),
            port: 22,
            identity_file: None,
            enabled: true,
            agents: agents.iter().map(|a| a.to_string()).collect(),
            require_approval: false,
        }
    }

    fn hosts_on_disk(path: &Path) -> Vec<SshHostConfig> {
        load_config_from_path(path, true).unwrap().ssh_hosts
    }

    fn with_hosts(path: &Path, hosts: Vec<SshHostConfig>) {
        let mut config = load_config_from_path(path, true).unwrap();
        config.ssh_hosts = hosts;
        save_config(path, &config).unwrap();
    }

    #[tokio::test]
    async fn delete_removes_only_the_target_after_approval_and_shows_what_is_lost() {
        let path = write_config(vec![agent("chief", false, true), agent("temp", false, false), agent("keep", false, false)]);
        with_hosts(&path, vec![ssh_host("only-temp", &["temp"]), ssh_host("shared", &["temp", "keep"]), ssh_host("everyone", &[])]);
        let (tool, approver) = tool_with(&path, true);

        let result = tool.call(json!({ "action": "delete", "id": "temp" })).await.unwrap();

        assert!(result["message"].as_str().unwrap().contains("'temp' deleted"));
        let ids: Vec<String> = agents_on_disk(&path).into_iter().map(|a| a.id).collect();
        assert_eq!(ids, ["chief", "keep"]);
        // No dangling reference, and a host that was only for `temp` is switched off, not opened to all.
        let hosts = hosts_on_disk(&path);
        assert_eq!((hosts[0].agents.len(), hosts[0].enabled), (0, false));
        assert_eq!((hosts[1].agents.clone(), hosts[1].enabled), (vec!["keep".to_string()], true));
        assert_eq!((hosts[2].agents.len(), hosts[2].enabled), (0, true));

        let asked = approver.asked.lock().unwrap();
        assert_eq!((asked[0].action.as_str(), asked[0].target.as_str()), ("delete_agent", "temp"));
        let detail = &asked[0].detail;
        assert!(detail.contains("cannot be undone") && detail.contains("persona of temp"), "{detail}");
        assert!(detail.contains("'only-temp' was only for this agent, so it will be switched off"), "{detail}");
        assert!(detail.contains("'shared' will stop listing this agent"), "{detail}");
        assert!(!detail.contains("everyone"), "{detail}");
    }

    #[tokio::test]
    async fn a_delete_that_cannot_succeed_is_refused_before_asking_anyone() {
        let path = write_config(vec![agent("chief", false, true), agent("boss", true, false), agent("plain", false, false)]);
        let (tool, approver) = tool_with(&path, true);
        let cases = [
            (json!({ "action": "delete", "id": "ghost" }), "no agent named"),
            (json!({ "action": "delete", "id": "chief" }), "only the user can delete"),
            (json!({ "action": "delete", "id": "boss" }), "only the user can delete"),
            (json!({ "action": "delete" }), "'id'"),
        ];
        for (args, expected) in cases {
            let err = tool.call(args.clone()).await.unwrap_err();
            assert!(err.to_string().contains(expected), "{args} → {err}");
        }
        assert!(approver.asked.lock().unwrap().is_empty());
        assert_eq!(agents_on_disk(&path).len(), 3);
    }

    #[tokio::test]
    async fn a_denied_delete_writes_nothing_and_no_approver_refuses() {
        let path = write_config(vec![agent("plain", false, false)]);
        with_hosts(&path, vec![ssh_host("h", &["plain"])]);
        let (tool, approver) = tool_with(&path, false);
        let err = tool.call(json!({ "action": "delete", "id": "plain" })).await.unwrap_err();
        assert!(err.to_string().contains("did not approve"));
        assert_eq!(approver.asked.lock().unwrap().len(), 1);
        let err = ManageAgentsTool::new(&path).call(json!({ "action": "delete", "id": "plain" })).await.unwrap_err();
        assert!(err.to_string().contains("can't ask"));
        assert_eq!(agents_on_disk(&path).len(), 1);
        assert_eq!(hosts_on_disk(&path)[0], ssh_host("h", &["plain"]));
    }

    #[tokio::test]
    async fn a_delete_is_applied_to_what_is_on_disk_after_the_prompt() {
        let path = write_config(vec![agent("temp", false, false)]);
        // While the prompt is open the user adds another agent and restricts a new SSH host to `temp`.
        struct EditsWhileAsking(PathBuf);
        #[async_trait]
        impl Approver for EditsWhileAsking {
            async fn approve(&self, _request: ApprovalRequest) -> bool {
                let mut config = load_config_from_path(&self.0, true).unwrap();
                config.agents.push(agent("added-meanwhile", false, false));
                config.ssh_hosts.push(ssh_host("added-host", &["temp"]));
                save_config(&self.0, &config).unwrap();
                true
            }
        }
        let tool = ManageAgentsTool::new(&path).with_approver(Arc::new(EditsWhileAsking(path.clone()))).unwrap();

        tool.call(json!({ "action": "delete", "id": "temp" })).await.unwrap();

        let ids: Vec<String> = agents_on_disk(&path).into_iter().map(|a| a.id).collect();
        assert_eq!(ids, ["added-meanwhile"]);
        assert_eq!((hosts_on_disk(&path)[0].agents.len(), hosts_on_disk(&path)[0].enabled), (0, false));
    }

    #[tokio::test]
    async fn the_change_is_applied_to_what_is_on_disk_after_the_prompt_not_before() {
        let path = write_config(vec![]);
        // The user answers "yes" — but while the prompt was open, another agent was saved elsewhere.
        struct EditsWhileAsking(PathBuf);
        #[async_trait]
        impl Approver for EditsWhileAsking {
            async fn approve(&self, _request: ApprovalRequest) -> bool {
                let mut config = load_config_from_path(&self.0, true).unwrap();
                config.agents.push(agent("added-meanwhile", false, false));
                save_config(&self.0, &config).unwrap();
                true
            }
        }
        let tool = ManageAgentsTool::new(&path).with_approver(Arc::new(EditsWhileAsking(path.clone()))).unwrap();

        tool.call(json!({ "action": "create", "id": "mine", "persona": "p" })).await.unwrap();

        let ids: Vec<String> = agents_on_disk(&path).into_iter().map(|a| a.id).collect();
        assert_eq!(ids, ["added-meanwhile", "mine"]);
    }

    #[tokio::test]
    async fn a_name_taken_during_the_prompt_is_caught_after_it() {
        let path = write_config(vec![]);
        struct TakesTheNameWhileAsking(PathBuf);
        #[async_trait]
        impl Approver for TakesTheNameWhileAsking {
            async fn approve(&self, _request: ApprovalRequest) -> bool {
                let mut config = load_config_from_path(&self.0, true).unwrap();
                config.agents.push(agent("mine", false, false));
                save_config(&self.0, &config).unwrap();
                true
            }
        }
        let tool = ManageAgentsTool::new(&path).with_approver(Arc::new(TakesTheNameWhileAsking(path.clone()))).unwrap();
        let err = tool.call(json!({ "action": "create", "id": "mine", "persona": "overwrites?" })).await.unwrap_err();
        assert!(err.to_string().contains("already exists"));
        assert_eq!(agents_on_disk(&path)[0].persona, "persona of mine");
    }

    #[tokio::test]
    async fn list_needs_no_approval_and_shows_flags_and_providers() {
        let path = write_config(vec![agent("chief", true, true), agent("plain", false, false)]);
        let long = AgentConfig { persona: "word ".repeat(200), ..agent("wordy", false, false) };
        let mut config = load_config_from_path(&path, true).unwrap();
        config.agents.push(long);
        save_config(&path, &config).unwrap();

        let result = ManageAgentsTool::new(&path).call(json!({ "action": "list" })).await.unwrap();

        assert_eq!(result["available_provider_ids"], json!(["local"]));
        let agents = result["agents"].as_array().unwrap();
        assert_eq!(agents.len(), 3);
        assert_eq!(agents[0]["can_manage_agents"], json!(true));
        assert!(agents[2]["persona"].as_str().unwrap().ends_with('…'));
    }

    #[test]
    fn the_spec_offers_no_way_to_grant_powers() {
        let spec = ManageAgentsTool::new("unused").spec();
        assert_eq!(spec.parameters["properties"]["action"]["enum"], json!(["list", "create", "update", "delete"]));
        let props = spec.parameters["properties"].as_object().unwrap();
        assert!(!props.contains_key("can_manage_agents") && !props.contains_key("can_delegate_to_agents"));
        assert!(props.contains_key("allowed_tools"));
    }
}
