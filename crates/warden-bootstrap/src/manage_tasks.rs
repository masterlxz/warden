//! `manage_tasks` (P92): lets an agent list, create, edit and delete the scheduled tasks in
//! `config.toml` — "every weekday at 8 summarize the news" said in a chat becomes a `[[tasks]]` entry.
//! Built like `manage_agents`, with the same safety story, enforced here and not left to the model:
//! 1. Only an agent that opted in (`AgentConfig.can_manage_tasks`, switched on by a person) gets the
//!    tool (`agent_scope::scope_to_agent`).
//! 2. Every create/update/delete waits for a human "yes" through the `Approver`, and the card shows
//!    the whole task: agent, schedule, first run and the full prompt. Without an approver it refuses —
//!    so a scheduled run, which has none, can never schedule more work by itself.
//! 3. Nobody hands out more than they have: an agent limited to some tools can only schedule an agent
//!    whose tools fit in its own, never one with every tool, no agent at all (every tool), or one that
//!    can reach other agents (delegate, message).
//!
//! A task only runs on the hub started with `--run-tasks` (or the desktop's switch); the scheduler
//! rereads the file, so nothing else needs to be told.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use warden_core::tool::{ApprovalRequest, Approver, Tool, ToolSpec};

use crate::tasks::{next_run, remove_task, task_status, upsert_task, TaskConfig, TaskStore, Zone};
use crate::{default_server_tasks_dir, load_config_from_path, save_config, FileConfig};

const APPROVAL_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_PROMPT_CHARS: usize = 4000;
const LIST_PREVIEW_CHARS: usize = 200;

/// What the model asked for, parsed once so it can be checked before asking a human and then
/// re-applied to a freshly read config after they answer.
#[derive(Debug, Clone, PartialEq)]
enum Change {
    Create(TaskConfig),
    /// `None` = leave as is. `agent`/`timezone`: `Some(None)` clears. A new schedule replaces the old one.
    Update { id: String, prompt: Option<String>, agent: Option<Option<String>>, schedule: Option<Schedule>, timezone: Option<Option<String>>, enabled: Option<bool> },
    Delete { id: String },
}

#[derive(Debug, Clone, PartialEq)]
enum Schedule {
    Every(String),
    Cron(String),
    Once(String),
}

impl Change {
    fn action(&self) -> &'static str {
        match self {
            Change::Create(_) => "create_task",
            Change::Update { .. } => "update_task",
            Change::Delete { .. } => "delete_task",
        }
    }

    fn id(&self) -> &str {
        match self {
            Change::Create(task) => &task.id,
            Change::Update { id, .. } | Change::Delete { id } => id,
        }
    }
}

#[derive(Clone)]
pub struct ManageTasksTool {
    config_path: PathBuf,
    approver: Option<Arc<dyn Approver>>,
    approval_timeout: Duration,
    /// The calling agent's own `allowed_tools`: `None` = it has every tool, so no cap.
    caller_limit: Option<Vec<String>>,
    /// Where the run state is read for `list`. `None`: no status, just the tasks.
    store: Option<TaskStore>,
}

impl ManageTasksTool {
    pub fn new(config_path: impl Into<PathBuf>) -> Self {
        Self {
            config_path: config_path.into(),
            approver: None,
            approval_timeout: APPROVAL_TIMEOUT,
            caller_limit: None,
            store: default_server_tasks_dir().map(TaskStore::new),
        }
    }

    pub fn with_caller_limit(mut self, limit: Option<Vec<String>>) -> Self {
        self.caller_limit = limit;
        self
    }

