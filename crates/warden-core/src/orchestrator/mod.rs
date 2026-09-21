use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::budget::TurnBudget;
use crate::jobs::{JobBoard, JobsGuard};
use crate::memory::Vault;
use crate::model::{Attachment, Message, ModelProvider, StreamEvent, ToolCall, Usage};
use crate::tool::{Tool, ToolProvider};

/// Caps how many rounds of tool calls a single `handle_message` will chase before
/// giving up, so a model stuck requesting tools can't loop forever.
const MAX_TOOL_ITERATIONS: usize = 8;

/// Central coordinator: owns the model, the vault, and the registered tools.
/// Channels (CLI, Telegram, WhatsApp, ...) call `handle_message` and don't
/// know anything about which model or tools are behind it.
/// What `handle_message` hands back to the caller: the final answer, plus the summed token
/// usage across every `chat` call made along the way (a single message can trigger several,
/// one per round of tool calls). `None` only when the provider never reported usage at all —
/// not the case for OpenAI/Gemini today, but kept optional since `ModelProvider` doesn't
/// guarantee it.
#[derive(Debug, Clone)]
pub struct MessageOutcome {
    pub content: String,
    pub usage: Option<Usage>,
    /// Media (image/audio/video) extracted from an MCP tool's `CallToolResult` content blocks
    /// during this turn (P64 frente 2) — never round-tripped back into the model's own context
    /// (that would re-inflate every subsequent turn with base64), only surfaced here for the
    /// caller to persist/render. Empty when no tool call this turn produced any.
    pub attachments: Vec<Attachment>,
    /// Paths of files actually written to disk during this turn (P64) — `generate_document`'s
    /// own result, and any oversized MCP media `spill_oversized_media` saved instead of dumping
    /// inline. Lets a caller (the desktop's "Open" affordance) offer to open the file without
    /// having to scrape a path out of the model's free-text answer.
    pub generated_files: Vec<String>,
}

#[derive(Clone)]
pub struct Orchestrator {
    model: Arc<dyn ModelProvider>,
    vault: Arc<Vault>,
    tools: Vec<Arc<dyn Tool>>,
    /// Directory oversized MCP media (P64/P66 — over `MAX_INLINE_MEDIA_BYTES`) gets written to
    /// instead of being dumped inline. `None` for an `Orchestrator` built directly (tests,
    /// `warden-mcp-server`) rather than through `warden-bootstrap::bootstrap()`, which always
    /// sets this via `with_media_root`.
    media_root: Option<PathBuf>,
    /// The agent this orchestrator speaks as (P72 c) — only used to scope the skill catalog and
    /// `use_skill`. Set per turn through `with_agent`; `None` (every channel without agents) sees
    /// just the skills that aren't restricted to an agent.
    agent_id: Option<String>,
    /// How many model calls the sub-agents of one turn may make in total (P46/P60), or `None` for
    /// no limit. Set once at startup (`with_delegation_limit`); each turn gets a fresh `TurnBudget`.
    delegation_limit: Option<u32>,
    /// The budget of the turn this orchestrator is running in. Present on the root once the turn
    /// starts (it only adds the sub-agents' usage to its own) and on every sub-agent (`charged`).
    budget: Option<Arc<TurnBudget>>,
    /// This orchestrator runs as a sub-agent, so each of its model calls is charged to `budget`.
    charged: bool,
    /// How many background jobs (P46) one turn may run at once, or `None` for no background jobs.
    /// Set once at startup (`with_parallel_jobs`); each turn's root gets a fresh `JobBoard`.
    job_limit: Option<usize>,
}

impl Orchestrator {
    pub fn new(model: Arc<dyn ModelProvider>, vault: Arc<Vault>) -> Self {
        Self {
            model,
            vault,
            tools: Vec::new(),
            media_root: None,
            agent_id: None,
            delegation_limit: None,
            budget: None,
            charged: false,
            job_limit: None,
        }
    }

    pub fn register_tool(&mut self, tool: Arc<dyn Tool>) {
        self.tools.push(tool);
    }

    /// Registers every tool a `ToolProvider` currently exposes (e.g. an MCP server's
    /// `tools/list`). Snapshot at call time — a provider whose tool set changes later needs to
    /// be re-registered to pick up the change, there's no live sync.
    pub async fn register_provider(&mut self, provider: &dyn ToolProvider) -> anyhow::Result<()> {
        for tool in provider.tools().await? {
            self.register_tool(tool);
        }
        Ok(())
    }

    pub fn vault(&self) -> &Arc<Vault> {
        &self.vault
    }

    /// The model this orchestrator talks to — for callers that need a one-off completion outside
    /// the tool loop (the desktop's skill-draft generator, P16).
    pub fn model(&self) -> &Arc<dyn ModelProvider> {
        &self.model
    }

    /// Returns a copy of this orchestrator using a different model — cheap, since `model` is an
    /// `Arc` and the rest of `Self` is `Clone` over `Arc`s/a `Vec<Arc<_>>`. Lets a caller (the
    /// desktop's per-conversation model selector) swap the model for one call without re-running
    /// `bootstrap()` (which would reconnect MCP servers, redo OAuth, etc.).
    pub fn with_model(&self, model: Arc<dyn ModelProvider>) -> Self {
        Self { model, ..self.clone() }
    }

    /// Returns a copy of this orchestrator with one extra tool registered — cheap, same reasoning
    /// as `with_model`. Lets a caller attach a tool to one specific turn/agent (P46's opt-in
    /// `delegate_to_agent`) without touching the shared instance every other conversation uses.
    pub fn with_tool(&self, tool: Arc<dyn Tool>) -> Self {
        let mut clone = self.clone();
        clone.register_tool(tool);
        clone
    }

    /// Returns a copy of this orchestrator acting as agent `agent_id` (P72 c): the skill catalog
    /// and `use_skill` only expose skills that are global or list this agent. Same cheap-clone
    /// reasoning as `with_model`/`with_tool`; the `use_skill`/`read_skill_file` tools, if registered, are
    /// swapped for ones scoped to the agent (a no-op when it isn't registered).
    pub fn with_agent(&self, agent_id: Option<String>) -> Self {
        let mut clone = self.clone();
        for tool in &mut clone.tools {
            let store = crate::skill::SkillStore::new(self.vault.clone());
            match tool.spec().name.as_str() {
                "use_skill" => {
                    *tool = Arc::new(crate::tool::skill_tools::UseSkillTool::new(store).for_agent(agent_id.clone()))
                }
                "read_skill_file" => {
                    *tool = Arc::new(crate::tool::skill_tools::ReadSkillFileTool::new(store).for_agent(agent_id.clone()))
                }
                _ => {
                    if let Some(scoped) = tool.scoped_to_agent(agent_id.as_deref()) {
                        *tool = scoped;
                    }
                }
            }
        }
        clone.agent_id = agent_id;
        clone
    }

    /// Returns a copy of this orchestrator that only has the tools named in `allowed` (P46, tool
    /// isolation per agent); `None` keeps every tool. A tool outside the list is gone, not just
    /// hidden: the model can't be offered it and a call to it fails as an unknown tool. Tools that
    /// carry a nested orchestrator (`delegate_task`) are narrowed the same way, so a sub-agent can't
    /// be used to reach what the caller may not. Same cheap-clone reasoning as `with_agent`.
    pub fn with_allowed_tools(&self, allowed: Option<&[String]>) -> Self {
        let Some(allowed) = allowed else { return self.clone() };
        let mut clone = self.clone();
        clone.tools.retain(|tool| allowed.contains(&tool.spec().name));
        for tool in &mut clone.tools {
            if let Some(restricted) = tool.restricted_to(allowed) {
                *tool = restricted;
            }
        }
        clone
    }

    /// Returns a copy that lets the sub-agents of a single turn make at most `max_calls` model calls
    /// between them (P46/P60); `0` removes the limit. Every turn starts with the full amount.
    pub fn with_delegation_limit(&self, max_calls: u32) -> Self {
        Self { delegation_limit: (max_calls > 0).then_some(max_calls), ..self.clone() }
    }

    /// Returns a copy whose turns let the model start background jobs (P46): delegation calls accept
    /// `background: true`, and up to `max_parallel` of those sub-agents run at the same time while the
    /// rest wait their turn (`0` still means one at a time). Only takes effect when the `jobs` tool is
    /// among this orchestrator's tools — see `attach_jobs`.
    pub fn with_parallel_jobs(&self, max_parallel: usize) -> Self {
        Self { job_limit: Some(max_parallel), ..self.clone() }
    }

    /// Starts the background jobs of the turn this orchestrator is about to run: a fresh board bound
    /// to every tool that takes one (`Tool::with_jobs`). Returns the guard that cancels whatever is
    /// still running when the turn ends, or `None` — no board — when jobs are off or the `jobs` tool
    /// isn't here (an agent whose `allowed_tools` left it out must not start jobs it can't collect).
    fn attach_jobs(&mut self) -> Option<JobsGuard> {
        let limit = self.job_limit?;
        if !self.tools.iter().any(|t| t.spec().name == "jobs") {
            return None;
        }
        let board = JobBoard::new(limit);
        for tool in &mut self.tools {
            if let Some(bound) = tool.with_jobs(&board) {
                *tool = bound;
            }
        }
        Some(JobsGuard(board))
    }

    /// The root of a turn: sub-agents reached through its tools spend from `budget`, and their token
    /// usage is added to this orchestrator's own at the end of the turn.
    fn with_turn_budget(&self, budget: Arc<TurnBudget>) -> Self {
        let mut clone = self.clone();
        clone.budget = Some(budget);
        clone.share_budget_with_tools();
        clone
    }

