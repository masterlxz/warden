//! Scheduled tasks (P92): a prompt an agent runs on its own, on a schedule — "every day at 8 summarize
//! my e-mails". Defined as `[[tasks]]` in `config.toml` (it syncs, like the agents), run by the hub
//! started with `--run-tasks` (`warden-server`'s `scheduler.rs`), or once by hand with
//! `warden-server tasks run`.
//!
//! The rules, from the design conversation (`ARCHITECTURE.md`, "Tarefas agendadas"):
//! 1. Each task has its own conversation, `task-<id>`; every run adds the prompt and the answer to it.
//! 2. A run missed while the hub was down happens once when it's back, however many were missed.
//! 3. Nobody is watching a run, so it gets no approver: a tool that needs a yes refuses, as on
//!    Telegram. The P4 spending limits apply, under the channel `tasks` and the user `task:<id>`.
//!
//! The run state (when each task was first seen, when it last ran and how it went) lives next to the
//! conversations, outside the sync: it belongs to the hub that runs the tasks, not to the workspace.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context;
use chrono::{Local, NaiveDateTime, TimeZone};
use chrono_tz::Tz;
use croner::parser::{CronParser, Seconds, Year};
use croner::Cron;
use serde::{Deserialize, Serialize};
use warden_core::model::Message;
use warden_core::orchestrator::{MessageOutcome, Orchestrator};
use warden_core::spend::SpendContext;
use warden_server_protocol::protocol::{TaskDto, TaskInfoDto};

use crate::{
    append_messages, assistant_message, build_model_for, load_conversation, message_id, now_millis, scope_to_agent, to_message, AgentConfig, AgentExtras,
    AppendOptions, ChatRole, ConversationMessage, ConversationWriteGuard, FileConfig,
};

/// A task's conversation is `task-<id>`, and a hub conversation id is at most 64 characters.
pub const MAX_TASK_ID_LEN: usize = 59;
pub const CONVERSATION_PREFIX: &str = "task-";
/// The shortest `every`.
pub const MIN_INTERVAL: Duration = Duration::from_secs(60);
/// How much of the task's conversation a run sends to the model: the last ten runs. A daily task
/// would otherwise carry a year of answers in every call.
pub const HISTORY_MESSAGES: usize = 20;

/// One `[[tasks]]` entry. Exactly one of `every`, `cron` and `once` is set.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TaskConfig {
    /// Unique among `tasks`: 1-59 ASCII letters, digits, `-` or `_`.
    pub id: String,
    /// The agent that runs it. `None` runs with no persona, like a chat with no agent picked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    pub prompt: String,
    /// An interval: `"30m"`, `"2h"`, `"1d"`. Counted from the last run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub every: Option<String>,
    /// A five-field cron expression, `"0 8 * * 1-5"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cron: Option<String>,
    /// Once, at this local time: `"2026-10-01T09:00"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub once: Option<String>,
    /// IANA zone for `cron` and `once`, `"America/Sao_Paulo"`. `None`: the hub machine's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    /// `false` pauses it. A paused task that is switched back on counts from that moment, so it
    /// doesn't make up for the runs it skipped.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_enabled() -> bool {
    true
}

#[derive(Debug, Clone)]
pub enum Schedule {
    Every(Duration),
    Cron(Box<Cron>),
    Once(NaiveDateTime),
}

/// The zone a task's times are read in.
#[derive(Debug, Clone, Copy)]
pub enum Zone {
    Local,
    Named(Tz),
}

impl Zone {
    pub fn parse(name: Option<&str>) -> anyhow::Result<Self> {
        match name.map(str::trim).filter(|n| !n.is_empty()) {
            None => Ok(Self::Local),
            Some(name) => name
                .parse::<Tz>()
                .map(Self::Named)
                .map_err(|_| anyhow::anyhow!("unknown time zone '{name}' — use an IANA name like 'America/Sao_Paulo' or 'UTC'")),
        }
    }

    fn to_millis(self, local: &NaiveDateTime) -> Option<i64> {
        match self {
            Self::Local => Local.from_local_datetime(local).earliest().map(|t| t.timestamp_millis()),
            Self::Named(tz) => tz.from_local_datetime(local).earliest().map(|t| t.timestamp_millis()),
        }
    }

    fn cron_after(self, cron: &Cron, after_ms: i64) -> Option<i64> {
        match self {
            Self::Local => {
                let after = Local.timestamp_millis_opt(after_ms).single()?;
                cron.find_next_occurrence(&after, false).ok().map(|t| t.timestamp_millis())
            }
            Self::Named(tz) => {
                let after = tz.timestamp_millis_opt(after_ms).single()?;
                cron.find_next_occurrence(&after, false).ok().map(|t| t.timestamp_millis())
            }
        }
    }

    /// `2026-09-28 08:00 -03`, for the prompt and the CLI.
    pub fn format(self, ms: i64) -> String {
        match self {
            Self::Local => Local.timestamp_millis_opt(ms).single().map(|t| t.format("%Y-%m-%d %H:%M %:z").to_string()),
            Self::Named(tz) => tz.timestamp_millis_opt(ms).single().map(|t| t.format("%Y-%m-%d %H:%M %Z").to_string()),
        }
        .unwrap_or_else(|| ms.to_string())
    }
}