    #[cfg(test)]
    fn with_store(mut self, store: TaskStore) -> Self {
        self.store = Some(store);
        self
    }

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
        let now = now_millis();
        let states = self.store.as_ref().and_then(|s| s.states().ok()).unwrap_or_default();
        let tasks: Vec<Value> = config
            .tasks
            .iter()
            .map(|task| {
                let zone = Zone::parse(task.timezone.as_deref()).unwrap_or(Zone::Local);
                let status = task_status(task, states.get(&task.id), now);
                json!({
                    "id": task.id,
                    "agent_id": task.agent,
                    "prompt": preview(&task.prompt, LIST_PREVIEW_CHARS),
                    "every": task.every,
                    "cron": task.cron,
                    "once": task.once,
                    "timezone": task.timezone,
                    "enabled": task.enabled,
                    "next_run": status.next_run_at_ms.map(|ms| zone.format(ms)),
                    "last_run": status.last_run_at_ms.map(|ms| zone.format(ms)),
                    "last_error": status.last_error,
                })
            })
            .collect();
        let agents: Vec<Value> = config
            .agents
            .iter()
            .map(|a| json!({ "id": a.id, "persona": preview(&a.persona, LIST_PREVIEW_CHARS), "you_can_schedule_it": self.check_power(&config, Some(&a.id)).is_ok() }))
            .collect();
        Ok(json!({
            "now": Zone::Local.format(now),
            "note": "Times without a 'timezone' are read in the hub machine's own zone (the offset in 'now').",
            "tasks": tasks,
            "agents": agents,
        }))
    }

    /// Rule 3: may the caller schedule `agent_id` (or no agent)?
    fn check_power(&self, config: &FileConfig, agent_id: Option<&str>) -> anyhow::Result<()> {
        let Some(limit) = &self.caller_limit else { return Ok(()) };
        let Some(agent_id) = agent_id else {
            anyhow::bail!("a task with no agent runs with every tool, which is more than you have — name an agent whose tools fit in yours");
        };
        let agent = config.agents.iter().find(|a| a.id == agent_id).ok_or_else(|| anyhow::anyhow!("no agent named '{agent_id}'"))?;
        if agent.can_delegate_to_agents || agent.can_message_agents || agent.can_manage_agents || agent.can_manage_tasks {
            anyhow::bail!("agent '{agent_id}' can reach other agents or change settings, which is more than you have — you can't schedule it");
        }
        let Some(tools) = &agent.allowed_tools else {
            anyhow::bail!("agent '{agent_id}' has every tool, which is more than you have — you can't schedule it");
        };
        if let Some(extra) = tools.iter().find(|t| !limit.contains(t)) {
            anyhow::bail!("agent '{agent_id}' has the tool '{extra}', which you don't — you can't schedule it");
        }
        Ok(())
    }

    /// What `config` would look like after `change`, and what a human reads to approve it. Pure but
    /// for the time used in "first run", so the same checks run before and after the prompt.
    fn plan(&self, config: &FileConfig, change: &Change) -> anyhow::Result<(Vec<TaskConfig>, String)> {
        let mut next = FileConfig { tasks: config.tasks.clone(), agents: config.agents.clone(), ..FileConfig::default() };
        let detail = match change {
            Change::Create(task) => {
                check_prompt(&task.prompt)?;
                upsert_task(&mut next, None, task.clone())?;
                let saved = next.tasks.iter().find(|t| t.id == task.id.trim()).expect("just added");
                self.check_power(config, saved.agent.as_deref())?;
                format!("New scheduled task '{}'\n{}", saved.id, describe(saved))
            }
            Change::Update { id, prompt, agent, schedule, timezone, enabled } => {
                let old = config.tasks.iter().find(|t| &t.id == id).ok_or_else(|| anyhow::anyhow!("no task named '{id}' — see 'list'"))?;
                let mut task = old.clone();
                if let Some(prompt) = prompt {
                    check_prompt(prompt)?;
                    task.prompt = prompt.clone();
                }
                if let Some(agent) = agent {
                    task.agent = agent.clone();
                }
                if let Some(schedule) = schedule {
                    (task.every, task.cron, task.once) = match schedule {
                        Schedule::Every(v) => (Some(v.clone()), None, None),
                        Schedule::Cron(v) => (None, Some(v.clone()), None),
                        Schedule::Once(v) => (None, None, Some(v.clone())),
                    };
                }
                if let Some(timezone) = timezone {
                    task.timezone = timezone.clone();
                }
                if let Some(enabled) = enabled {
                    task.enabled = *enabled;
                }
                upsert_task(&mut next, Some(id), task)?;
                let saved = next.tasks.iter().find(|t| &t.id == id).expect("just edited");
                // Editing a task that runs a more powerful agent is handing it out too.
                self.check_power(config, saved.agent.as_deref())?;
                format!("Change scheduled task '{id}'\n\nBefore:\n{}\n\nAfter:\n{}", describe(old), describe(saved))
            }
            Change::Delete { id } => {
                let old = config.tasks.iter().find(|t| &t.id == id).cloned().ok_or_else(|| anyhow::anyhow!("no task named '{id}' — see 'list'"))?;
                remove_task(&mut next, id)?;
                format!("Delete scheduled task '{id}' (its conversation stays)\n{}", describe(&old))
            }
        };
        Ok((next.tasks, detail))
    }

    async fn change(&self, change: Change) -> anyhow::Result<Value> {
        // Check first, so a request that can't succeed never costs the user a prompt.
        let (_, detail) = self.plan(&self.load()?, &change)?;

        let Some(approver) = &self.approver else {
            anyhow::bail!(
                "creating, changing or deleting scheduled tasks needs the user's approval, and this channel can't ask for it \
                 (use the desktop app, the web, the phone or the interactive CLI)"
            );
        };
        let request = ApprovalRequest { target: change.id().to_string(), action: change.action().to_string(), detail };
        let approved = tokio::time::timeout(self.approval_timeout, approver.approve(request)).await.unwrap_or(false);
        if !approved {
            anyhow::bail!("the user did not approve this change to task '{}'", change.id());
        }

        // The prompt can sit open a while, and the tasks may have changed meanwhile.
        let mut config = self.load()?;
        let (tasks, _) = self.plan(&config, &change)?;
        config.tasks = tasks;
        save_config(&self.config_path, &config)?;
        let message = match &change {
            Change::Delete { id } => format!("Task '{id}' deleted. Its conversation stays."),
            Change::Create(_) | Change::Update { .. } => {
                let id = change.id().trim();
                let first = config.tasks.iter().find(|t| t.id == id).and_then(|t| {
                    let zone = Zone::parse(t.timezone.as_deref()).unwrap_or(Zone::Local);
                    next_run(t, None, now_millis()).map(|ms| zone.format(ms))
                });
                format!(
                    "Task '{id}' {}.{} It runs only on the hub started with --run-tasks (or the desktop's switch); each run lands in the conversation 'task-{id}'.",
                    if matches!(change, Change::Create(_)) { "created" } else { "updated" },
                    first.map(|f| format!(" Next run: {f}.")).unwrap_or_default()
                )
            }
        };
        Ok(json!({ "status": "ok", "message": message }))
    }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as i64
}