    /// A sub-agent of the turn `budget` belongs to: every model call it makes is charged to it, and so
    /// is everything it delegates to in turn — one budget for the whole tree.
    pub fn charged_to(&self, budget: Arc<TurnBudget>) -> Self {
        let mut clone = self.clone();
        clone.budget = Some(budget);
        clone.charged = true;
        clone.share_budget_with_tools();
        clone
    }

    fn share_budget_with_tools(&mut self) {
        let Some(budget) = self.budget.clone() else { return };
        for tool in &mut self.tools {
            if let Some(charged) = tool.with_budget(&budget) {
                *tool = charged;
            }
        }
    }

    /// Returns a copy of this orchestrator whose tools that need a human "yes" (`ssh_*` on a host
    /// with `require_approval`) ask `approver`. Channels that can't ask simply never call this, and
    /// those tools refuse. Same cheap-clone reasoning as `with_agent`.
    pub fn with_approver(&self, approver: Arc<dyn crate::tool::Approver>) -> Self {
        let mut clone = self.clone();
        for tool in &mut clone.tools {
            if let Some(asking) = tool.with_approver(approver.clone()) {
                *tool = asking;
            }
        }
        clone
    }

    /// Returns a copy of this orchestrator that writes oversized MCP media (P64/P66) to `root`
    /// instead of dropping it — same cheap-clone reasoning as `with_model`/`with_tool`. Called
    /// once by `warden-bootstrap::bootstrap()` with the same "generated" directory
    /// `generate_document` already writes into.
    pub fn with_media_root(&self, root: PathBuf) -> Self {
        Self { media_root: Some(root), ..self.clone() }
    }

    /// Every tool currently registered — used by `warden-mcp-server` to re-expose this
    /// orchestrator's whole capability set (vault access, shell if enabled, whatever MCP servers
    /// were connected in `bootstrap()`, ...) as its own MCP server for third-party clients.
    pub fn tools(&self) -> &[Arc<dyn Tool>] {
        &self.tools
    }

    /// `history` is the prior turns of this conversation (user/assistant pairs, oldest first),
    /// as tracked by the caller — the orchestrator itself is stateless across calls. Pass `&[]`
    /// for a fresh conversation or a one-off sub-agent task.
    pub async fn handle_message(&self, history: &[Message], user_input: &str) -> anyhow::Result<MessageOutcome> {
        self.handle_turn(history, user_input, Vec::new(), None).await
    }

    /// Same as `handle_message`, but the current turn can carry image attachments (P28) —
    /// only meaningful to providers/models that support multimodal input.
    pub async fn handle_message_with_attachments(
        &self,
        history: &[Message],
        user_input: &str,
        attachments: Vec<Attachment>,
    ) -> anyhow::Result<MessageOutcome> {
        self.handle_turn(history, user_input, attachments, None).await
    }

    /// Same as `handle_message`, but forwards every `StreamEvent` to `on_event` as it arrives —
    /// the entry point a live UI (the CLI's ratatui-based interactive loop) uses to render
    /// content as it streams in, instead of waiting for the whole turn to finish. Every other
    /// caller (Telegram, WhatsApp, the desktop app, `DelegateTool`) keeps using `handle_message`,
    /// which is just this with a no-op sink.
    pub async fn handle_message_streaming(
        &self,
        history: &[Message],
        user_input: &str,
        on_event: impl FnMut(&StreamEvent) + Send,
    ) -> anyhow::Result<MessageOutcome> {
        self.handle_turn_streaming(history, user_input, Vec::new(), None, on_event).await
    }

    /// The general form both `handle_message` and `handle_message_with_attachments` wrap —
    /// `system_prompt` is an agent's persona (a per-conversation concept, only the desktop's
    /// `send_message` passes one; every other caller keeps getting `None`, unchanged behavior).
    /// Ignored when empty/whitespace-only, so an agent with a blank persona field behaves like no
    /// agent at all rather than sending a hollow system message.
    pub async fn handle_turn(
        &self,
        history: &[Message],
        user_input: &str,
        attachments: Vec<Attachment>,
        system_prompt: Option<&str>,
    ) -> anyhow::Result<MessageOutcome> {
        self.handle_turn_streaming(history, user_input, attachments, system_prompt, |_| {}).await
    }

    /// The general, streaming-capable form every other `handle_*` method wraps. Each round of the
    /// tool-call loop drives the model's `chat_stream` directly instead of the buffered `chat`,
    /// forwarding every `StreamEvent` to `on_event` live — a non-streaming caller just passes a
    /// no-op sink, so this is the one real implementation of the loop rather than two that could
    /// drift apart. One new failure mode versus the old buffered-JSON world: a connection can now
    /// drop *mid-stream*, after some content already arrived — that partial content is discarded
    /// and the call still surfaces as a plain `Err`, exactly as a connection failure would have
    /// looked before.
    pub async fn handle_turn_streaming(
        &self,
        history: &[Message],
        user_input: &str,
        attachments: Vec<Attachment>,
        system_prompt: Option<&str>,
        on_event: impl FnMut(&StreamEvent) + Send,
    ) -> anyhow::Result<MessageOutcome> {
        // A turn that starts here (not one a parent orchestrator started for a sub-agent, which
        // already carries its parent's budget) gets its own budget, so the limit applies whichever
        // channel called and starts from zero every turn.
        let mut turn = match (&self.budget, self.delegation_limit) {
            (None, Some(limit)) => self.with_turn_budget(TurnBudget::new(limit)),
            _ => self.clone(),
        };
        // Background jobs belong to the turn's root only: a sub-agent's tools stay unbound, so it
        // can't start jobs that would outlive its own short turn. Held until the turn ends (or its
        // future is dropped), which cancels the jobs nobody collected.
        let _jobs = if self.charged { None } else { turn.attach_jobs() };
        turn.run_turn(history, user_input, attachments, system_prompt, on_event).await
    }

    async fn run_turn(
        &self,
        history: &[Message],
        user_input: &str,
        attachments: Vec<Attachment>,
        system_prompt: Option<&str>,
        mut on_event: impl FnMut(&StreamEvent) + Send,
    ) -> anyhow::Result<MessageOutcome> {
        let mut messages = Vec::new();

        if let Some(persona) = system_prompt {
            if !persona.trim().is_empty() {
                messages.push(Message::system(persona.to_string()));
            }
        }

        // Fixed/standard memory (P52) — unlike the search block below, not keyed off relevance to
        // this turn's input: always injected in full when non-empty, so the model has standing
        // context (user profile, behavior rules, accumulated feedback) on every single turn, not
        // just when a grep/embedding match happens to surface it.
        let standing_memory = self.vault.standing_memory();
        if !standing_memory.is_empty() {
            messages.push(Message::system(format!(
                "Standing memory from the user's vault (always included, not relevance-dependent):\n\n{standing_memory}"
            )));
        }

        // Skill catalog (P16) — names + descriptions only; bodies load on demand via `use_skill`.
        // Read fresh from the vault each turn so a skill created mid-conversation shows up on the
        // next turn without rebuilding the orchestrator. Skipped when `use_skill` isn't registered
        // (an `Orchestrator` built without it), since the catalog would advertise an unusable tool.
        if self.tools.iter().any(|t| t.spec().name == "use_skill") {
            if let Some(catalog) = crate::skill::SkillStore::new(self.vault.clone()).catalog(self.agent_id.as_deref()) {
                messages.push(Message::system(catalog));
            }
        }

        // Semantic search runs ONNX inference (blocking, CPU-heavy) — `spawn_blocking` keeps it
        // off the async runtime thread. Falls back to the plain grep on any error (model download
        // failed offline, corrupt index, panic) so vault context injection never breaks outright.
        // Behind the `semantic-search` feature (default on) — see `warden-core/Cargo.toml`; every
        // real `Orchestrator` host (CLI/desktop/server/Telegram/WhatsApp) keeps it enabled, this
        // only matters for `warden-core` compiled with default features off (e.g. as a dependency
        // of `warden-sync`, which never constructs an `Orchestrator` at all).
        #[cfg(feature = "semantic-search")]
        let hits = {
            let vault_for_search = self.vault.clone();
            let query = user_input.to_string();
            tokio::task::spawn_blocking(move || vault_for_search.search_semantic(&query, 8))
                .await
                .ok()
                .and_then(|result| result.ok())
                .unwrap_or_else(|| self.vault.search(user_input, 8).unwrap_or_default())
        };
        #[cfg(not(feature = "semantic-search"))]
        let hits = self.vault.search(user_input, 8).unwrap_or_default();
        if !hits.is_empty() {
            let context = hits
                .iter()
                .map(|h| format!("[{}:{}] {}", h.path, h.line_number, h.line.trim()))
                .collect::<Vec<_>>()
                .join("\n");
            messages.push(Message::system(format!(
                "Relevant context found in the user's memory vault (may or may not be relevant — use your judgment):\n{context}"
            )));
        }

        messages.extend(history.iter().cloned());
        messages.push(Message::user_with_attachments(user_input, attachments));

        let mut usage = Usage::default();
        let mut has_usage = false;
        let mut attachments: Vec<Attachment> = Vec::new();
        let mut generated_files: Vec<String> = Vec::new();

        // Only sub-agents spend from the turn's budget (see `TurnBudget`).
        let sub_agent_budget = self.budget.as_ref().filter(|_| self.charged);

        for _ in 0..MAX_TOOL_ITERATIONS {
            if let Some(budget) = sub_agent_budget {
                budget.charge()?;
            }
            // Recomputed every iteration: a tool's spec can change mid-turn (`delegate_to_agent`
            // lists the agents `manage_agents` created a moment ago).
            let tool_specs = self.tools.iter().filter(|t| t.is_available()).map(|t| t.spec()).collect::<Vec<_>>();
            let stream = self.model.chat_stream(messages.clone(), tool_specs).await?;
            let response = crate::model::drain_chat_stream(stream, &mut on_event).await?;

            if let Some(budget) = sub_agent_budget {
                budget.record(response.usage.as_ref());
            }
            if let Some(u) = response.usage {
                usage.prompt_tokens += u.prompt_tokens;
                usage.completion_tokens += u.completion_tokens;
                usage.total_tokens += u.total_tokens;
                has_usage = true;
            }

            if response.tool_calls.is_empty() {
                // The turn's root also reports what its sub-agents used (P18).
                if let Some(sub_agents) = self.budget.as_ref().filter(|_| !self.charged).and_then(|b| b.usage()) {
                    usage += &sub_agents;
                    has_usage = true;
                }
                return Ok(MessageOutcome { content: response.content, usage: has_usage.then_some(usage), attachments, generated_files });
            }

            messages.push(Message::assistant_tool_calls(response.tool_calls.clone()));

            for tool_call in &response.tool_calls {
                let content = match self.run_tool(tool_call).await {
                    Ok(value) => {
                        let (content, extracted, files) = extract_media_from_tool_result(&value, self.media_root.as_deref());
                        attachments.extend(extracted);
                        generated_files.extend(files);
                        content
                    }
                    Err(err) => format!("error: {err:#}"),
                };
                messages.push(Message::tool_result(tool_call, content));
            }
        }

        anyhow::bail!("exceeded max tool-call iterations ({MAX_TOOL_ITERATIONS}) without a final answer")
    }

