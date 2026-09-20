//! `manage_agents` (P46): lets a "chief" agent list, create and edit the other named agents in
//! `config.toml`. Lives here rather than in `warden-core` because it works on `AgentConfig` and
//! reads/writes the config file, exactly like `usage::UsageStatsTool` does for its data.
//!
//! The safety story is three rules, all enforced here and not left to the model:
//! 1. Only an agent that opted in (`AgentConfig.can_manage_agents`) is ever given the tool — the
//!    channels (desktop, CLI) attach it per turn, the same way they do for `delegate_to_agent`.
//! 2. Every `create`/`update` waits for a human "yes" through the `Approver`, and the request shows
//!    the *whole* persona that would be saved. Without an approver it refuses.
//! 3. No agent can grant a permission: created agents always start with `can_manage_agents` and
//!    `can_delegate_to_agents` off, and `update` refuses to touch an agent that has either one
//!    (that includes the caller itself). Those flags are switched on by a person only.
//!
//! There is deliberately no `delete`, same reasoning as `manage_skill`: a model mistake can only
//! overwrite, never silently lose, something the user set up.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use warden_core::tool::{ApprovalRequest, Approver, Tool, ToolSpec};

use crate::{load_config_from_path, save_config, AgentConfig, FileConfig};

const MAX_ID_CHARS: usize = 64;
/// Small enough that the approval card can show the whole persona — a reviewer must see everything
/// that will be saved, not a truncated preview.
const MAX_PERSONA_CHARS: usize = 4000;
const LIST_PERSONA_PREVIEW_CHARS: usize = 200;
const APPROVAL_TIMEOUT: Duration = Duration::from_secs(120);

/// What the model asked for, parsed once so it can be checked before asking a human and then
/// re-applied to a freshly read config after they answer.
#[derive(Debug, Clone, PartialEq)]
enum Change {
    Create { id: String, persona: String, provider_id: Option<String> },
    /// `None` = leave as is. `provider_id: Some(None)` = clear it (an empty string from the model).
    Update { id: String, persona: Option<String>, provider_id: Option<Option<String>> },
}

impl Change {
    fn action(&self) -> &'static str {
        match self {
            Change::Create { .. } => "create_agent",
            Change::Update { .. } => "update_agent",
        }
    }

    fn id(&self) -> &str {
        match self {
            Change::Create { id, .. } | Change::Update { id, .. } => id,
        }
    }
}

#[derive(Clone)]
pub struct ManageAgentsTool {
    config_path: PathBuf,
    approver: Option<Arc<dyn Approver>>,
    approval_timeout: Duration,
}

impl ManageAgentsTool {
    pub fn new(config_path: impl Into<PathBuf>) -> Self {
        Self { config_path: config_path.into(), approver: None, approval_timeout: APPROVAL_TIMEOUT }
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
                })
            })
            .collect();
        let providers: Vec<&str> = config.providers.iter().map(|p| p.id.as_str()).collect();
        Ok(json!({ "agents": agents, "available_provider_ids": providers }))
    }

    async fn change(&self, change: Change) -> anyhow::Result<Value> {
        // Check first, so a request that can't succeed never costs the user a prompt.
        let (_, detail) = plan(&self.load()?, &change)?;

        let Some(approver) = &self.approver else {
            anyhow::bail!(
                "creating or changing agents needs the user's approval, and this channel can't ask for it \
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
        config.agents = plan(&config, &change)?.0;
        save_config(&self.config_path, &config)?;
        let verb = if matches!(change, Change::Create { .. }) { "created" } else { "updated" };
        Ok(json!({
            "status": "ok",
            "message": format!(
                "Agent '{}' {verb}. It becomes available from the user's next message (agents are read at the start of each turn).",
                change.id()
            ),
        }))
    }
}

/// The agent list `config` would have after `change`, or why that can't be done — plus what a human
/// needs to see to approve it. Pure (no I/O) so the same checks run before and after the prompt.
fn plan(config: &FileConfig, change: &Change) -> anyhow::Result<(Vec<AgentConfig>, String)> {
    let mut updated = config.agents.clone();
    let detail = match change {
        Change::Create { id, persona, provider_id } => {
            check_id(id)?;
            check_persona(persona)?;
            check_provider(config, provider_id.as_deref())?;
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
            });
            format!(
                "New agent '{id}'\nModel provider: {}\nCannot delegate or manage agents (only you can turn that on).\n\nPersona:\n{persona}",
                provider_label(provider_id.as_deref())
            )
        }
        Change::Update { id, persona, provider_id } => {
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
            if persona.is_none() && provider_id.is_none() {
                anyhow::bail!("nothing to change — pass 'persona' and/or 'provider_id'");
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
            if let Some(new_persona) = persona {
                check_persona(new_persona)?;
                lines.push(format!("\nOld persona:\n{}\n\nNew persona:\n{new_persona}", current.persona));
                updated[index].persona = new_persona.clone();
            }
            lines.join("\n")
        }
    };
    Ok((updated, detail))
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

#[async_trait]
impl Tool for ManageAgentsTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "manage_agents".to_string(),
            description: "List, create or edit the user's named agents (each is a persona, optionally with its own \
                          model). Use it only when the user asks to create or change an agent. Every create/update \
                          is shown to the user, who must approve it; you cannot delete agents or give any agent the \
                          power to delegate or manage other agents. A new agent is usable from the user's next \
                          message. Start with 'list' to see what exists."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["list", "create", "update"] },
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
        if !matches!(action, "create" | "update") {
            anyhow::bail!("unknown action '{action}' — use 'list', 'create' or 'update'");
        }
        let id = args.get("id").and_then(Value::as_str).ok_or_else(|| anyhow::anyhow!("missing required 'id' argument"))?.to_string();
        let persona = args.get("persona").and_then(Value::as_str).map(str::to_string);
        let change = match action {
            "create" => {
                let persona = persona.ok_or_else(|| anyhow::anyhow!("missing required 'persona' argument"))?;
                Change::Create { id, persona, provider_id: provider_arg(&args).flatten() }
            }
            _ => Change::Update { id, persona, provider_id: provider_arg(&args) },
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
        AgentConfig { id: id.into(), persona: format!("persona of {id}"), provider_id: None, can_delegate_to_agents: delegate, can_manage_agents: manage }
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
    fn the_spec_offers_no_way_to_delete_or_grant_powers() {
        let spec = ManageAgentsTool::new("unused").spec();
        assert_eq!(spec.parameters["properties"]["action"]["enum"], json!(["list", "create", "update"]));
        let props = spec.parameters["properties"].as_object().unwrap();
        assert!(!props.contains_key("can_manage_agents") && !props.contains_key("can_delegate_to_agents"));
    }
}