fn check_prompt(prompt: &str) -> anyhow::Result<()> {
    anyhow::ensure!(prompt.chars().count() <= MAX_PROMPT_CHARS, "the prompt is over {MAX_PROMPT_CHARS} characters — shorten it");
    Ok(())
}

/// Everything a person needs to see about a task: agent, schedule, first run and the whole prompt.
fn describe(task: &TaskConfig) -> String {
    let zone = Zone::parse(task.timezone.as_deref()).unwrap_or(Zone::Local);
    let first = match next_run(task, None, now_millis()) {
        Some(ms) => zone.format(ms),
        None if !task.enabled => "paused".to_string(),
        None => "none".to_string(),
    };
    format!(
        "Agent: {}\nSchedule: {}\nNext run: {first}\n\nPrompt:\n{}",
        task.agent.as_deref().unwrap_or("(none — runs with every tool)"),
        task.schedule_label(),
        task.prompt
    )
}

fn preview(text: &str, max_chars: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > max_chars {
        format!("{}…", flat.chars().take(max_chars).collect::<String>())
    } else {
        flat
    }
}

fn str_arg(args: &Value, name: &str) -> Option<String> {
    args.get(name).and_then(Value::as_str).map(str::to_string)
}

/// A clearable field: absent = leave alone, empty string = clear, anything else = set.
fn clearable_arg(args: &Value, name: &str) -> Option<Option<String>> {
    args.get(name).and_then(Value::as_str).map(|s| Some(s.trim().to_string()).filter(|s| !s.is_empty()))
}