/// `"30m"`, `"2h"`, `"1d"`: a whole number and a unit, at least a minute.
pub fn parse_every(text: &str) -> anyhow::Result<Duration> {
    let text = text.trim();
    let split = text.find(|c: char| !c.is_ascii_digit()).unwrap_or(text.len());
    let (number, unit) = text.split_at(split);
    let unit_secs = match unit.trim() {
        "m" | "min" => 60,
        "h" => 3600,
        "d" => 86_400,
        _ => anyhow::bail!("invalid interval '{text}' — use a number and m, h or d, like '30m', '2h' or '1d'"),
    };
    let number: u64 = number.parse().map_err(|_| anyhow::anyhow!("invalid interval '{text}' — use a number and m, h or d, like '30m', '2h' or '1d'"))?;
    let every = Duration::from_secs(number.saturating_mul(unit_secs));
    anyhow::ensure!(every >= MIN_INTERVAL, "the interval '{text}' is too short — the shortest is 1m");
    Ok(every)
}

/// Five fields — minute, hour, day of month, month, day of week — or an alias like `@daily`.
pub fn parse_cron(text: &str) -> anyhow::Result<Cron> {
    CronParser::builder()
        .seconds(Seconds::Disallowed)
        .year(Year::Disallowed)
        .build()
        .parse(text)
        .map_err(|err| anyhow::anyhow!("invalid cron '{text}' ({err}) — use five fields, like '0 8 * * 1-5' for 8:00 on weekdays"))
}

/// `"2026-10-01T09:00"` (seconds and a space instead of the `T` are fine too).
pub fn parse_once(text: &str) -> anyhow::Result<NaiveDateTime> {
    let text = text.trim();
    ["%Y-%m-%dT%H:%M", "%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M", "%Y-%m-%d %H:%M:%S"]
        .iter()
        .find_map(|format| NaiveDateTime::parse_from_str(text, format).ok())
        .ok_or_else(|| anyhow::anyhow!("invalid date '{text}' — use YYYY-MM-DDTHH:MM, like '2026-10-01T09:00'"))
}

impl TaskConfig {
    pub fn schedule(&self) -> anyhow::Result<(Schedule, Zone)> {
        let zone = Zone::parse(self.timezone.as_deref())?;
        let schedule = match (&self.every, &self.cron, &self.once) {
            (Some(every), None, None) => Schedule::Every(parse_every(every)?),
            (None, Some(cron), None) => Schedule::Cron(Box::new(parse_cron(cron)?)),
            (None, None, Some(once)) => Schedule::Once(parse_once(once)?),
            (None, None, None) => anyhow::bail!("task '{}' has no schedule — set one of every, cron or once", self.id),
            _ => anyhow::bail!("task '{}' has more than one schedule — set only one of every, cron or once", self.id),
        };
        Ok((schedule, zone))
    }

    /// What resets the run state when edited: the schedule and its zone, not the prompt or agent.
    fn fingerprint(&self) -> String {
        format!("{:?}|{:?}|{:?}|{:?}", self.every, self.cron, self.once, self.timezone)
    }

    /// The schedule as the CLI shows it.
    pub fn schedule_label(&self) -> String {
        let zone = self.timezone.as_deref().map(|tz| format!(" ({tz})")).unwrap_or_default();
        match (&self.every, &self.cron, &self.once) {
            (Some(every), _, _) => format!("every {every}"),
            (_, Some(cron), _) => format!("cron {cron}{zone}"),
            (_, _, Some(once)) => format!("once {once}{zone}"),
            _ => "no schedule".to_string(),
        }
    }
}

pub fn conversation_id(task_id: &str) -> String {
    format!("{CONVERSATION_PREFIX}{task_id}")
}

pub(crate) fn is_valid_task_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_TASK_ID_LEN && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Every check a task list must pass: safe unique ids, a prompt, exactly one valid schedule and an
/// agent that exists.
pub fn check_tasks(tasks: &[TaskConfig], agents: &[AgentConfig]) -> anyhow::Result<()> {
    let mut seen = std::collections::HashSet::new();
    for task in tasks {
        anyhow::ensure!(
            is_valid_task_id(&task.id),
            "invalid task id '{}' — use 1-{MAX_TASK_ID_LEN} letters, digits, '-' or '_'",
            task.id
        );
        anyhow::ensure!(seen.insert(task.id.as_str()), "there are two tasks named '{}'", task.id);
        anyhow::ensure!(!task.prompt.trim().is_empty(), "task '{}' has an empty prompt", task.id);
        task.schedule()?;
        if let Some(agent) = &task.agent {
            // P84: tasks are the owner's, so they run the owner's agents only.
            anyhow::ensure!(agents.iter().any(|a| &a.id == agent && a.owner.is_none()), "task '{}' names agent '{agent}', which doesn't exist", task.id);
        }
    }
    Ok(())
}

fn next_due(schedule: &Schedule, zone: Zone, anchor_ms: i64) -> Option<i64> {
    match schedule {
        Schedule::Every(every) => Some(anchor_ms.saturating_add(every.as_millis() as i64)),
        Schedule::Cron(cron) => zone.cron_after(cron, anchor_ms),
        Schedule::Once(at) => zone.to_millis(at),
    }
}

