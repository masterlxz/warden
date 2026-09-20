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
use warden_core::tool::{ApprovalRequest, Approver, Tool, ToolSpec};

use crate::{load_config_from_path, remove_agent_from, save_config, AgentConfig, FileConfig, SshHostConfig, SAFE_AGENT_TOOLS};

const MAX_ID_CHARS: usize = 64;
/// Tools that follow `AgentConfig.can_delegate_to_agents`/`can_manage_agents`, never a tool list.
const FLAG_GATED_TOOLS: [&str; 2] = ["delegate_to_agent", "manage_agents"];
/// Small enough that the approval card can show the whole persona — a reviewer must see everything
/// that will be saved, not a truncated preview.
const MAX_PERSONA_CHARS: usize = 4000;
const LIST_PERSONA_PREVIEW_CHARS: usize = 200;
const APPROVAL_TIMEOUT: Duration = Duration::from_secs(120);

/// What the model asked for, parsed once so it can be checked before asking a human and then
/// re-applied to a freshly read config after they answer.
#[derive(Debug, Clone, PartialEq)]
enum Change {
    /// `allowed_tools: None` = the safe default.
    Create { id: String, persona: String, provider_id: Option<String>, allowed_tools: Option<Vec<String>> },
    /// `None` = leave as is. `provider_id: Some(None)` = clear it (an empty string from the model).
    Update { id: String, persona: Option<String>, provider_id: Option<Option<String>>, allowed_tools: Option<Vec<String>> },
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
}

#[derive(Clone)]
pub struct ManageAgentsTool {
    config_path: PathBuf,
    approver: Option<Arc<dyn Approver>>,
    approval_timeout: Duration,
    rules: ToolRules,
}