/// At most one of `every`, `cron` and `once`.
fn schedule_arg(args: &Value) -> anyhow::Result<Option<Schedule>> {
    let given: Vec<Schedule> = [("every", Schedule::Every as fn(String) -> Schedule), ("cron", Schedule::Cron), ("once", Schedule::Once)]
        .into_iter()
        .filter_map(|(name, make)| str_arg(args, name).filter(|v| !v.trim().is_empty()).map(make))
        .collect();
    match given.len() {
        0 => Ok(None),
        1 => Ok(given.into_iter().next()),
        _ => anyhow::bail!("give only one of 'every', 'cron' and 'once'"),
    }
}

#[async_trait]
impl Tool for ManageTasksTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "manage_tasks".to_string(),
            description: "List, create, edit or delete the user's scheduled tasks: a prompt an agent runs on its own, on \
                          a schedule (\"every weekday at 8 summarize the news\"), whose answers land in the conversation \
                          'task-<id>'. Use it only when the user asks for something to happen on a schedule or later, or \
                          to change or remove such a task. Every create/update/delete is shown to the user, who must \
                          approve it. Start with 'list': it gives the current time and zone (to build 'cron' and 'once' \
                          right), the tasks, and the agents you can schedule. Write the prompt as a complete request, \
                          since the run won't see this conversation."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["list", "create", "update", "delete"] },
                    "id": {
                        "type": "string",
                        "description": "The task's name: 1-59 letters, digits, '-' or '_' (create: new; update/delete: existing)."
                    },
                    "prompt": {
                        "type": "string",
                        "description": "What the agent is asked on every run — self-contained. Required for create."
                    },
                    "agent_id": {
                        "type": "string",
                        "description": "The agent that runs it (see 'agents' in 'list'). On update, an empty string clears it."
                    },
                    "every": { "type": "string", "description": "An interval counted from the last run: '30m', '2h', '1d' (at least 1m)." },
                    "cron": { "type": "string", "description": "Five-field cron: minute hour day-of-month month day-of-week, e.g. '0 8 * * 1-5'." },
                    "once": { "type": "string", "description": "A single run at this local date and time: 'YYYY-MM-DDTHH:MM'." },
                    "timezone": {
                        "type": "string",
                        "description": "IANA zone for 'cron' and 'once', e.g. 'America/Sao_Paulo'. Leave out for the hub machine's zone; on update, an empty string clears it."
                    },
                    "enabled": { "type": "boolean", "description": "Update only: false pauses the task, true resumes it." }
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
        let id = str_arg(&args, "id").ok_or_else(|| anyhow::anyhow!("missing required 'id' argument"))?;
        let change = match action {
            "delete" => Change::Delete { id },
            "create" => {
                let prompt = str_arg(&args, "prompt").ok_or_else(|| anyhow::anyhow!("missing required 'prompt' argument"))?;
                let schedule = schedule_arg(&args)?.ok_or_else(|| anyhow::anyhow!("give one of 'every', 'cron' or 'once'"))?;
                let (every, cron, once) = match schedule {
                    Schedule::Every(v) => (Some(v), None, None),
                    Schedule::Cron(v) => (None, Some(v), None),
                    Schedule::Once(v) => (None, None, Some(v)),
                };
                Change::Create(TaskConfig {
                    id,
                    agent: clearable_arg(&args, "agent_id").flatten(),
                    prompt,
                    every,
                    cron,
                    once,
                    timezone: clearable_arg(&args, "timezone").flatten(),
                    enabled: true,
                })
            }
            _ => Change::Update {
                id,
                prompt: str_arg(&args, "prompt"),
                agent: clearable_arg(&args, "agent_id"),
                schedule: schedule_arg(&args)?,
                timezone: clearable_arg(&args, "timezone"),
                enabled: args.get("enabled").and_then(Value::as_bool),
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
    use crate::AgentConfig;

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "warden-manage-tasks-test-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn agent(id: &str, tools: Option<&[&str]>) -> AgentConfig {
        AgentConfig {
            id: id.into(),
            persona: format!("persona of {id}"),
            provider_id: None,
            can_delegate_to_agents: false,
            can_manage_agents: false,
            can_message_agents: false,
            can_manage_tasks: false,
            allowed_tools: tools.map(|t| t.iter().map(|s| s.to_string()).collect()),
            owner: None,
            shared_with: Vec::new(),
        }
    }

    fn write_config(dir: &Path, agents: Vec<AgentConfig>) -> PathBuf {
        let path = dir.join("config.toml");
        save_config(&path, &FileConfig { agents, ..FileConfig::default() }).unwrap();
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

    fn tool_with(dir: &Path, path: &Path, answer: bool, limit: Option<&[&str]>) -> (Arc<dyn Tool>, Arc<Scripted>) {
        let approver = Arc::new(Scripted { answer, asked: Mutex::new(Vec::new()) });
        let tool = ManageTasksTool::new(path)
            .with_store(TaskStore::new(dir.join("tasks")))
            .with_caller_limit(limit.map(|l| l.iter().map(|s| s.to_string()).collect()))
            .with_approver(approver.clone())
            .unwrap();
        (tool, approver)
    }

    fn tasks_on_disk(path: &Path) -> Vec<TaskConfig> {
        load_config_from_path(path, true).unwrap().tasks
    }

    fn create(id: &str, agent: &str) -> Value {
        json!({ "action": "create", "id": id, "agent_id": agent, "prompt": "Summarize today's news about Rust.", "cron": "0 8 * * 1-5", "timezone": "America/Sao_Paulo" })
    }

    #[tokio::test]
    async fn create_saves_only_after_a_yes_and_the_card_shows_everything() {
        let dir = temp_dir();
        let path = write_config(&dir, vec![agent("reader", None)]);

        let (no, asked) = tool_with(&dir, &path, false, None);
        assert!(no.call(create("news", "reader")).await.is_err());
        assert!(tasks_on_disk(&path).is_empty());
        assert_eq!(asked.asked.lock().unwrap().len(), 1);

        let (yes, asked) = tool_with(&dir, &path, true, None);
        let reply = yes.call(create("news", "reader")).await.unwrap();
        assert!(reply["message"].as_str().unwrap().contains("Next run:"), "{reply}");
        let saved = tasks_on_disk(&path);
        assert_eq!((saved[0].id.as_str(), saved[0].agent.as_deref(), saved[0].cron.as_deref()), ("news", Some("reader"), Some("0 8 * * 1-5")));

        let card = asked.asked.lock().unwrap()[0].clone();
        assert_eq!((card.target.as_str(), card.action.as_str()), ("news", "create_task"));
        for part in ["Agent: reader", "cron 0 8 * * 1-5 (America/Sao_Paulo)", "Next run: ", "Summarize today's news about Rust."] {
            assert!(card.detail.contains(part), "missing {part:?} in:\n{}", card.detail);
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn without_an_approver_it_refuses() {
        let dir = temp_dir();
        let path = write_config(&dir, vec![agent("reader", None)]);
        let tool = ManageTasksTool::new(&path).with_store(TaskStore::new(dir.join("tasks")));
        let err = tool.call(create("news", "reader")).await.unwrap_err();
        assert!(format!("{err:#}").contains("approval"), "{err:#}");
        assert!(tasks_on_disk(&path).is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn an_impossible_task_is_refused_before_asking() {
        let dir = temp_dir();
        let path = write_config(&dir, vec![agent("reader", None)]);
        let (tool, asked) = tool_with(&dir, &path, true, None);
        let bad = [
            json!({ "action": "create", "id": "a", "prompt": "p", "cron": "every morning" }),
            json!({ "action": "create", "id": "a", "prompt": "p", "every": "1d", "agent_id": "ghost" }),
            json!({ "action": "create", "id": "a b", "prompt": "p", "every": "1d" }),
            json!({ "action": "create", "id": "a", "prompt": "p", "every": "1d", "cron": "0 8 * * *" }),
            json!({ "action": "create", "id": "a", "prompt": "p" }),
            json!({ "action": "update", "id": "ghost", "enabled": false }),
            json!({ "action": "delete", "id": "ghost" }),
        ];
        for args in bad {
            assert!(tool.call(args.clone()).await.is_err(), "{args}");
        }
        tool.call(create("news", "reader")).await.unwrap();
        assert!(tool.call(create("news", "reader")).await.is_err(), "same id");
        assert_eq!(asked.asked.lock().unwrap().len(), 1, "only the valid create asked");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn a_limited_agent_cant_schedule_more_than_it_has() {
        let dir = temp_dir();
        let mut delegating = agent("boss", Some(&["read_file"]));
        delegating.can_delegate_to_agents = true;
        let path = write_config(&dir, vec![agent("reader", Some(&["read_file"])), agent("admin", Some(&["read_file", "shell"])), agent("all", None), delegating]);
        let (tool, asked) = tool_with(&dir, &path, true, Some(&["read_file", "use_skill"]));

        for target in ["admin", "all", "boss"] {
            assert!(tool.call(create("t", target)).await.is_err(), "{target}");
        }
        assert!(tool.call(json!({ "action": "create", "id": "t", "prompt": "p", "every": "1d" })).await.is_err(), "no agent = every tool");
        assert!(asked.asked.lock().unwrap().is_empty());
        tool.call(create("t", "reader")).await.unwrap();
        assert!(tool.call(json!({ "action": "update", "id": "t", "agent_id": "admin" })).await.is_err(), "not by editing either");

        let listed = tool.call(json!({ "action": "list" })).await.unwrap();
        let can: Vec<(&str, bool)> = listed["agents"].as_array().unwrap().iter().map(|a| (a["id"].as_str().unwrap(), a["you_can_schedule_it"].as_bool().unwrap())).collect();
        assert_eq!(can, [("reader", true), ("admin", false), ("all", false), ("boss", false)]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn update_changes_only_what_it_names_and_delete_removes() {
        let dir = temp_dir();
        let path = write_config(&dir, vec![agent("reader", None)]);
        let (tool, asked) = tool_with(&dir, &path, true, None);
        tool.call(create("news", "reader")).await.unwrap();

        tool.call(json!({ "action": "update", "id": "news", "enabled": false })).await.unwrap();
        let task = &tasks_on_disk(&path)[0];
        assert!(!task.enabled);
        assert_eq!((task.cron.as_deref(), task.prompt.as_str()), (Some("0 8 * * 1-5"), "Summarize today's news about Rust."));

        tool.call(json!({ "action": "update", "id": "news", "every": "2h", "timezone": "", "agent_id": "" })).await.unwrap();
        let task = &tasks_on_disk(&path)[0];
        assert_eq!((task.every.as_deref(), task.cron.as_deref(), task.timezone.as_deref(), task.agent.as_deref()), (Some("2h"), None, None, None));
        let card = asked.asked.lock().unwrap().last().unwrap().detail.clone();
        assert!(card.contains("Before:") && card.contains("After:") && card.contains("every 2h"), "{card}");

        tool.call(json!({ "action": "delete", "id": "news" })).await.unwrap();
        assert!(tasks_on_disk(&path).is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn list_gives_the_time_and_the_tasks() {
        let dir = temp_dir();
        let path = write_config(&dir, vec![agent("reader", None)]);
        let (tool, _) = tool_with(&dir, &path, true, None);
        tool.call(create("news", "reader")).await.unwrap();
        let listed = tool.call(json!({ "action": "list" })).await.unwrap();
        assert!(listed["now"].as_str().is_some_and(|n| n.starts_with("20")), "{listed}");
        assert_eq!(listed["tasks"][0]["id"], "news");
        assert!(listed["tasks"][0]["next_run"].as_str().is_some());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn an_unanswered_prompt_counts_as_no() {
        struct NeverAnswers;
        #[async_trait]
        impl Approver for NeverAnswers {
            async fn approve(&self, _request: ApprovalRequest) -> bool {
                std::future::pending().await
            }
        }
        let dir = temp_dir();
        let path = write_config(&dir, vec![agent("reader", None)]);
        let tool = ManageTasksTool::new(&path).with_approval_timeout(Duration::from_millis(50)).with_approver(Arc::new(NeverAnswers)).unwrap();
        assert!(tool.call(create("news", "reader")).await.is_err());
        assert!(tasks_on_disk(&path).is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }
}