/// What the hub remembers about one task.
#[derive(Serialize, Deserialize, Default, Debug, Clone, PartialEq)]
pub struct TaskState {
    /// The schedule this state was counted against; a different one starts over.
    pub fingerprint: String,
    /// When this hub first saw the task (or its current schedule, or its switching back on): a new
    /// task counts from here, so it never fires the moment it's added.
    pub seen_at_ms: i64,
    #[serde(default)]
    pub last_run_at_ms: Option<i64>,
    #[serde(default)]
    pub last_finished_at_ms: Option<i64>,
    /// Why the last run failed. `None` after one that worked.
    #[serde(default)]
    pub last_error: Option<String>,
}

impl TaskState {
    fn anchor(&self) -> i64 {
        self.last_run_at_ms.map_or(self.seen_at_ms, |run| run.max(self.seen_at_ms))
    }
}

/// When `task` runs next, for display: `None` when paused, done (a `once` that ran) or unparseable.
pub fn next_run(task: &TaskConfig, state: Option<&TaskState>, now_ms: i64) -> Option<i64> {
    if !task.enabled {
        return None;
    }
    let (schedule, zone) = task.schedule().ok()?;
    let state = state.filter(|s| s.fingerprint == task.fingerprint());
    if matches!(schedule, Schedule::Once(_)) && state.is_some_and(|s| s.last_run_at_ms.is_some()) {
        return None;
    }
    next_due(&schedule, zone, state.map_or(now_ms, TaskState::anchor))
}

#[derive(Serialize, Deserialize, Default)]
struct StateFile {
    #[serde(default)]
    tasks: BTreeMap<String, TaskState>,
}

/// A hub's scheduled-task directory: `conversations/` and `state.json`.
#[derive(Debug, Clone)]
pub struct TaskStore {
    dir: PathBuf,
}

impl TaskStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn conversations_dir(&self) -> PathBuf {
        self.dir.join("conversations")
    }

    fn state_path(&self) -> PathBuf {
        self.dir.join("state.json")
    }

    pub fn states(&self) -> anyhow::Result<BTreeMap<String, TaskState>> {
        let path = self.state_path();
        match std::fs::read_to_string(&path) {
            Ok(text) => Ok(serde_json::from_str::<StateFile>(&text).with_context(|| format!("failed to parse {}", path.display()))?.tasks),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
            Err(err) => Err(err).with_context(|| format!("failed to read {}", path.display())),
        }
    }

    /// Reads, changes and saves the state under the same lock the conversations use, so the hub and
    /// a `tasks run` in another process never save over each other.
    fn update<R>(&self, change: impl FnOnce(&mut BTreeMap<String, TaskState>) -> R) -> anyhow::Result<R> {
        let _guard = ConversationWriteGuard::acquire(&self.dir)?;
        let mut tasks = self.states()?;
        let result = change(&mut tasks);
        let path = self.state_path();
        let tmp = self.dir.join("state.json.tmp");
        let text = serde_json::to_string_pretty(&StateFile { tasks }).context("failed to serialize the task state")?;
        std::fs::write(&tmp, text).with_context(|| format!("failed to write {}", tmp.display()))?;
        std::fs::rename(&tmp, &path).with_context(|| format!("failed to write {}", path.display()))?;
        Ok(result)
    }

    /// The tasks due at `now_ms`, each marked as run so the next call doesn't return it again. Also
    /// starts counting new, edited and paused tasks from now, and forgets removed ones. A task whose
    /// schedule doesn't parse is skipped (`check_tasks` is where that gets reported).
    pub fn claim_due(&self, tasks: &[TaskConfig], now_ms: i64) -> anyhow::Result<Vec<TaskConfig>> {
        self.update(|states| {
            states.retain(|id, _| tasks.iter().any(|t| &t.id == id));
            let mut due = Vec::new();
            for task in tasks {
                let Ok((schedule, zone)) = task.schedule() else { continue };
                let fingerprint = task.fingerprint();
                let state = states.entry(task.id.clone()).or_default();
                if state.fingerprint != fingerprint {
                    *state = TaskState { fingerprint, seen_at_ms: now_ms, ..TaskState::default() };
                }
                if !task.enabled {
                    // Switching it back on counts from then, not from before the pause.
                    state.seen_at_ms = now_ms;
                    continue;
                }
                if matches!(schedule, Schedule::Once(_)) && state.last_run_at_ms.is_some() {
                    continue;
                }
                if next_due(&schedule, zone, state.anchor()).is_some_and(|at| at <= now_ms) {
                    state.last_run_at_ms = Some(now_ms);
                    due.push(task.clone());
                }
            }
            due
        })
    }

    /// Marks `task_id` as run at `now_ms` without asking whether it was due — `tasks run`.
    pub fn mark_started(&self, task: &TaskConfig, now_ms: i64) -> anyhow::Result<()> {
        self.update(|states| {
            let fingerprint = task.fingerprint();
            let state = states.entry(task.id.clone()).or_default();
            if state.fingerprint != fingerprint {
                *state = TaskState { fingerprint, seen_at_ms: now_ms, ..TaskState::default() };
            }
            state.last_run_at_ms = Some(now_ms);
        })
    }

    pub fn record_finish(&self, task_id: &str, now_ms: i64, error: Option<String>) -> anyhow::Result<()> {
        self.update(|states| {
            if let Some(state) = states.get_mut(task_id) {
                state.last_finished_at_ms = Some(now_ms);
                state.last_error = error;
            }
        })
    }
}