impl ManageAgentsTool {
    pub fn new(config_path: impl Into<PathBuf>) -> Self {
        Self { config_path: config_path.into(), approver: None, approval_timeout: APPROVAL_TIMEOUT, rules: ToolRules::default() }
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
        let agents: Vec<Value> = config
            .agents
            .iter()
            .map(|a| {
                json!({
                    "id": a.id,
                    "persona": preview(&a.persona, LIST_PERSONA_PREVIEW_CHARS),
                    "provider_id": a.provider_id,
                    "can_delegate_to_agents": a.can_delegate_to_agents,
                    "can_manage_agents": a.can_manage_agents,
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

    async fn change(&self, change: Change) -> anyhow::Result<Value> {
        // Check first, so a request that can't succeed never costs the user a prompt.
        let Planned { detail, .. } = plan(&self.load()?, &change, &self.rules)?;

        let Some(approver) = &self.approver else {
            anyhow::bail!(
                "creating, changing or deleting agents needs the user's approval, and this channel can't ask for it \
                 (use the desktop app or the interactive CLI)"
            );
        };
        let request = ApprovalRequest { target: change.id().to_string(), action: change.action().to_string(), detail };
        let approved = tokio::time::timeout(self.approval_timeout, approver.approve(request)).await.unwrap_or(false);
        if !approved {
            anyhow::bail!("the user did not approve this change to agent '{}'", change.id());
        }

        // The prompt can sit open for a while and the user may have edited agents meanwhile, so
        // re-check and apply against a fresh read rather than the one from before the wait.
        let mut config = self.load()?;
        let planned = plan(&config, &change, &self.rules)?;
        config.agents = planned.agents;
        config.ssh_hosts = planned.ssh_hosts;
        save_config(&self.config_path, &config)?;
        let message = match change {
            Change::Delete { .. } => format!("Agent '{}' deleted.", change.id()),
            Change::Create { .. } | Change::Update { .. } => format!(
                "Agent '{}' {}. It becomes available from the user's next message (agents are read at the start of each turn).",
                change.id(),
                if matches!(change, Change::Create { .. }) { "created" } else { "updated" }
            ),
        };
        Ok(json!({ "status": "ok", "message": message }))
    }
}

/// What `config` would look like after `change`, or why that can't be done — plus what a human
/// needs to see to approve it. Pure (no I/O) so the same checks run before and after the prompt.
fn plan(config: &FileConfig, change: &Change, rules: &ToolRules) -> anyhow::Result<Planned> {
    let mut updated = config.agents.clone();
    let mut ssh_hosts = config.ssh_hosts.clone();
    let detail = match change {
        Change::Create { id, persona, provider_id, allowed_tools } => {
            check_id(id)?;
            check_persona(persona)?;
            check_provider(config, provider_id.as_deref())?;
            let tools = match allowed_tools {
                Some(requested) => rules.check(requested)?,
                None => rules.default_tools(),
            };
            if config.agents.iter().any(|a| &a.id == id) {
                anyhow::bail!("an agent named '{id}' already exists — pick another name, or use action 'update'");
            }
            updated.push(AgentConfig {
                id: id.clone(),
                persona: persona.clone(),
                provider_id: provider_id.clone(),
                // Never granted from here; only a person turns these on.
                can_delegate_to_agents: false,
                can_manage_agents: false,
                allowed_tools: Some(tools.clone()),
            });
            format!(
                "New agent '{id}'\nModel provider: {}\nTools: {}\nCannot delegate or manage agents (only you can turn that on).\n\nPersona:\n{persona}",
                provider_label(provider_id.as_deref()),
                tools_label(Some(&tools))
            )
        }
        Change::Update { id, persona, provider_id, allowed_tools } => {
            let Some(index) = config.agents.iter().position(|a| &a.id == id) else {
                anyhow::bail!("no agent named '{id}' — use action 'list' to see the existing ones");
            };
            let current = &config.agents[index];
            if current.can_delegate_to_agents || current.can_manage_agents {
                anyhow::bail!(
                    "agent '{id}' can delegate or manage agents, so only the user can edit it (Settings or /agents) — \
                     an agent may not change one with more power than a plain agent"
                );
            }
            if persona.is_none() && provider_id.is_none() && allowed_tools.is_none() {
                anyhow::bail!("nothing to change — pass 'persona', 'provider_id' and/or 'allowed_tools'");
            }
            let mut lines = vec![format!("Change agent '{id}'")];
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
            lines.join("\n")
        }
        Change::Delete { id } => {
            let Some(current) = config.agents.iter().find(|a| &a.id == id) else {
                anyhow::bail!("no agent named '{id}' — use action 'list' to see the existing ones");
            };
            if current.can_delegate_to_agents || current.can_manage_agents {
                anyhow::bail!(
                    "agent '{id}' can delegate or manage agents, so only the user can delete it (Settings or /agents remove) — \
                     an agent may not remove one with more power than a plain agent"
                );
            }
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

fn tools_label(tools: Option<&[String]>) -> String {
    match tools {
        None => "(all tools)".to_string(),
        Some([]) => "(none)".to_string(),
        Some(list) => list.join(", "),
    }
}

fn check_id(id: &str) -> anyhow::Result<()> {
    if id.trim().is_empty() || id != id.trim() {
        anyhow::bail!("agent id must not be empty or start/end with spaces");
    }
    if id.chars().count() > MAX_ID_CHARS || id.chars().any(char::is_control) {
        anyhow::bail!("agent id must be at most {MAX_ID_CHARS} characters, with no control characters");
    }
    Ok(())
}

fn check_persona(persona: &str) -> anyhow::Result<()> {
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
        Some(id) if !config.providers.iter().any(|p| p.id == id) => {
            let known: Vec<&str> = config.providers.iter().map(|p| p.id.as_str()).collect();
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
                          agents, and you cannot delete one that has it. A new agent starts with read-only tools \
                          unless you list others, and is usable from the user's next message. Start with 'list' to \
                          see what exists."
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
                Change::Create { id, persona, provider_id: provider_arg(&args).flatten(), allowed_tools: allowed_tools_arg(&args)? }
            }
            _ => Change::Update { id, persona, provider_id: provider_arg(&args), allowed_tools: allowed_tools_arg(&args)? },
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
        AgentConfig { id: id.into(), persona: format!("persona of {id}"), provider_id: None, can_delegate_to_agents: delegate, can_manage_agents: manage, allowed_tools: None }
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
    async fn a_model_cannot_smuggle_flags_in_through_extra_arguments() {
        let path = write_config(vec![]);
        let (tool, _) = tool_with(&path, true);
        tool.call(json!({ "action": "create", "id": "sneaky", "persona": "p", "can_manage_agents": true, "can_delegate_to_agents": true }))
            .await
            .unwrap();
        let created = &agents_on_disk(&path)[0];
        assert!(!created.can_delegate_to_agents && !created.can_manage_agents);
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
        let known = names(&["read_file", "write_file", "shell", "use_skill", "read_skill_file", "usage_stats", "generate_document", "delegate_to_agent", "manage_agents"]);
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
        assert!(detail.contains("Tools: read_file, use_skill, read_skill_file, usage_stats, generate_document"), "{detail}");
        assert!(!SAFE_AGENT_TOOLS.iter().any(|t| ["shell", "write_file"].contains(t)));
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
