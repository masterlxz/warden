//! The code engine (P103 b, P89): a conversation of a code project is driven by an external coding agent working in
//! the project's folder, instead of the Warden's own turn. This module is the thin layer the rest of the Warden talks
//! to — open a session, send a task, receive events, answer permission asks, abort — with the opencode as the first
//! implementation (`opencode`). Another engine (Crush, Codex CLI, Aider, a native one) goes behind `CodeEngine`
//! without touching the memory, the agents or the clients, which stay the Warden's.
//!
//! The opencode formats used here were read from the OpenAPI of `opencode serve` 1.18.34 (`GET /doc`), not from its
//! documentation, which is thinner.

use std::sync::Arc;

use async_trait::async_trait;

use crate::tool::Approver;

pub mod mode;
pub mod opencode;
pub mod process;
mod tracker;

pub use mode::{CodeMode, CodeModes};
pub use tracker::{PermissionAsk, Signal, Tracker};

/// What the person watching a turn sees, as it happens.
#[derive(Debug, Clone, PartialEq)]
pub enum CodeEvent {
    /// More of the answer's text.
    Text(String),
    /// A tool the engine is using, at each stage it passes through.
    Tool(ToolEvent),
    /// Something the engine said about itself that isn't the answer: a retry, a wait.
    Notice(String),
    /// The session the task runs in, as soon as there is one, for whoever may have to stop it (`abort`) before the task
    /// ends. Not for showing.
    Session(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolEvent {
    /// Stable across the stages of one call, so a client updates a line instead of adding one.
    pub call_id: String,
    pub tool: String,
    /// What it is doing, in a line (a command, a file); the tool's name until the engine says more.
    pub title: String,
    pub status: ToolStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolStatus {
    Running,
    Completed,
    Failed,
}

/// One task for the engine.
pub struct TurnRequest {
    /// The folder the engine works in: the project's `workdir`.
    pub workdir: String,
    /// The conversation's session, if it already has one. `None` opens a new one.
    pub session_id: Option<String>,
    pub prompt: String,
    /// What a new session is called (the project's name).
    pub title: String,
    /// Whose actions the person is asked about: the project's name.
    pub target: String,
    /// What the engine is told besides the task, every time: the project's instructions.
    pub system: Option<String>,
    /// How much to ask the person, now and as they change it during the task.
    pub mode: tokio::sync::watch::Receiver<CodeMode>,
}

pub struct TurnOutcome {
    /// To be kept on the conversation and sent back with its next task.
    pub session_id: String,
    /// The engine's answer, as text.
    pub text: String,
    /// The distinct tools it used, in order of first use.
    pub tools_used: Vec<String>,
}

#[async_trait]
pub trait CodeEngine: Send + Sync {
    /// Runs one task to its end, telling `on_event` what happens. Every action the engine wants permission for is put
    /// to `approver`; with none, it is refused (a channel that can't ask must not say yes).
    async fn run_turn(&self, request: TurnRequest, approver: Option<Arc<dyn Approver>>, on_event: &mut (dyn FnMut(CodeEvent) + Send)) -> anyhow::Result<TurnOutcome>;

    /// Stops the task a session is running. Harmless when it isn't running one.
    async fn abort(&self, workdir: &str, session_id: &str) -> anyhow::Result<()>;
}