/// A task as a form sends it: blanks become `None` and the text is trimmed.
fn normalized(task: TaskConfig) -> TaskConfig {
    let clean = |value: Option<String>| value.map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
    TaskConfig {
        id: task.id.trim().to_string(),
        agent: clean(task.agent),
        prompt: task.prompt.trim().to_string(),
        every: clean(task.every),
        cron: clean(task.cron),
        once: clean(task.once),
        timezone: clean(task.timezone),
        enabled: task.enabled,
    }
}

/// Adds `task`, or puts it in place of `original_id` (a rename when the ids differ), and checks the
/// whole list. `config` is left as it was on an error.
pub fn upsert_task(config: &mut FileConfig, original_id: Option<&str>, task: TaskConfig) -> anyhow::Result<()> {
    let task = normalized(task);
    let mut tasks = config.tasks.clone();
    match original_id {
        Some(original) => {
            let i = tasks.iter().position(|t| t.id == original).ok_or_else(|| anyhow::anyhow!("no task named '{original}'"))?;
            tasks[i] = task;
        }
        None => {
            anyhow::ensure!(!tasks.iter().any(|t| t.id == task.id), "there's already a task named '{}'", task.id);
            tasks.push(task);
        }
    }
    check_tasks(&tasks, &config.agents)?;
    crate::webhooks::check_task_clashes(&tasks, &config.webhooks)?;
    config.tasks = tasks;
    Ok(())
}

pub fn remove_task(config: &mut FileConfig, id: &str) -> anyhow::Result<()> {
    let i = config.tasks.iter().position(|t| t.id == id).ok_or_else(|| anyhow::anyhow!("no task named '{id}'"))?;
    config.tasks.remove(i);
    Ok(())
}

pub fn set_task_enabled(config: &mut FileConfig, id: &str, enabled: bool) -> anyhow::Result<()> {
    let task = config.tasks.iter_mut().find(|t| t.id == id).ok_or_else(|| anyhow::anyhow!("no task named '{id}'"))?;
    task.enabled = enabled;
    Ok(())
}

/// Where a task stands, for the screens and `tasks list`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TaskStatus {
    /// `None` when paused, done (a `once` that ran) or the schedule doesn't parse.
    pub next_run_at_ms: Option<i64>,
    pub last_run_at_ms: Option<i64>,
    pub last_finished_at_ms: Option<i64>,
    /// Why the last finished run failed.
    pub last_error: Option<String>,
    /// Started and not finished yet.
    pub running: bool,
    pub schedule_error: Option<String>,
}

pub fn task_status(task: &TaskConfig, state: Option<&TaskState>, now_ms: i64) -> TaskStatus {
    let last_run_at_ms = state.and_then(|s| s.last_run_at_ms);
    let last_finished_at_ms = state.and_then(|s| s.last_finished_at_ms);
    TaskStatus {
        next_run_at_ms: next_run(task, state, now_ms),
        last_run_at_ms,
        last_finished_at_ms,
        last_error: state.and_then(|s| s.last_error.clone()),
        running: last_run_at_ms.is_some_and(|run| last_finished_at_ms.is_none_or(|done| done < run)),
        schedule_error: task.schedule().err().map(|err| format!("{err:#}")),
    }
}

impl From<TaskDto> for TaskConfig {
    fn from(dto: TaskDto) -> Self {
        Self { id: dto.id, agent: dto.agent_id, prompt: dto.prompt, every: dto.every, cron: dto.cron, once: dto.once, timezone: dto.timezone, enabled: dto.enabled }
    }
}

impl From<TaskConfig> for TaskDto {
    fn from(task: TaskConfig) -> Self {
        Self { id: task.id, agent_id: task.agent, prompt: task.prompt, every: task.every, cron: task.cron, once: task.once, timezone: task.timezone, enabled: task.enabled }
    }
}

/// Every task of `config` with where it stands in `store`, as the screens show them.
pub fn task_infos(tasks: &[TaskConfig], store: &TaskStore, now_ms: i64) -> anyhow::Result<Vec<TaskInfoDto>> {
    let states = store.states()?;
    Ok(tasks
        .iter()
        .map(|task| {
            let status = task_status(task, states.get(&task.id), now_ms);
            TaskInfoDto {
                task: task.clone().into(),
                next_run_at_ms: status.next_run_at_ms,
                last_run_at_ms: status.last_run_at_ms,
                last_finished_at_ms: status.last_finished_at_ms,
                last_error: status.last_error,
                running: status.running,
                schedule_error: status.schedule_error,
            }
        })
        .collect())
}

/// What only this machine decides about the hub it runs — kept out of `config.toml`, which syncs
/// whole (so a switch there would turn on in every machine at once). Today: whether the desktop's
/// embedded hub runs the scheduled tasks (`warden-server` takes `--run-tasks` instead), and whether
/// the desktop lends this computer to a hub as a node (P97).
#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct HubLocalConfig {
    pub run_tasks: bool,
    pub lend: Option<LendConfig>,
}