    async fn run_tool(&self, tool_call: &ToolCall) -> anyhow::Result<serde_json::Value> {
        let tool = self
            .tools
            .iter()
            .find(|t| t.spec().name == tool_call.name)
            .ok_or_else(|| anyhow::anyhow!("model requested unknown tool '{}'", tool_call.name))?;
        tool.call(tool_call.arguments.clone()).await
    }
}

/// Above this, an image/audio/video content block never becomes an `Attachment` (P64 frente 2) —
/// big enough for a still image or a short audio clip, not enough for real video. It's spilled to
/// disk instead (P64/P66 — `spill_oversized_media`) rather than dumped as raw base64 text into the
/// model's context. Checked against the base64 string's decoded byte length.
const MAX_INLINE_MEDIA_BYTES: usize = 8 * 1024 * 1024;

fn within_media_size_cap(base64_data: &str) -> bool {
    base64_data.len() * 3 / 4 <= MAX_INLINE_MEDIA_BYTES
}

/// Recognizes `generate_document`'s exact success shape (`{"status":"ok","path":...}`, the only
/// tool in the workspace that returns both fields together — confirmed by grepping every tool's
/// `json!(...)` result) inside an otherwise-unstructured tool result, so its path can feed the
/// same `generated_files` list as `spill_oversized_media`'s without misfiring on `write_file`
/// (`{"status":"ok"}`, no `path`) or any other tool.
fn generated_file_path(value: &Value) -> Option<String> {
    (value.get("status").and_then(Value::as_str) == Some("ok"))
        .then(|| value.get("path").and_then(Value::as_str).map(str::to_string))
        .flatten()
}

/// Turns a tool's raw JSON result into (text fed back to the model, media extracted as
/// attachments, paths of any file actually written to disk this call). Only tool results shaped
/// like an MCP `CallToolResult` (rmcp) — a `content` array where every item has a recognized
/// `type` (`text`/`image`/`audio`/`resource`/`resource_link`) — are parsed structurally; anything
/// else (`shell`, `read_file`, ...) falls through to the exact same `value.to_string()` this
/// replaced, unchanged, except for `generate_document`'s result specifically, which is recognized
/// by `generated_file_path` for the third return value (its text is still the plain
/// `value.to_string()` as before — this doesn't change what the model sees, only what a caller
/// can additionally act on).
///
/// `image`/`audio` blocks and `resource` blocks whose `mimeType` is image/audio/video become an
/// `Attachment` when their base64 payload fits `MAX_INLINE_MEDIA_BYTES` — the model only ever sees
/// a short placeholder for these (never the base64 itself, which would otherwise re-inflate every
/// subsequent turn's context). The same kind of block over that cap (typically video — MCP has no
/// dedicated `VideoContent`, it only ever arrives via `resource`) is written to disk under
/// `media_root` instead, via `spill_oversized_media`; the model sees a placeholder citing the file
/// path, the exact same "no chat affordance, the model just cites the path" convention already
/// used by `generate_document`/`write_file` — its real path also feeds the third return value. A
/// `resource_link` (a URI, no inline bytes) is deliberately never auto-fetched — fetching an
/// arbitrary URL an MCP server hands back would be an outbound network call driven by untrusted
/// tool output (SSRF-shaped risk) — so it always falls through as text, regardless of size.
fn extract_media_from_tool_result(value: &Value, media_root: Option<&Path>) -> (String, Vec<Attachment>, Vec<String>) {
    let known_type = |item: &Value| matches!(item.get("type").and_then(Value::as_str), Some("text" | "image" | "audio" | "resource" | "resource_link"));
    let Some(content) = value.get("content").and_then(Value::as_array).filter(|blocks| blocks.iter().all(known_type)) else {
        return (value.to_string(), Vec::new(), generated_file_path(value).into_iter().collect());
    };

    let mut text_parts = Vec::new();
    let mut attachments = Vec::new();
    let mut generated_files = Vec::new();

    for block in content {
        let block_type = block.get("type").and_then(Value::as_str).unwrap_or("");
        match block_type {
            "text" => {
                if let Some(text) = block.get("text").and_then(Value::as_str) {
                    text_parts.push(text.to_string());
                }
            }
            "image" | "audio" => {
                let data = block.get("data").and_then(Value::as_str);
                let mime_type = block.get("mimeType").and_then(Value::as_str);
                match (data, mime_type) {
                    (Some(data), Some(mime_type)) if within_media_size_cap(data) => {
                        attachments.push(Attachment { mime_type: mime_type.to_string(), data: data.to_string() });
                        text_parts.push(format!("[{block_type} attached: {mime_type}]"));
                    }
                    (Some(data), Some(mime_type)) => {
                        let (text, path) = spill_oversized_media(media_root, block_type, mime_type, data);
                        generated_files.extend(path);
                        text_parts.push(text);
                    }
                    _ => text_parts.push(block.to_string()),
                }
            }
            "resource" => {
                let resource = block.get("resource");
                let blob = resource.and_then(|r| r.get("blob")).and_then(Value::as_str);
                let mime_type = resource.and_then(|r| r.get("mimeType")).and_then(Value::as_str);
                let is_media_mime = |m: &str| m.starts_with("image/") || m.starts_with("audio/") || m.starts_with("video/");
                match (blob, mime_type) {
                    (Some(data), Some(mime_type)) if is_media_mime(mime_type) && within_media_size_cap(data) => {
                        attachments.push(Attachment { mime_type: mime_type.to_string(), data: data.to_string() });
                        text_parts.push(format!("[resource attached: {mime_type}]"));
                    }
                    (Some(data), Some(mime_type)) if is_media_mime(mime_type) => {
                        let (text, path) = spill_oversized_media(media_root, "resource", mime_type, data);
                        generated_files.extend(path);
                        text_parts.push(text);
                    }
                    _ => text_parts.push(block.to_string()),
                }
            }
            // "resource_link" (a URI, never auto-fetched) and any other block type.
            _ => text_parts.push(block.to_string()),
        }
    }

    (text_parts.join("\n"), attachments, generated_files)
}

/// Monotonic tie-breaker for `spill_oversized_media`'s filenames — a nanosecond timestamp alone
/// isn't guaranteed unique when a single tool result carries more than one oversized block (rare,
/// but a wrong guess here would silently overwrite a file). Same "timestamp + counter, no new
/// dependency" idiom `warden-bootstrap`'s test helper `temp_toml_path` already uses for the same
/// problem.
static MEDIA_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Maps a media MIME type to a filename extension for `spill_oversized_media` — covers the
/// subtypes the channels already know how to route (Telegram's `telegram_media_method`, the
/// WhatsApp sidecar's per-mime-prefix payload, mobile's `attachmentKindFor`), falling back to a
/// sanitized subtype for anything else so a written file is never left without an extension.
fn extension_for_mime(mime_type: &str) -> String {
    let subtype = mime_type.split('/').nth(1).unwrap_or("");
    match subtype {
        "quicktime" => "mov".to_string(),
        "mpeg" if mime_type.starts_with("audio/") => "mp3".to_string(),
        "jpeg" => "jpg".to_string(),
        "" => "bin".to_string(),
        other => {
            let sanitized: String = other.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
            if sanitized.is_empty() { "bin".to_string() } else { sanitized }
        }
    }
}