/// The desktop's "lend this computer" (P97): what `warden-server node` takes as flags. The pairing
/// key isn't here — it's only needed until the hub issues a token, which lives in `node.json`.
#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct LendConfig {
    pub enabled: bool,
    pub hub_url: String,
    /// How this computer shows up on the hub; blank is the host name.
    pub name: String,
    pub description: String,
    pub tags: Vec<String>,
    pub shell: bool,
    pub files: Option<PathBuf>,
    /// MCP server names from this machine's `config.toml`.
    pub mcp: Vec<String>,
    /// Provider ids from this machine's `config.toml`.
    pub models: Vec<String>,
}

pub fn default_hub_local_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("warden").join("hub-local.json"))
}

/// A missing file is the default: nothing switched on.
pub fn load_hub_local(path: &Path) -> anyhow::Result<HubLocalConfig> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).with_context(|| format!("failed to parse {}", path.display())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(HubLocalConfig::default()),
        Err(err) => Err(err).with_context(|| format!("failed to read {}", path.display())),
    }
}

pub fn save_hub_local(path: &Path, config: &HubLocalConfig) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(config).context("failed to serialize the hub's local settings")?;
    std::fs::write(path, text).with_context(|| format!("failed to write {}", path.display()))
}

/// Runs `task` once and adds the prompt and the answer — or why there is none — to its
/// conversation in `conversations_dir`. `base` is the hub's orchestrator before any agent scoping.
pub async fn run_task(
    base: &Orchestrator,
    config: &FileConfig,
    config_path: Option<&Path>,
    task: &TaskConfig,
    conversations_dir: &Path,
    now_ms: i64,
) -> anyhow::Result<MessageOutcome> {
    let zone = Zone::parse(task.timezone.as_deref()).unwrap_or(Zone::Local);
    let input = format!("[Scheduled task '{}', {}]\n\n{}", task.id, zone.format(now_ms), task.prompt.trim());
    let turn = UnattendedTurn {
        conversation: conversation_id(&task.id),
        title: format!("Tarefa: {}", task.id),
        agent: task.agent.as_deref(),
        spend: SpendContext::new("tasks").with_user(format!("task:{}", task.id)),
        input,
    };
    run_unattended_turn(base, config, config_path, conversations_dir, turn).await
}

/// One turn nobody is watching — a scheduled task's run or a webhook's call (`webhooks.rs`): where it is saved, who
/// runs it and what it is told.
pub(crate) struct UnattendedTurn<'a> {
    /// The conversation id the prompt and the answer are added to.
    pub conversation: String,
    pub title: String,
    pub agent: Option<&'a str>,
    /// Who the spending is counted for.
    pub spend: SpendContext,
    pub input: String,
}

/// Runs `turn` with the last `HISTORY_MESSAGES` of its conversation as history, and adds the input and the answer — or
/// why there is none — to the conversation.
pub(crate) async fn run_unattended_turn(base: &Orchestrator, config: &FileConfig, config_path: Option<&Path>, conversations_dir: &Path, turn: UnattendedTurn<'_>) -> anyhow::Result<MessageOutcome> {
    let UnattendedTurn { conversation, title, agent, spend, input } = turn;
    let outcome = match prepare(base, config, config_path, agent, spend) {
        Ok((orchestrator, persona)) => {
            let history: Vec<Message> = load_conversation(conversations_dir, &conversation)?
                .map(|c| {
                    let skip = c.messages.len().saturating_sub(HISTORY_MESSAGES);
                    c.messages[skip..].iter().map(to_message).collect()
                })
                .unwrap_or_default();
            orchestrator.handle_turn(&history, &input, Vec::new(), persona.as_deref()).await
        }
        Err(err) => Err(err),
    };

    let user = plain_message(ChatRole::User, input);
    let reply = match &outcome {
        Ok(outcome) => assistant_message(outcome),
        // In the conversation too, so whoever opens it sees why this run has no answer.
        Err(err) => plain_message(ChatRole::Assistant, format!("(could not run: {err:#})")),
    };
    let options = AppendOptions { title_seed: &title, agent_id: agent, provider_id: None, project_id: None, create: true, ..Default::default() };
    append_messages(conversations_dir, &conversation, options, vec![user, reply])?;
    outcome
}

/// The orchestrator and persona an unattended turn runs with: spending counted as `spend`, scoped to `agent` and that
/// agent's model. No approver is attached — nobody is there to answer.
fn prepare(base: &Orchestrator, config: &FileConfig, config_path: Option<&Path>, agent: Option<&str>, spend: SpendContext) -> anyhow::Result<(Orchestrator, Option<String>)> {
    let base = base.with_spend_context(spend);
    let Some(agent_id) = agent else {
        return Ok((base, None));
    };
    let scoped = scope_to_agent(&base, config, config_path, agent_id, AgentExtras::default())
        .ok_or_else(|| anyhow::anyhow!("agent '{agent_id}' doesn't exist any more"))?;
    let mut orchestrator = scoped.orchestrator;
    if let Some(provider_id) = &scoped.provider_id {
        let model = build_model_for(config, provider_id, None).with_context(|| format!("agent '{agent_id}' can't use its model '{provider_id}'"))?;
        orchestrator = orchestrator.with_model(model);
    }
    Ok((orchestrator, Some(scoped.persona)))
}

fn plain_message(role: ChatRole, content: String) -> ConversationMessage {
    ConversationMessage {
        id: message_id(),
        role,
        content,
        created_at: now_millis(),
        usage: None,
        attachments: Vec::new(),
        generated_files: Vec::new(), tools_used: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use warden_core::memory::Vault;
    use warden_core::model::{response_stream, ChatStream, ModelProvider, Response, Role};
    use warden_core::tool::ToolSpec;

    use super::*;
    use crate::{render_config, AgentConfig};

    const MINUTE: i64 = 60_000;
    const HOUR: i64 = 60 * MINUTE;

    fn temp_dir() -> PathBuf {
        std::env::temp_dir().join(format!(
            "warden-tasks-test-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ))
    }

    fn task(id: &str) -> TaskConfig {
        TaskConfig {
            id: id.into(),
            agent: None,
            prompt: "say hi".into(),
            every: Some("1h".into()),
            cron: None,
            once: None,
            timezone: None,
            enabled: true,
        }
    }

    fn agent(id: &str) -> AgentConfig {
        AgentConfig {
            id: id.into(),
            persona: format!("I am {id}"),
            provider_id: None,
            can_delegate_to_agents: false,
            can_manage_agents: false,
            can_message_agents: false,
            can_manage_tasks: false,
            allowed_tools: None,
            autonomy: crate::default_autonomy(),
            owner: None,
            shared_with: Vec::new(),
        }
    }

    fn ids(tasks: Vec<TaskConfig>) -> Vec<String> {
        tasks.into_iter().map(|t| t.id).collect()
    }

    #[test]
    fn intervals_parse_and_refuse_nonsense() {
        assert_eq!(parse_every("30m").unwrap(), Duration::from_secs(1800));
        assert_eq!(parse_every(" 2h ").unwrap(), Duration::from_secs(7200));
        assert_eq!(parse_every("1d").unwrap(), Duration::from_secs(86_400));
        assert!(parse_every("0m").is_err());
        assert!(parse_every("1s").is_err());
        assert!(parse_every("h").is_err());
        assert!(parse_every("1 week").is_err());
    }

    #[test]
    fn cron_takes_five_fields_only() {
        assert!(parse_cron("0 8 * * 1-5").is_ok());
        assert!(parse_cron("@daily").is_ok());
        assert!(parse_cron("0 0 8 * * 1-5").is_err(), "no seconds field");
        assert!(parse_cron("every day").is_err());
    }

    #[test]
    fn once_takes_a_local_date_and_time() {
        assert!(parse_once("2026-10-01T09:00").is_ok());
        assert!(parse_once("2026-10-01 09:00:30").is_ok());
        assert!(parse_once("tomorrow").is_err());
    }

    #[test]
    fn cron_is_read_in_the_task_zone() {
        let cron = parse_cron("0 8 * * *").unwrap();
        let zone = Zone::parse(Some("America/Sao_Paulo")).unwrap();
        // 2026-09-28 00:00 UTC is 2026-09-27 21:00 in São Paulo; the next 8:00 there is 11:00 UTC.
        let after = chrono::Utc.with_ymd_and_hms(2026, 9, 28, 0, 0, 0).unwrap().timestamp_millis();
        let next = next_due(&Schedule::Cron(Box::new(cron)), zone, after).unwrap();
        assert_eq!(next, chrono::Utc.with_ymd_and_hms(2026, 9, 28, 11, 0, 0).unwrap().timestamp_millis());
        assert!(Zone::parse(Some("Mars/Olympus")).is_err());
    }

    #[test]
    fn checks_catch_bad_tasks() {
        let agents = [agent("ana")];
        assert!(check_tasks(&[task("daily")], &agents).is_ok());
        assert!(check_tasks(&[task("a b")], &agents).is_err());
        assert!(check_tasks(&[task(&"x".repeat(MAX_TASK_ID_LEN + 1))], &agents).is_err());
        assert!(check_tasks(&[task("a"), task("a")], &agents).is_err());
        assert!(check_tasks(&[TaskConfig { agent: Some("bia".into()), ..task("a") }], &agents).is_err());
        assert!(check_tasks(&[TaskConfig { agent: Some("ana".into()), ..task("a") }], &agents).is_ok());
        assert!(check_tasks(&[TaskConfig { cron: Some("0 8 * * *".into()), ..task("a") }], &agents).is_err(), "two schedules");
        assert!(check_tasks(&[TaskConfig { every: None, ..task("a") }], &agents).is_err(), "no schedule");
        assert!(check_tasks(&[TaskConfig { prompt: "  ".into(), ..task("a") }], &agents).is_err());
    }

    #[test]
    fn a_new_task_waits_for_its_first_time() {
        let dir = temp_dir();
        let store = TaskStore::new(&dir);
        let tasks = [task("hourly")];
        assert!(store.claim_due(&tasks, 0).unwrap().is_empty(), "seen now, due in an hour");
        assert!(store.claim_due(&tasks, 59 * MINUTE).unwrap().is_empty());
        assert_eq!(ids(store.claim_due(&tasks, HOUR).unwrap()), ["hourly"]);
        assert!(store.claim_due(&tasks, HOUR + MINUTE).unwrap().is_empty(), "claimed once");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missed_runs_become_one() {
        let dir = temp_dir();
        let store = TaskStore::new(&dir);
        let tasks = [task("hourly")];
        store.claim_due(&tasks, 0).unwrap();
        // The hub was down for five hours: one run, and the next one an hour after it.
        assert_eq!(ids(store.claim_due(&tasks, 5 * HOUR).unwrap()), ["hourly"]);
        assert!(store.claim_due(&tasks, 5 * HOUR + 30 * MINUTE).unwrap().is_empty());
        assert_eq!(ids(store.claim_due(&tasks, 6 * HOUR).unwrap()), ["hourly"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn once_runs_once_and_again_when_edited() {
        let dir = temp_dir();
        let store = TaskStore::new(&dir);
        let at = |text: &str| TaskConfig { every: None, once: Some(text.into()), timezone: Some("UTC".into()), ..task("remind") };
        let first = [at("1970-01-01T01:00")];
        assert!(store.claim_due(&first, 0).unwrap().is_empty());
        assert_eq!(ids(store.claim_due(&first, 2 * HOUR).unwrap()), ["remind"], "missed while down: runs when back");
        assert!(store.claim_due(&first, 3 * HOUR).unwrap().is_empty());
        assert_eq!(next_run(&first[0], store.states().unwrap().get("remind"), 3 * HOUR), None);

        let edited = [at("1970-01-01T04:00")];
        assert_eq!(ids(store.claim_due(&edited, 4 * HOUR).unwrap()), ["remind"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_paused_task_counts_from_when_it_comes_back() {
        let dir = temp_dir();
        let store = TaskStore::new(&dir);
        store.claim_due(&[task("hourly")], 0).unwrap();
        let paused = [TaskConfig { enabled: false, ..task("hourly") }];
        assert!(store.claim_due(&paused, 3 * HOUR).unwrap().is_empty());
        assert!(store.claim_due(&[task("hourly")], 3 * HOUR + MINUTE).unwrap().is_empty(), "no catching up on the pause");
        assert_eq!(ids(store.claim_due(&[task("hourly")], 4 * HOUR).unwrap()), ["hourly"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn removed_tasks_are_forgotten() {
        let dir = temp_dir();
        let store = TaskStore::new(&dir);
        store.claim_due(&[task("a"), task("b")], 0).unwrap();
        store.claim_due(&[task("a")], MINUTE).unwrap();
        assert_eq!(store.states().unwrap().keys().collect::<Vec<_>>(), ["a"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn tasks_round_trip_through_the_config_file_with_comments() {
        let existing = "# my tasks\n[[tasks]]\n# every morning\nid = \"morning\"\nprompt = \"summarize\"\ncron = \"0 8 * * *\"\n";
        let config: FileConfig = toml::from_str(existing).unwrap();
        assert_eq!(config.tasks[0].cron.as_deref(), Some("0 8 * * *"));
        assert!(config.tasks[0].enabled);
        let mut config = config;
        config.tasks[0].enabled = false;
        let rendered = render_config(Some(existing), &config).unwrap();
        assert!(rendered.contains("# every morning"), "{rendered}");
        assert!(rendered.contains("enabled = false"), "{rendered}");
        let back: FileConfig = toml::from_str(&rendered).unwrap();
        assert_eq!(back.tasks, config.tasks);
        assert!(!toml::to_string(&FileConfig::default()).unwrap().contains("tasks"));
    }

    /// Answers "<persona>|<number of messages>|<last message>", or fails.
    struct Echo {
        fail: bool,
    }

    #[async_trait]
    impl ModelProvider for Echo {
        async fn chat_stream(&self, messages: Vec<Message>, _tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
            if self.fail {
                anyhow::bail!("provider down");
            }
            let persona = messages.iter().find(|m| m.role == Role::System).map(|m| m.content.clone()).unwrap_or_default();
            let last = messages.last().unwrap().content.clone();
            let content = format!("{persona}|{}|{last}", messages.len());
            Ok(response_stream(Response { content, tool_calls: Vec::new(), usage: None }))
        }
    }

    #[test]
    fn upsert_creates_renames_and_refuses_clashes() {
        let mut config = FileConfig { agents: vec![agent("ana")], ..FileConfig::default() };
        let form = TaskConfig { id: " daily ".into(), agent: Some("".into()), timezone: Some(" ".into()), ..task("x") };
        upsert_task(&mut config, None, form).unwrap();
        assert_eq!((config.tasks[0].id.as_str(), config.tasks[0].agent.as_deref(), config.tasks[0].timezone.as_deref()), ("daily", None, None));

        assert!(upsert_task(&mut config, None, task("daily")).is_err(), "same id");
        upsert_task(&mut config, None, task("other")).unwrap();
        assert!(upsert_task(&mut config, Some("other"), task("daily")).is_err(), "rename onto an existing id");
        assert!(upsert_task(&mut config, Some("ghost"), task("x")).is_err());
        assert!(upsert_task(&mut config, None, TaskConfig { agent: Some("bia".into()), ..task("third") }).is_err());
        assert_eq!(config.tasks.len(), 2, "a refused change leaves the list alone");

        upsert_task(&mut config, Some("daily"), TaskConfig { agent: Some("ana".into()), ..task("morning") }).unwrap();
        assert_eq!(config.tasks.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["morning", "other"]);

        set_task_enabled(&mut config, "other", false).unwrap();
        assert!(!config.tasks[1].enabled);
        remove_task(&mut config, "morning").unwrap();
        assert_eq!(config.tasks.len(), 1);
        assert!(remove_task(&mut config, "morning").is_err() && set_task_enabled(&mut config, "morning", true).is_err());
    }

    #[test]
    fn status_tells_running_from_done() {
        let job = task("hourly");
        let never = task_status(&job, None, 0);
        assert_eq!((never.next_run_at_ms, never.running, never.last_run_at_ms), (Some(HOUR), false, None));

        let fingerprint = job.fingerprint();
        let running = TaskState { fingerprint: fingerprint.clone(), seen_at_ms: 0, last_run_at_ms: Some(HOUR), ..TaskState::default() };
        assert!(task_status(&job, Some(&running), HOUR).running);
        let failed = TaskState { last_finished_at_ms: Some(HOUR + 5), last_error: Some("down".into()), ..running };
        let status = task_status(&job, Some(&failed), HOUR + 10);
        assert!(!status.running);
        assert_eq!((status.last_error.as_deref(), status.next_run_at_ms), (Some("down"), Some(2 * HOUR)));

        let broken = task_status(&TaskConfig { every: Some("soon".into()), ..task("x") }, None, 0);
        assert!(broken.schedule_error.is_some() && broken.next_run_at_ms.is_none());
    }

    #[test]
    fn the_local_switch_defaults_off_and_round_trips() {
        let dir = temp_dir();
        let path = dir.join("hub-local.json");
        assert_eq!(load_hub_local(&path).unwrap(), HubLocalConfig::default());
        save_hub_local(&path, &HubLocalConfig { run_tasks: true, lend: None }).unwrap();
        assert!(load_hub_local(&path).unwrap().run_tasks);
        std::fs::write(&path, "{}").unwrap();
        assert!(!load_hub_local(&path).unwrap().run_tasks);
        // A file from before P97 (only `run_tasks`) still reads, with nothing lent.
        std::fs::write(&path, r#"{ "run_tasks": true }"#).unwrap();
        assert_eq!(load_hub_local(&path).unwrap(), HubLocalConfig { run_tasks: true, lend: None });
        let lend = LendConfig { enabled: true, hub_url: "wss://vps.example.ts.net:7420".into(), shell: true, files: Some(dir.join("shared")), mcp: vec!["github".into()], ..LendConfig::default() };
        let both = HubLocalConfig { run_tasks: true, lend: Some(lend) };
        save_hub_local(&path, &both).unwrap();
        assert_eq!(load_hub_local(&path).unwrap(), both);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn a_run_speaks_as_the_agent_and_lands_in_the_task_conversation() {
        let dir = temp_dir();
        let base = Orchestrator::new(Arc::new(Echo { fail: false }), Arc::new(Vault::new(dir.join("vault"))));
        let config = FileConfig { agents: vec![agent("ana")], ..FileConfig::default() };
        let job = TaskConfig { agent: Some("ana".into()), timezone: Some("UTC".into()), ..task("daily") };
        let conversations = dir.join("conversations");

        let outcome = run_task(&base, &config, None, &job, &conversations, 0).await.unwrap();
        assert!(outcome.content.starts_with("I am ana|"), "{}", outcome.content);
        assert!(outcome.content.ends_with("[Scheduled task 'daily', 1970-01-01 00:00 UTC]\n\nsay hi"), "{}", outcome.content);

        let saved = load_conversation(&conversations, "task-daily").unwrap().unwrap();
        assert_eq!(saved.title, "Tarefa: daily");
        assert_eq!(saved.agent_id.as_deref(), Some("ana"));
        assert_eq!(saved.messages.len(), 2);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn only_the_last_runs_go_to_the_model() {
        let dir = temp_dir();
        let base = Orchestrator::new(Arc::new(Echo { fail: false }), Arc::new(Vault::new(dir.join("vault"))));
        let config = FileConfig::default();
        let conversations = dir.join("conversations");
        for _ in 0..12 {
            run_task(&base, &config, None, &task("daily"), &conversations, 0).await.unwrap();
        }
        let outcome = run_task(&base, &config, None, &task("daily"), &conversations, 0).await.unwrap();
        // The system context may add messages ahead; the history is capped at HISTORY_MESSAGES.
        let sent: usize = outcome.content.split('|').nth(1).unwrap().parse().unwrap();
        assert!(sent <= HISTORY_MESSAGES + 3, "sent {sent} messages");
        assert_eq!(load_conversation(&conversations, "task-daily").unwrap().unwrap().messages.len(), 26);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn a_failed_run_leaves_a_note() {
        let dir = temp_dir();
        let base = Orchestrator::new(Arc::new(Echo { fail: true }), Arc::new(Vault::new(dir.join("vault"))));
        let conversations = dir.join("conversations");
        assert!(run_task(&base, &FileConfig::default(), None, &task("daily"), &conversations, 0).await.is_err());
        let missing_agent = TaskConfig { agent: Some("gone".into()), ..task("other") };
        let err = run_task(&base, &FileConfig::default(), None, &missing_agent, &conversations, 0).await.unwrap_err();
        assert!(format!("{err:#}").contains("doesn't exist"), "{err:#}");

        let saved = load_conversation(&conversations, "task-other").unwrap().unwrap();
        assert_eq!(saved.messages[1].role, ChatRole::Assistant);
        assert!(saved.messages[1].content.starts_with("(could not run:"), "{}", saved.messages[1].content);
        std::fs::remove_dir_all(&dir).ok();
    }
}