/// Handles an image/audio/video block whose base64 payload is over `MAX_INLINE_MEDIA_BYTES` —
/// spills it to disk under `media_root` (P64/P66) instead of the old behavior of dumping the raw
/// base64 string as text into the model's context, which only made a bad situation (too big to
/// attach) worse (also too big to keep as context). Returns the placeholder text that replaces
/// the block in what the model sees, plus — only on an actual successful write — `Some(path)` so
/// a caller can offer to open the real file (P64's desktop "Open" affordance) without having to
/// scrape the path back out of that placeholder text. Never panics and never falls back to
/// dumping the raw block — a decode/write failure just yields a short error placeholder and
/// `None` instead.
fn spill_oversized_media(media_root: Option<&Path>, block_type: &str, mime_type: &str, base64_data: &str) -> (String, Option<String>) {
    use base64::Engine;

    let bytes = match base64::engine::general_purpose::STANDARD.decode(base64_data) {
        Ok(bytes) => bytes,
        Err(_) => return (format!("[{block_type} block had malformed data — dropped]"), None),
    };
    let size = bytes.len();

    let Some(media_root) = media_root else {
        return (
            format!("[{block_type} too large to attach inline: {mime_type}, {size} bytes — no generated-file directory configured]"),
            None,
        );
    };

    let dir = media_root.join("mcp-media");
    if let Err(err) = std::fs::create_dir_all(&dir) {
        return (format!("[{block_type} too large to attach inline: {mime_type}, {size} bytes — failed to save to disk: {err:#}]"), None);
    }

    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let counter = MEDIA_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = dir.join(format!("{nanos}-{counter}.{}", extension_for_mime(mime_type)));

    match std::fs::write(&path, &bytes) {
        Ok(()) => {
            let path = path.display().to_string();
            (format!("[{block_type} too large to attach inline: {mime_type}, {size} bytes — saved to {path}]"), Some(path))
        }
        Err(err) => (format!("[{block_type} too large to attach inline: {mime_type}, {size} bytes — failed to save to disk: {err:#}]"), None),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use serde_json::json;

    use super::*;
    use crate::model::{ChatStream, Response, Role, response_stream};
    use crate::tool::ToolSpec;

    struct MockModel {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl ModelProvider for MockModel {
        async fn chat_stream(&self, messages: Vec<Message>, _tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            if call == 0 {
                Ok(response_stream(Response {
                    content: String::new(),
                    tool_calls: vec![ToolCall { id: "call_1".to_string(), name: "echo".to_string(), arguments: json!({ "text": "hi" }), thought_signature: None }],
                    usage: None,
                }))
            } else {
                let last = messages.last().expect("tool result should have been appended");
                assert_eq!(last.role, Role::Tool);
                assert!(last.content.contains("hi"));
                Ok(response_stream(Response { content: "done".to_string(), tool_calls: Vec::new(), usage: None }))
            }
        }
    }

    struct EchoTool;

    #[async_trait]
    impl Tool for EchoTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec { name: "echo".to_string(), description: "echoes text".to_string(), parameters: json!({}) }
        }

        async fn call(&self, args: serde_json::Value) -> anyhow::Result<serde_json::Value> {
            Ok(args)
        }
    }

    fn temp_vault() -> Arc<Vault> {
        Arc::new(Vault::new(std::env::temp_dir().join(format!(
            "warden-orch-test-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ))))
    }

    /// A fresh, not-yet-created directory for `spill_oversized_media` tests — `create_dir_all`
    /// happens lazily inside `spill_oversized_media` itself, same as `temp_vault`'s directory is
    /// only created on first write.
    fn temp_media_root() -> PathBuf {
        std::env::temp_dir().join(format!(
            "warden-orch-media-test-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ))
    }

    #[tokio::test]
    async fn executes_tool_calls_and_returns_final_answer() {
        let model = Arc::new(MockModel { calls: AtomicUsize::new(0) });
        let mut orchestrator = Orchestrator::new(model, temp_vault());
        orchestrator.register_tool(Arc::new(EchoTool));

        let result = orchestrator.handle_message(&[], "say hi").await.unwrap();
        assert_eq!(result.content, "done");
    }

    struct AlwaysToolCallModel;

    #[async_trait]
    impl ModelProvider for AlwaysToolCallModel {
        async fn chat_stream(&self, _messages: Vec<Message>, _tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
            Ok(response_stream(Response {
                content: String::new(),
                tool_calls: vec![ToolCall { id: "call_x".to_string(), name: "echo".to_string(), arguments: json!({}), thought_signature: None }],
                usage: None,
            }))
        }
    }

    #[tokio::test]
    async fn gives_up_after_max_iterations() {
        let mut orchestrator = Orchestrator::new(Arc::new(AlwaysToolCallModel), temp_vault());
        orchestrator.register_tool(Arc::new(EchoTool));

        let result = orchestrator.handle_message(&[], "loop forever").await;
        assert!(result.is_err());
    }

    struct TwoToolProvider;

    #[async_trait]
    impl ToolProvider for TwoToolProvider {
        async fn tools(&self) -> anyhow::Result<Vec<Arc<dyn Tool>>> {
            Ok(vec![Arc::new(EchoTool), Arc::new(EchoTool)])
        }
    }

    #[tokio::test]
    async fn register_provider_registers_every_tool_it_yields() {
        let model = Arc::new(MockModel { calls: AtomicUsize::new(0) });
        let mut orchestrator = Orchestrator::new(model, temp_vault());

        orchestrator.register_provider(&TwoToolProvider).await.unwrap();

        let result = orchestrator.handle_message(&[], "say hi").await.unwrap();
        assert_eq!(result.content, "done");
    }

    /// Echoes the first message's role/content back as its answer — lets a test assert on
    /// exactly what `handle_turn` built without needing a separate recorder/mutex.
    struct EchoesFirstMessageModel;

    #[async_trait]
    impl ModelProvider for EchoesFirstMessageModel {
        async fn chat_stream(&self, messages: Vec<Message>, _tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
            let first = messages.first().expect("at least one message");
            Ok(response_stream(Response { content: format!("{:?}:{}", first.role, first.content), tool_calls: Vec::new(), usage: None }))
        }
    }

    #[tokio::test]
    async fn handle_turn_prepends_the_persona_as_the_first_message() {
        let orchestrator = Orchestrator::new(Arc::new(EchoesFirstMessageModel), temp_vault());

        let result = orchestrator.handle_turn(&[], "hi", Vec::new(), Some("You are a pirate.")).await.unwrap();
        assert_eq!(result.content, "System:You are a pirate.");
    }

    #[tokio::test]
    async fn handle_turn_ignores_a_blank_persona() {
        let orchestrator = Orchestrator::new(Arc::new(EchoesFirstMessageModel), temp_vault());

        let result = orchestrator.handle_turn(&[], "hi", Vec::new(), Some("   ")).await.unwrap();
        assert_eq!(result.content, "User:hi");
    }

    /// Joins every message's role/content into one string, "|"-separated — lets a test assert on
    /// the full ordering `handle_turn` built, not just the first message.
    struct EchoesAllMessagesModel;

    #[async_trait]
    impl ModelProvider for EchoesAllMessagesModel {
        async fn chat_stream(&self, messages: Vec<Message>, _tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
            let joined = messages.iter().map(|m| format!("{:?}:{}", m.role, m.content)).collect::<Vec<_>>().join("|");
            Ok(response_stream(Response { content: joined, tool_calls: Vec::new(), usage: None }))
        }
    }

    #[tokio::test]
    async fn standing_memory_is_injected_between_persona_and_history() {
        let vault = temp_vault();
        vault.write("_profile.md", "Name: Ada.").unwrap();
        let orchestrator = Orchestrator::new(Arc::new(EchoesAllMessagesModel), vault);

        let history = [Message::user("earlier message".to_string())];
        let result =
            orchestrator.handle_turn(&history, "hi", Vec::new(), Some("You are a pirate.")).await.unwrap();

        let parts: Vec<&str> = result.content.split('|').collect();
        assert_eq!(parts[0], "System:You are a pirate.");
        assert_eq!(parts[1], "System:Standing memory from the user's vault (always included, not relevance-dependent):\n\n## User profile\n\nName: Ada.");
        assert_eq!(parts[2], "User:earlier message");
        assert_eq!(parts[3], "User:hi");
    }

    #[tokio::test]
    async fn no_standing_memory_message_when_vault_has_none_of_the_fixed_files() {
        let orchestrator = Orchestrator::new(Arc::new(EchoesAllMessagesModel), temp_vault());

        let result = orchestrator.handle_turn(&[], "hi", Vec::new(), None).await.unwrap();

        assert_eq!(result.content, "User:hi");
    }

    fn vault_with_skill() -> Arc<Vault> {
        let vault = temp_vault();
        let store = crate::skill::SkillStore::new(vault.clone());
        store
            .save(&crate::skill::Skill {
                name: "review-pr".into(),
                description: "Reviews a PR".into(),
                body: "Step 1.".into(),
                agents: Vec::new(),
            })
            .unwrap();
        vault
    }

    #[tokio::test]
    async fn skill_catalog_is_injected_after_standing_memory_when_use_skill_is_registered() {
        let vault = vault_with_skill();
        vault.write("_profile.md", "Name: Ada.").unwrap();
        let mut orchestrator = Orchestrator::new(Arc::new(EchoesAllMessagesModel), vault.clone());
        orchestrator.register_tool(Arc::new(crate::tool::skill_tools::UseSkillTool::new(
            crate::skill::SkillStore::new(vault),
        )));

        let result = orchestrator.handle_turn(&[], "hi", Vec::new(), None).await.unwrap();

        let parts: Vec<&str> = result.content.split('|').collect();
        assert!(parts[0].starts_with("System:Standing memory"));
        assert!(parts[1].starts_with("System:Available skills"));
        assert!(parts[1].contains("- review-pr: Reviews a PR"));
        assert_eq!(parts[2], "User:hi");
    }

    #[tokio::test]
    async fn no_skill_catalog_without_skills_or_without_the_use_skill_tool() {
        // Skills exist but the tool isn't registered.
        let orchestrator = Orchestrator::new(Arc::new(EchoesAllMessagesModel), vault_with_skill());
        let result = orchestrator.handle_turn(&[], "hi", Vec::new(), None).await.unwrap();
        assert_eq!(result.content, "User:hi");

        // Tool registered but no skills.
        let vault = temp_vault();
        let mut orchestrator = Orchestrator::new(Arc::new(EchoesAllMessagesModel), vault.clone());
        orchestrator.register_tool(Arc::new(crate::tool::skill_tools::UseSkillTool::new(
            crate::skill::SkillStore::new(vault),
        )));
        let result = orchestrator.handle_turn(&[], "hi", Vec::new(), None).await.unwrap();
        assert_eq!(result.content, "User:hi");
    }

    #[tokio::test]
    async fn with_agent_scopes_the_skill_catalog_to_that_agent() {
        let vault = vault_with_skill();
        crate::skill::SkillStore::new(vault.clone())
            .save(&crate::skill::Skill {
                name: "only-writer".into(),
                description: "Writer only".into(),
                body: "Write.".into(),
                agents: vec!["writer".into()],
            })
            .unwrap();
        let mut orchestrator = Orchestrator::new(Arc::new(EchoesAllMessagesModel), vault.clone());
        orchestrator.register_tool(Arc::new(crate::tool::skill_tools::UseSkillTool::new(
            crate::skill::SkillStore::new(vault),
        )));

        let plain = orchestrator.handle_turn(&[], "hi", Vec::new(), None).await.unwrap().content;
        assert!(plain.contains("review-pr") && !plain.contains("only-writer"));

        let as_writer = orchestrator.with_agent(Some("writer".into()));
        let scoped = as_writer.handle_turn(&[], "hi", Vec::new(), None).await.unwrap().content;
        assert!(scoped.contains("review-pr") && scoped.contains("only-writer"));

        let as_other = orchestrator.with_agent(Some("reviewer".into()));
        let other = as_other.handle_turn(&[], "hi", Vec::new(), None).await.unwrap().content;
        assert!(!other.contains("only-writer"));

        // The swapped-in `use_skill` is scoped too, not just the catalog.
        let tool = as_other.tools().iter().find(|t| t.spec().name == "use_skill").unwrap();
        assert!(tool.call(serde_json::json!({ "name": "only-writer" })).await.is_err());
        let tool = as_writer.tools().iter().find(|t| t.spec().name == "use_skill").unwrap();
        assert!(tool.call(serde_json::json!({ "name": "only-writer" })).await.is_ok());
    }

    /// Replies with the names of the tools it was offered, comma-separated.
    struct EchoesToolNamesModel;

    #[async_trait]
    impl ModelProvider for EchoesToolNamesModel {
        async fn chat_stream(&self, _messages: Vec<Message>, tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
            let names = tools.iter().map(|t| t.name.clone()).collect::<Vec<_>>().join(",");
            Ok(response_stream(Response { content: names, tool_calls: Vec::new(), usage: None }))
        }
    }

    #[tokio::test]
    async fn with_agent_hides_ssh_exec_from_an_agent_that_cannot_reach_any_host() {
        use crate::tool::ssh::{SshHost, SshTool};
        let host = |id: &str, agents: &[&str]| SshHost {
            id: id.into(),
            host: "example.com".into(),
            user: "deploy".into(),
            port: 22,
            identity_file: None,
            agents: agents.iter().map(|a| a.to_string()).collect(),
            require_approval: false,
        };
        let mut orchestrator = Orchestrator::new(Arc::new(EchoesToolNamesModel), temp_vault());
        orchestrator.register_tool(Arc::new(SshTool::exec(vec![host("prod", &["ops"])])));

        // No agent, and a different agent: the tool isn't even advertised.
        let none = orchestrator.handle_turn(&[], "hi", Vec::new(), None).await.unwrap().content;
        assert!(!none.contains("ssh_exec"));
        let other = orchestrator.with_agent(Some("writer".into())).handle_turn(&[], "hi", Vec::new(), None).await.unwrap().content;
        assert!(!other.contains("ssh_exec"));

        // The allowed agent gets it, and re-scoping an already-scoped copy doesn't lose the host.
        let ops = orchestrator.with_agent(Some("ops".into()));
        assert!(ops.handle_turn(&[], "hi", Vec::new(), None).await.unwrap().content.contains("ssh_exec"));
        let back_and_forth = ops.with_agent(Some("writer".into())).with_agent(Some("ops".into()));
        assert!(back_and_forth.handle_turn(&[], "hi", Vec::new(), None).await.unwrap().content.contains("ssh_exec"));
    }

    /// Its description changes once it has been called — like `delegate_to_agent` gaining an agent.
    struct ChangesItsSpecWhenCalled(Arc<AtomicUsize>);

    #[async_trait]
    impl Tool for ChangesItsSpecWhenCalled {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "evolving".to_string(),
                description: format!("called {} times", self.0.load(Ordering::SeqCst)),
                parameters: serde_json::json!({}),
            }
        }
        async fn call(&self, _args: Value) -> anyhow::Result<Value> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(serde_json::json!("ok"))
        }
    }

    /// Calls `evolving` once, then answers with the description it saw on the second look.
    struct ReportsToolDescriptionOnSecondCall(AtomicUsize);

    #[async_trait]
    impl ModelProvider for ReportsToolDescriptionOnSecondCall {
        async fn chat_stream(&self, _messages: Vec<Message>, tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
            if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
                return Ok(response_stream(Response {
                    content: String::new(),
                    tool_calls: vec![ToolCall { id: "1".into(), name: "evolving".into(), arguments: serde_json::json!({}), thought_signature: None }],
                    usage: None,
                }));
            }
            let seen = tools.iter().find(|t| t.name == "evolving").map(|t| t.description.clone()).unwrap_or_default();
            Ok(response_stream(Response { content: seen, tool_calls: Vec::new(), usage: None }))
        }
    }

    #[tokio::test]
    async fn a_tool_spec_that_changes_mid_turn_is_seen_by_the_next_model_call() {
        let mut orchestrator = Orchestrator::new(Arc::new(ReportsToolDescriptionOnSecondCall(AtomicUsize::new(0))), temp_vault());
        orchestrator.register_tool(Arc::new(ChangesItsSpecWhenCalled(Arc::new(AtomicUsize::new(0)))));

        let result = orchestrator.handle_turn(&[], "hi", Vec::new(), None).await.unwrap();
        assert_eq!(result.content, "called 1 times");
    }

    /// A sub-agent's model: every call takes 150 ms and the number running at once is recorded. Long
    /// enough that jobs reaching it a few ms apart (each first searches the vault) still overlap.
    struct SlowSubAgentModel {
        active: Arc<AtomicUsize>,
        peak: Arc<AtomicUsize>,
        tools_seen: Arc<std::sync::Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl ModelProvider for SlowSubAgentModel {
        async fn chat_stream(&self, _messages: Vec<Message>, tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
            self.tools_seen.lock().unwrap().extend(tools.into_iter().map(|t| t.name));
            let now = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(now, Ordering::SeqCst);
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            self.active.fetch_sub(1, Ordering::SeqCst);
            Ok(response_stream(Response { content: "sub-agent done".to_string(), tool_calls: Vec::new(), usage: None }))
        }
    }

    /// The chief: starts three background jobs, reads all three, then reports what came back.
    struct ChiefStartsThreeJobs;

    #[async_trait]
    impl ModelProvider for ChiefStartsThreeJobs {
        async fn chat_stream(&self, messages: Vec<Message>, tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
            let call = |id: &str, name: &str, arguments: Value| ToolCall { id: id.into(), name: name.into(), arguments, thought_signature: None };
            let results: Vec<String> = messages.iter().filter(|m| m.role == Role::Tool).map(|m| m.content.clone()).collect();
            let response = match results.len() {
                0 => {
                    let offered = tools.iter().find(|t| t.name == "delegate_task").is_some_and(|t| t.parameters["properties"].get("background").is_some());
                    if !offered {
                        Response { content: "background not offered".to_string(), tool_calls: Vec::new(), usage: None }
                    } else {
                        let start = |n: usize| call(&format!("start_{n}"), "delegate_task", json!({ "task": format!("task {n}"), "background": true }));
                        Response { content: String::new(), tool_calls: vec![start(1), start(2), start(3)], usage: None }
                    }
                }
                3 => Response {
                    content: String::new(),
                    tool_calls: (1..=3).map(|n| call(&format!("read_{n}"), "jobs", json!({ "action": "result", "job_id": format!("job-{n}") }))).collect(),
                    usage: None,
                },
                _ => Response { content: results[3..].join(" | "), tool_calls: Vec::new(), usage: None },
            };
            Ok(response_stream(response))
        }
    }

    fn chief_with_jobs(sub_model: Arc<dyn ModelProvider>, limit: usize) -> Orchestrator {
        use crate::tool::delegate::DelegateTool;
        use crate::tool::job_tools::JobsTool;

        let mut sub = Orchestrator::new(sub_model, temp_vault());
        sub.register_tool(Arc::new(JobsTool::new()));
        let mut chief = Orchestrator::new(Arc::new(ChiefStartsThreeJobs), temp_vault());
        chief.register_tool(Arc::new(DelegateTool::new(sub)));
        chief.register_tool(Arc::new(JobsTool::new()));
        chief.with_parallel_jobs(limit)
    }

    #[tokio::test]
    async fn background_jobs_run_in_parallel_up_to_the_limit_and_are_collected() {
        let (active, peak) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let sub = Arc::new(SlowSubAgentModel { active, peak: peak.clone(), tools_seen: Default::default() });
        let chief = chief_with_jobs(sub, 2);

        let answer = chief.handle_message(&[], "go").await.unwrap().content;

        assert_eq!(answer.matches("sub-agent done").count(), 3, "{answer}");
        assert_eq!(peak.load(Ordering::SeqCst), 2, "three jobs under a limit of 2 should peak at 2 at once");
    }

    #[tokio::test]
    async fn without_the_jobs_tool_background_is_not_offered() {
        use crate::tool::delegate::DelegateTool;

        let sub = Arc::new(SlowSubAgentModel { active: Default::default(), peak: Default::default(), tools_seen: Default::default() });
        let mut chief = Orchestrator::new(Arc::new(ChiefStartsThreeJobs), temp_vault());
        chief.register_tool(Arc::new(DelegateTool::new(Orchestrator::new(sub, temp_vault()))));

        let answer = chief.with_parallel_jobs(2).handle_message(&[], "go").await.unwrap().content;

        assert_eq!(answer, "background not offered");
    }

    #[tokio::test]
    async fn background_is_not_offered_when_jobs_are_not_configured() {
        use crate::tool::delegate::DelegateTool;
        use crate::tool::job_tools::JobsTool;

        let sub = Arc::new(SlowSubAgentModel { active: Default::default(), peak: Default::default(), tools_seen: Default::default() });
        let mut chief = Orchestrator::new(Arc::new(ChiefStartsThreeJobs), temp_vault());
        chief.register_tool(Arc::new(DelegateTool::new(Orchestrator::new(sub, temp_vault()))));
        chief.register_tool(Arc::new(JobsTool::new()));

        // No `with_parallel_jobs`: behaves exactly as before jobs existed.
        assert_eq!(chief.handle_message(&[], "go").await.unwrap().content, "background not offered");
    }

    #[tokio::test]
    async fn a_sub_agent_is_never_offered_jobs_or_background() {
        let tools_seen: Arc<std::sync::Mutex<Vec<String>>> = Default::default();
        let sub = Arc::new(SlowSubAgentModel { active: Default::default(), peak: Default::default(), tools_seen: tools_seen.clone() });

        chief_with_jobs(sub, 2).handle_message(&[], "go").await.unwrap();

        assert!(!tools_seen.lock().unwrap().iter().any(|name| name == "jobs"), "{:?}", tools_seen.lock().unwrap());
    }

    /// Never answers: stands in for a job that is still running when the turn ends.
    struct HangingSubAgent {
        dropped: Arc<AtomicUsize>,
        entered: Arc<AtomicUsize>,
    }

    struct CountsDrops(Arc<AtomicUsize>);
    impl Drop for CountsDrops {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[async_trait]
    impl ModelProvider for HangingSubAgent {
        async fn chat_stream(&self, _messages: Vec<Message>, _tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
            let _guard = CountsDrops(self.dropped.clone());
            self.entered.fetch_add(1, Ordering::SeqCst);
            std::future::pending::<()>().await;
            unreachable!()
        }
    }

    /// Starts one job and answers straight away, without ever reading it.
    struct ChiefForgetsItsJob {
        /// Counts the hung job's entries into its model call.
        job_entered: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl ModelProvider for ChiefForgetsItsJob {
        async fn chat_stream(&self, messages: Vec<Message>, _tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
            let response = if messages.iter().any(|m| m.role == Role::Tool) {
                // Don't answer until the job is really running (inside its model call), so ending the
                // turn has something to cancel. Bounded: a job that never gets there fails the test below.
                for _ in 0..500 {
                    if self.job_entered.load(Ordering::SeqCst) > 0 {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
                Response { content: "answered without waiting".to_string(), tool_calls: Vec::new(), usage: None }
            } else {
                let start = ToolCall { id: "1".into(), name: "delegate_task".into(), arguments: json!({ "task": "slow", "background": true }), thought_signature: None };
                Response { content: String::new(), tool_calls: vec![start], usage: None }
            };
            Ok(response_stream(response))
        }
    }

    #[tokio::test]
    async fn a_job_nobody_collects_is_cancelled_when_the_turn_ends() {
        use crate::tool::delegate::DelegateTool;
        use crate::tool::job_tools::JobsTool;

        let (dropped, entered) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let hanging = HangingSubAgent { dropped: dropped.clone(), entered: entered.clone() };
        let mut chief = Orchestrator::new(Arc::new(ChiefForgetsItsJob { job_entered: entered.clone() }), temp_vault());
        chief.register_tool(Arc::new(DelegateTool::new(Orchestrator::new(Arc::new(hanging), temp_vault()))));
        chief.register_tool(Arc::new(JobsTool::new()));

        // The turn is not held up by the hung job...
        let answer = tokio::time::timeout(std::time::Duration::from_secs(5), chief.with_parallel_jobs(2).handle_message(&[], "go"))
            .await
            .expect("the turn must not wait for a job it never asked for")
            .unwrap()
            .content;
        assert_eq!(answer, "answered without waiting");
        assert_eq!(entered.load(Ordering::SeqCst), 1, "the job never started running, so there was nothing to cancel");

        // ...and the job's work was stopped, not left running in the background.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn background_jobs_spend_from_the_turns_shared_budget() {
        let sub = Arc::new(SlowSubAgentModel { active: Default::default(), peak: Default::default(), tools_seen: Default::default() });
        // Room for two sub-agent model calls in the whole turn; three jobs each want one.
        let chief = chief_with_jobs(sub, 3).with_delegation_limit(2);

        let outcome = chief.handle_message(&[], "go").await.unwrap();

        assert_eq!(outcome.content.matches("sub-agent done").count(), 2, "{}", outcome.content);
        assert_eq!(outcome.content.matches("limit of 2 model calls").count(), 1, "{}", outcome.content);
    }

    struct NamedTool(&'static str);

    #[async_trait]
    impl Tool for NamedTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec { name: self.0.to_string(), description: String::new(), parameters: serde_json::json!({}) }
        }
        async fn call(&self, _args: Value) -> anyhow::Result<Value> {
            Ok(serde_json::json!("ran"))
        }
    }

    #[tokio::test]
    async fn with_allowed_tools_drops_every_tool_outside_the_list() {
        let mut orchestrator = Orchestrator::new(Arc::new(EchoesToolNamesModel), temp_vault());
        for name in ["read_file", "shell", "write_file"] {
            orchestrator.register_tool(Arc::new(NamedTool(name)));
        }

        let all = orchestrator.with_allowed_tools(None);
        assert_eq!(all.handle_turn(&[], "hi", Vec::new(), None).await.unwrap().content, "read_file,shell,write_file");

        let allowed = vec!["read_file".to_string(), "ghost".to_string()];
        let narrowed = orchestrator.with_allowed_tools(Some(&allowed));
        assert_eq!(narrowed.handle_turn(&[], "hi", Vec::new(), None).await.unwrap().content, "read_file");
        // Gone, not just hidden: a call to it is an unknown tool.
        let err = narrowed.run_tool(&ToolCall { id: "1".into(), name: "shell".into(), arguments: serde_json::json!({}), thought_signature: None }).await.unwrap_err();
        assert!(err.to_string().contains("unknown tool"));
        // The original is untouched.
        assert_eq!(orchestrator.tools().len(), 3);

        let none: Vec<String> = Vec::new();
        assert!(orchestrator.with_allowed_tools(Some(&none)).tools().is_empty());
    }

    #[tokio::test]
    async fn with_allowed_tools_also_narrows_the_sub_agent_behind_delegate_task() {
        use crate::tool::delegate::DelegateTool;
        let mut inner = Orchestrator::new(Arc::new(EchoesToolNamesModel), temp_vault());
        for name in ["read_file", "shell"] {
            inner.register_tool(Arc::new(NamedTool(name)));
        }
        let mut outer = Orchestrator::new(Arc::new(EchoesToolNamesModel), temp_vault());
        outer.register_tool(Arc::new(DelegateTool::new(inner)));
        let delegate_only = |o: &Orchestrator| o.tools().iter().find(|t| t.spec().name == "delegate_task").unwrap().clone();

        // Unrestricted: the sub-agent is offered both tools.
        let result = delegate_only(&outer).call(serde_json::json!({ "task": "x" })).await.unwrap();
        assert_eq!(result["result"], "read_file,shell");

        // Allowed to delegate, but not to reach `shell` through the sub-agent.
        let allowed = vec!["delegate_task".to_string(), "read_file".to_string()];
        let restricted = outer.with_allowed_tools(Some(&allowed));
        let result = delegate_only(&restricted).call(serde_json::json!({ "task": "x" })).await.unwrap();
        assert_eq!(result["result"], "read_file");
    }

    /// Scripted by what it is offered, and it reports 1+1 tokens per call. Whoever can `delegate_task`
    /// delegates once and then repeats what came back; a leaf with a `noop` tool keeps calling it
    /// (burns model calls until something stops it); any other leaf just answers "leaf".
    struct Delegator {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl ModelProvider for Delegator {
        async fn chat_stream(&self, messages: Vec<Message>, tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let offered = |name: &str| tools.iter().any(|t| t.name == name);
            let call = |name: &str, args: serde_json::Value| vec![ToolCall { id: "c".into(), name: name.into(), arguments: args, thought_signature: None }];
            let (content, tool_calls) = if offered("delegate_task") {
                if messages.last().is_some_and(|m| m.role == Role::Tool) {
                    (format!("got: {}", messages.last().unwrap().content), Vec::new())
                } else {
                    (String::new(), call("delegate_task", json!({ "task": "go" })))
                }
            } else if offered("noop") {
                (String::new(), call("noop", json!({})))
            } else {
                ("leaf".to_string(), Vec::new())
            };
            let usage = Some(Usage { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 });
            Ok(response_stream(Response { content, tool_calls, usage }))
        }
    }

    /// A root that can delegate to one sub-agent (with a `noop` tool, so it keeps burning calls).
    fn root_with_burning_sub_agent(model: Arc<Delegator>, limit: u32) -> Orchestrator {
        use crate::tool::delegate::DelegateTool;
        let mut sub = Orchestrator::new(model.clone(), temp_vault());
        sub.register_tool(Arc::new(NamedTool("noop")));
        let mut root = Orchestrator::new(model, temp_vault());
        root.register_tool(Arc::new(DelegateTool::new(sub)));
        root.with_delegation_limit(limit)
    }

    #[tokio::test]
    async fn the_turn_budget_stops_a_sub_agent_and_the_root_still_answers() {
        let model = Arc::new(Delegator { calls: AtomicUsize::new(0) });
        let root = root_with_burning_sub_agent(model.clone(), 3);

        let outcome = root.handle_message(&[], "go").await.unwrap();

        // The sub-agent's refusal reaches the root as a tool error, and the root answers with it.
        assert!(outcome.content.contains("limit of 3 model calls"), "{}", outcome.content);
        // root, 3 charged sub-agent calls (the fourth was refused before reaching the model), root again.
        assert_eq!(model.calls.load(Ordering::SeqCst), 5);
    }

    #[tokio::test]
    async fn every_turn_starts_with_a_fresh_budget() {
        let model = Arc::new(Delegator { calls: AtomicUsize::new(0) });
        let root = root_with_burning_sub_agent(model.clone(), 3);

        root.handle_message(&[], "one").await.unwrap();
        let after_first = model.calls.load(Ordering::SeqCst);
        root.handle_message(&[], "two").await.unwrap();

        assert_eq!(model.calls.load(Ordering::SeqCst) - after_first, 5);
    }

    #[tokio::test]
    async fn without_a_limit_nothing_is_capped_or_counted() {
        let model = Arc::new(Delegator { calls: AtomicUsize::new(0) });
        let mut sub = Orchestrator::new(model.clone(), temp_vault());
        sub.register_tool(Arc::new(NamedTool("noop")));
        let mut root = Orchestrator::new(model.clone(), temp_vault());
        root.register_tool(Arc::new(crate::tool::delegate::DelegateTool::new(sub)));

        // No budget: the sub-agent burns its whole iteration allowance, then its own cap ends it.
        let outcome = root.handle_message(&[], "go").await.unwrap();
        assert!(outcome.content.contains("exceeded max tool-call iterations"), "{}", outcome.content);
        assert_eq!(model.calls.load(Ordering::SeqCst), 1 + MAX_TOOL_ITERATIONS + 1);
        // And a zero limit means "no limit", not "nothing allowed".
        assert!(root.with_delegation_limit(0).delegation_limit.is_none());
    }

    #[tokio::test]
    async fn sub_agent_tokens_are_added_to_the_roots_usage() {
        use crate::tool::delegate::DelegateTool;
        let model = Arc::new(Delegator { calls: AtomicUsize::new(0) });
        let leaf = Orchestrator::new(model.clone(), temp_vault()); // answers "leaf" in one call
        let mut root = Orchestrator::new(model.clone(), temp_vault());
        root.register_tool(Arc::new(DelegateTool::new(leaf)));

        let outcome = root.with_delegation_limit(10).handle_message(&[], "go").await.unwrap();

        // Two root calls and one sub-agent call, 2 tokens each.
        assert_eq!(outcome.usage, Some(Usage { prompt_tokens: 3, completion_tokens: 3, total_tokens: 6 }));
        // The same turn without a budget still drops the sub-agent's share (nothing to add it up).
        let unlimited = root.handle_message(&[], "go").await.unwrap();
        assert_eq!(unlimited.usage, Some(Usage { prompt_tokens: 2, completion_tokens: 2, total_tokens: 4 }));
    }

    #[tokio::test]
    async fn a_chain_of_sub_agents_shares_one_budget() {
        use crate::tool::delegate::DelegateTool;
        // root -> level1 -> leaf. Charged calls: level1 (delegates), leaf (answers), level1 (answers).
        let build = |limit: u32| {
            let model = Arc::new(Delegator { calls: AtomicUsize::new(0) });
            let leaf = Orchestrator::new(model.clone(), temp_vault());
            let mut level1 = Orchestrator::new(model.clone(), temp_vault());
            level1.register_tool(Arc::new(DelegateTool::new(leaf)));
            let mut root = Orchestrator::new(model, temp_vault());
            root.register_tool(Arc::new(DelegateTool::new(level1)));
            root.with_delegation_limit(limit)
        };

        let enough = build(3).handle_message(&[], "go").await.unwrap();
        assert!(enough.content.contains("got: {\"result\":\"got: {\\\"result\\\":\\\"leaf\\\"}\"}"), "{}", enough.content);

        // One short: level1's second call is refused, whichever level made the earlier ones.
        let short = build(2).handle_message(&[], "go").await.unwrap();
        assert!(short.content.contains("limit of 2 model calls"), "{}", short.content);
    }

    #[tokio::test]
    async fn with_agent_scopes_read_skill_file_too() {
        let vault = vault_with_skill();
        let store = crate::skill::SkillStore::new(vault.clone());
        store
            .save(&crate::skill::Skill {
                name: "only-writer".into(),
                description: "Writer only".into(),
                body: "Write.".into(),
                agents: vec!["writer".into()],
            })
            .unwrap();
        store.save_file("only-writer", "a.txt", "secret").unwrap();
        let mut orchestrator = Orchestrator::new(Arc::new(EchoesAllMessagesModel), vault);
        orchestrator.register_tool(Arc::new(crate::tool::skill_tools::ReadSkillFileTool::new(store)));
        let args = serde_json::json!({ "skill": "only-writer", "file": "a.txt" });

        let other = orchestrator.with_agent(Some("reviewer".into()));
        let tool = other.tools().iter().find(|t| t.spec().name == "read_skill_file").unwrap();
        assert!(tool.call(args.clone()).await.is_err());

        let writer = orchestrator.with_agent(Some("writer".into()));
        let tool = writer.tools().iter().find(|t| t.spec().name == "read_skill_file").unwrap();
        assert!(tool.call(args).await.is_ok());
    }

    struct NamedModel {
        name: &'static str,
    }

    #[async_trait]
    impl ModelProvider for NamedModel {
        async fn chat_stream(&self, _messages: Vec<Message>, _tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
            Ok(response_stream(Response { content: self.name.to_string(), tool_calls: Vec::new(), usage: None }))
        }
    }

    #[tokio::test]
    async fn with_model_swaps_the_model_used_without_touching_the_original() {
        let orchestrator = Orchestrator::new(Arc::new(NamedModel { name: "a" }), temp_vault());
        let swapped = orchestrator.with_model(Arc::new(NamedModel { name: "b" }));

        assert_eq!(orchestrator.handle_message(&[], "hi").await.unwrap().content, "a");
        assert_eq!(swapped.handle_message(&[], "hi").await.unwrap().content, "b");
    }

    /// Emits its events directly (not via `response_stream`) so tests can assert on the exact
    /// sequence `handle_turn_streaming` forwards to a live callback, not just the final result.
    struct MultiDeltaModel;

    #[async_trait]
    impl ModelProvider for MultiDeltaModel {
        async fn chat_stream(&self, _messages: Vec<Message>, _tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
            let events: Vec<anyhow::Result<StreamEvent>> = vec![
                Ok(StreamEvent::ContentDelta("Hel".to_string())),
                Ok(StreamEvent::ContentDelta("lo".to_string())),
                Ok(StreamEvent::Usage(Usage { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 })),
            ];
            Ok(Box::pin(futures_util::stream::iter(events)))
        }
    }

    #[tokio::test]
    async fn handle_message_streaming_forwards_every_event_in_order() {
        let orchestrator = Orchestrator::new(Arc::new(MultiDeltaModel), temp_vault());
        let observed = std::sync::Mutex::new(Vec::new());

        let result = orchestrator.handle_message_streaming(&[], "hi", |event| observed.lock().unwrap().push(event.clone())).await.unwrap();

        assert_eq!(result.content, "Hello");
        assert_eq!(result.usage, Some(Usage { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 }));

        let observed = observed.into_inner().unwrap();
        assert_eq!(observed.len(), 3);
        assert!(matches!(&observed[0], StreamEvent::ContentDelta(s) if s == "Hel"));
        assert!(matches!(&observed[1], StreamEvent::ContentDelta(s) if s == "lo"));
        assert!(matches!(&observed[2], StreamEvent::Usage(_)));
    }

    /// A non-streaming caller (`handle_message`, no-op sink) must still get exactly the same
    /// `MessageOutcome` as before streaming existed — proof the two paths haven't drifted apart.
    #[tokio::test]
    async fn handle_message_still_returns_the_full_accumulated_outcome() {
        let orchestrator = Orchestrator::new(Arc::new(MultiDeltaModel), temp_vault());
        let result = orchestrator.handle_message(&[], "hi").await.unwrap();
        assert_eq!(result.content, "Hello");
        assert_eq!(result.usage, Some(Usage { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 }));
    }

    struct MidStreamErrorModel;

    #[async_trait]
    impl ModelProvider for MidStreamErrorModel {
        async fn chat_stream(&self, _messages: Vec<Message>, _tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
            let events: Vec<anyhow::Result<StreamEvent>> = vec![Ok(StreamEvent::ContentDelta("partial".to_string())), Err(anyhow::anyhow!("connection dropped"))];
            Ok(Box::pin(futures_util::stream::iter(events)))
        }
    }

    #[tokio::test]
    async fn a_mid_stream_error_propagates_as_an_err() {
        let orchestrator = Orchestrator::new(Arc::new(MidStreamErrorModel), temp_vault());
        let result = orchestrator.handle_message(&[], "hi").await;
        assert!(result.is_err());
    }

    // --- P64 frente 2: media extracted from an MCP-shaped tool result ---

    /// Returns whatever `serde_json::Value` is handed to it in its constructor — lets a test drive
    /// an arbitrary MCP `CallToolResult`-shaped (or not) payload through the orchestrator's tool
    /// loop without needing a real MCP server.
    struct FixedResultTool(Value);

    #[async_trait]
    impl Tool for FixedResultTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec { name: "mcp_tool".to_string(), description: "returns a fixed value".to_string(), parameters: json!({}) }
        }

        async fn call(&self, _args: Value) -> anyhow::Result<Value> {
            Ok(self.0.clone())
        }
    }

    /// Calls `mcp_tool` once, then asserts the tool-result message text (what the model itself
    /// would see next) matches `expects`, before returning a final plain answer.
    struct AssertsToolResultTextModel {
        calls: AtomicUsize,
        expects: &'static str,
    }

    #[async_trait]
    impl ModelProvider for AssertsToolResultTextModel {
        async fn chat_stream(&self, messages: Vec<Message>, _tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            if call == 0 {
                Ok(response_stream(Response {
                    content: String::new(),
                    tool_calls: vec![ToolCall { id: "call_1".to_string(), name: "mcp_tool".to_string(), arguments: json!({}), thought_signature: None }],
                    usage: None,
                }))
            } else {
                let last = messages.last().expect("tool result should have been appended");
                assert_eq!(last.role, Role::Tool);
                assert!(last.content.contains(self.expects), "expected tool-result text to contain {:?}, got {:?}", self.expects, last.content);
                Ok(response_stream(Response { content: "done".to_string(), tool_calls: Vec::new(), usage: None }))
            }
        }
    }

    #[tokio::test]
    async fn extracts_an_image_block_as_an_attachment_and_keeps_base64_out_of_the_models_context() {
        let tool_result = json!({
            "content": [
                { "type": "text", "text": "here is your image" },
                { "type": "image", "data": "aGVsbG8=", "mimeType": "image/png" }
            ]
        });
        let model = Arc::new(AssertsToolResultTextModel { calls: AtomicUsize::new(0), expects: "[image attached: image/png]" });
        let mut orchestrator = Orchestrator::new(model, temp_vault());
        orchestrator.register_tool(Arc::new(FixedResultTool(tool_result)));

        let result = orchestrator.handle_message(&[], "make an image").await.unwrap();

        assert_eq!(result.content, "done");
        assert_eq!(result.attachments, vec![Attachment { mime_type: "image/png".to_string(), data: "aGVsbG8=".to_string() }]);
    }

    #[tokio::test]
    async fn extracts_a_video_resource_block_the_same_way_as_image_audio() {
        let tool_result = json!({
            "content": [
                { "type": "resource", "resource": { "uri": "file:///clip.mp4", "mimeType": "video/mp4", "blob": "dmlkZW8=" } }
            ]
        });
        let model = Arc::new(AssertsToolResultTextModel { calls: AtomicUsize::new(0), expects: "[resource attached: video/mp4]" });
        let mut orchestrator = Orchestrator::new(model, temp_vault());
        orchestrator.register_tool(Arc::new(FixedResultTool(tool_result)));

        let result = orchestrator.handle_message(&[], "make a video").await.unwrap();

        assert_eq!(result.attachments, vec![Attachment { mime_type: "video/mp4".to_string(), data: "dmlkZW8=".to_string() }]);
    }

    #[tokio::test]
    async fn a_resource_link_is_never_auto_fetched_into_an_attachment() {
        let tool_result = json!({
            "content": [
                { "type": "resource_link", "uri": "https://example.com/report.pdf", "mimeType": "application/pdf" }
            ]
        });
        let model = Arc::new(AssertsToolResultTextModel { calls: AtomicUsize::new(0), expects: "resource_link" });
        let mut orchestrator = Orchestrator::new(model, temp_vault());
        orchestrator.register_tool(Arc::new(FixedResultTool(tool_result)));

        let result = orchestrator.handle_message(&[], "fetch a report").await.unwrap();

        assert!(result.attachments.is_empty());
    }

    #[tokio::test]
    async fn a_non_mcp_shaped_tool_result_falls_back_to_the_old_plain_stringify() {
        // No "content" array at all — the shape every non-MCP tool (generate_document, shell, ...)
        // actually returns.
        let tool_result = json!({ "status": "ok", "path": "/tmp/relatorio.pdf" });
        let model = Arc::new(AssertsToolResultTextModel { calls: AtomicUsize::new(0), expects: "/tmp/relatorio.pdf" });
        let mut orchestrator = Orchestrator::new(model, temp_vault());
        orchestrator.register_tool(Arc::new(FixedResultTool(tool_result)));

        let result = orchestrator.handle_message(&[], "generate a report").await.unwrap();

        assert!(result.attachments.is_empty());
        assert_eq!(result.generated_files, vec!["/tmp/relatorio.pdf".to_string()]);
    }

    #[tokio::test]
    async fn a_status_ok_result_with_no_path_is_not_mistaken_for_a_generated_file() {
        // write_file's exact shape (`{"status":"ok"}`, no `path`) — must not false-positive.
        let tool_result = json!({ "status": "ok" });
        let model = Arc::new(AssertsToolResultTextModel { calls: AtomicUsize::new(0), expects: "ok" });
        let mut orchestrator = Orchestrator::new(model, temp_vault());
        orchestrator.register_tool(Arc::new(FixedResultTool(tool_result)));

        let result = orchestrator.handle_message(&[], "save a note").await.unwrap();

        assert!(result.generated_files.is_empty());
    }

    #[test]
    fn within_media_size_cap_accepts_small_payloads_and_rejects_large_ones() {
        let small = "A".repeat(1_000);
        assert!(within_media_size_cap(&small));

        // Comfortably over MAX_INLINE_MEDIA_BYTES once decoded (~0.75 bytes per base64 char) —
        // not an exact-boundary check, since base64 only encodes in whole groups of 3 bytes/4
        // chars and `within_media_size_cap` itself is a byte-length approximation, not an exact
        // decode.
        let large = "A".repeat((MAX_INLINE_MEDIA_BYTES + 1_000_000) * 4 / 3);
        assert!(!within_media_size_cap(&large));
    }

    // --- P64/P66: oversized media spilled to disk instead of dumped as raw base64 text ---

    /// "AAAA" repeated `groups` times is valid base64 (each group decodes to 3 zero bytes) —
    /// lets a test build an oversized-but-decodable payload without a real video file.
    fn oversized_valid_base64() -> String {
        let groups = MAX_INLINE_MEDIA_BYTES / 3 + 10;
        "AAAA".repeat(groups)
    }

    #[tokio::test]
    async fn an_oversized_video_resource_is_saved_to_disk_and_cited_by_path_not_dumped_as_text() {
        let data = oversized_valid_base64();
        let tool_result = json!({
            "content": [
                { "type": "resource", "resource": { "uri": "file:///clip.mp4", "mimeType": "video/mp4", "blob": data } }
            ]
        });
        let media_root = temp_media_root();
        let model = Arc::new(AssertsToolResultTextModel { calls: AtomicUsize::new(0), expects: "too large to attach inline: video/mp4" });
        let mut orchestrator = Orchestrator::new(model, temp_vault()).with_media_root(media_root.clone());
        orchestrator.register_tool(Arc::new(FixedResultTool(tool_result)));

        let result = orchestrator.handle_message(&[], "make a big video").await.unwrap();

        assert!(result.attachments.is_empty(), "oversized media must never become an inline Attachment");

        let written = std::fs::read_dir(media_root.join("mcp-media"))
            .expect("mcp-media dir should have been created")
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        assert_eq!(written.len(), 1, "expected exactly one spilled file, got {written:?}");
        assert_eq!(written[0].extension().and_then(|e| e.to_str()), Some("mp4"));
        assert!(std::fs::read(&written[0]).unwrap().len() > MAX_INLINE_MEDIA_BYTES);
        assert_eq!(result.generated_files, vec![written[0].display().to_string()]);
    }

    #[tokio::test]
    async fn an_oversized_video_with_no_media_root_configured_degrades_safely() {
        let data = oversized_valid_base64();
        let tool_result = json!({
            "content": [
                { "type": "resource", "resource": { "uri": "file:///clip.mp4", "mimeType": "video/mp4", "blob": data } }
            ]
        });
        let model = Arc::new(AssertsToolResultTextModel {
            calls: AtomicUsize::new(0),
            expects: "too large to attach inline: video/mp4",
        });
        // Plain `Orchestrator::new`, no `with_media_root` — same as any orchestrator not built via
        // `warden-bootstrap::bootstrap()`.
        let mut orchestrator = Orchestrator::new(model, temp_vault());
        orchestrator.register_tool(Arc::new(FixedResultTool(tool_result)));

        let result = orchestrator.handle_message(&[], "make a big video").await.unwrap();

        assert!(result.attachments.is_empty());
        assert!(result.generated_files.is_empty());
    }

    #[tokio::test]
    async fn malformed_oversized_base64_never_gets_dumped_as_raw_text() {
        // Long enough to fail `within_media_size_cap` (same length threshold as the boundary
        // test above), but not valid base64 at all — must not panic, and must not fall back to
        // dumping the raw (huge) block as text.
        let data = "!".repeat((MAX_INLINE_MEDIA_BYTES + 1_000_000) * 4 / 3);
        let tool_result = json!({
            "content": [
                { "type": "image", "data": data, "mimeType": "image/png" }
            ]
        });
        let model = Arc::new(AssertsToolResultTextModel { calls: AtomicUsize::new(0), expects: "malformed data — dropped" });
        let mut orchestrator = Orchestrator::new(model, temp_vault());
        orchestrator.register_tool(Arc::new(FixedResultTool(tool_result)));

        let result = orchestrator.handle_message(&[], "make an image").await.unwrap();

        assert!(result.attachments.is_empty());
        assert!(result.generated_files.is_empty());
    }
}
