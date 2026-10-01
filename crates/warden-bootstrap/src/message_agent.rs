//! `message_agent` (P46, "funcionários" mode): one agent leaves a message for another, with no chief
//! in between. The message lands in a conversation between the two ("A → B") in the same place the
//! channel keeps its conversations, so the person sees it in their list and can open it and talk to
//! B directly. B answers there in the background, as a normal turn of its own; A can wait for the
//! answer or come back for it with `read`.
//!
//! The rules, enforced here and not left to the model:
//! 1. Only an agent with `AgentConfig.can_message_agents` gets the tool (the channels attach it per
//!    turn, `agent_scope::scope_to_agent`), and `manage_agents` never turns that on.
//! 2. B runs from the channel's base orchestrator, like a `delegate_to_agent` target: its own persona,
//!    model, skills and tool list — and never `message_agent`, `delegate_to_agent`, `manage_agents` or
//!    an approver, since those are attached per turn and B's turn gets none. That is what stops two
//!    agents from messaging each other back and forth forever.
//! 3. One message in flight per conversation: while B is still answering A, another message from A
//!    to B is refused. At most one background turn per pair of agents, whatever the model does.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use warden_core::orchestrator::Orchestrator;
use warden_core::tool::{Tool, ToolSpec};

use crate::{
    append_to_conversation, assistant_message, delegate_targets, load_config_from_path, load_conversation, message_id, now_millis, to_message,
    ChatRole, ConversationMessage, FileConfig,
};

/// Called with a conversation id whenever `message_agent` creates or updates that conversation, so a
/// channel can tell its UI to reload the list (a Tauri event, a `ConversationsChanged` on the hub).
pub type ConversationsChanged = Arc<dyn Fn(&str) + Send + Sync>;

/// How long `send` with `wait: true` waits before handing back and letting B finish on its own.
const WAIT_TIMEOUT: Duration = Duration::from_secs(180);
const MAX_MESSAGE_CHARS: usize = 8000;

/// Conversations with a message still being answered, by file path — process-wide, since the
/// channel may build a fresh tool for every turn.
static IN_FLIGHT: Mutex<Option<HashSet<PathBuf>>> = Mutex::new(None);

fn in_flight() -> std::sync::MutexGuard<'static, Option<HashSet<PathBuf>>> {
    IN_FLIGHT.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn is_busy(key: &Path) -> bool {
    in_flight().as_ref().is_some_and(|set| set.contains(key))
}

/// How many notes this process is still answering. The CLI waits for them before exiting (P87):
/// the answer runs in this process and would die with it, leaving the note unanswered.
pub fn answers_in_flight() -> usize {
    in_flight().as_ref().map_or(0, HashSet::len)
}

/// Marks a conversation busy until dropped — so a failed or panicking turn frees it too.
struct InFlight(PathBuf);

impl InFlight {
    fn claim(key: PathBuf) -> Option<Self> {
        in_flight().get_or_insert_with(HashSet::new).insert(key.clone()).then(|| Self(key))
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        if let Some(set) = in_flight().as_mut() {
            set.remove(&self.0);
        }
    }
}

/// The id of the conversation between `from` and `to` (in that direction). Agent ids are free text,
/// so the id is a stable hash rather than the names themselves: always a safe file name, always the
/// same for the same pair. The names are in the conversation's title.
pub fn thread_id(from: &str, to: &str) -> String {
    // FNV-1a, 64 bits: stable across builds, unlike `std`'s hasher.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in from.bytes().chain([0]).chain(to.bytes()) {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("agents-{hash:016x}")
}

pub fn thread_title(from: &str, to: &str) -> String {
    format!("{from} → {to}")
}

#[derive(Clone)]
pub struct MessageAgentTool {
    caller: String,
    config_path: PathBuf,
    conversations_dir: PathBuf,
    /// The channel's orchestrator before any agent scoping: B's turn is built from it.
    base: Orchestrator,
    on_changed: Option<ConversationsChanged>,
    wait_timeout: Duration,
}

impl MessageAgentTool {
    pub fn new(caller: impl Into<String>, config_path: impl Into<PathBuf>, conversations_dir: impl Into<PathBuf>, base: Orchestrator) -> Self {
        Self {
            caller: caller.into(),
            config_path: config_path.into(),
            conversations_dir: conversations_dir.into(),
            base,
            on_changed: None,
            wait_timeout: WAIT_TIMEOUT,
        }
    }

    pub fn on_changed(mut self, notify: Option<ConversationsChanged>) -> Self {
        self.on_changed = notify;
        self
    }

    #[cfg(test)]
    fn with_wait_timeout(mut self, timeout: Duration) -> Self {
        self.wait_timeout = timeout;
        self
    }

    fn load(&self) -> anyhow::Result<FileConfig> {
        load_config_from_path(&self.config_path, true)
    }

    fn notify(&self, conversation_id: &str) {
        if let Some(notify) = &self.on_changed {
            notify(conversation_id);
        }
    }

    fn thread_path(&self, conversation_id: &str) -> PathBuf {
        self.conversations_dir.join(format!("{conversation_id}.json"))
    }

    /// The agents this one can message: everyone configured but itself.
    fn recipients(config: &FileConfig, caller: &str) -> Vec<(String, String)> {
        config.agents.iter().filter(|a| a.id != caller && a.owner.is_none()).map(|a| (a.id.clone(), a.persona.clone())).collect()
    }

    fn check_recipient(&self, config: &FileConfig, to: &str) -> anyhow::Result<()> {
        if to == self.caller {
            anyhow::bail!("you can't leave a message for yourself — pick another agent");
        }
        if !config.agents.iter().any(|a| a.id == to && a.owner.is_none()) {
            let known: Vec<String> = Self::recipients(config, &self.caller).into_iter().map(|(id, _)| id).collect();
            anyhow::bail!("no agent named '{to}' — the agents you can message are: {}", known.join(", "));
        }
        Ok(())
    }

    async fn send(&self, to: &str, message: &str, wait: bool) -> anyhow::Result<Value> {
        let message = message.trim();
        if message.is_empty() {
            anyhow::bail!("the message is empty");
        }
        if message.chars().count() > MAX_MESSAGE_CHARS {
            anyhow::bail!("the message is too long (max {MAX_MESSAGE_CHARS} characters)");
        }
        let config = self.load()?;
        self.check_recipient(&config, to)?;
        let Some(target) = delegate_targets(&config, &self.base).into_iter().find(|t| t.id == to) else {
            anyhow::bail!("agent '{to}' can't be reached right now (its model provider is missing or invalid)");
        };

        let conversation_id = thread_id(&self.caller, to);
        let Some(claim) = InFlight::claim(self.thread_path(&conversation_id)) else {
            anyhow::bail!("'{to}' is still answering your previous message — use action 'read' to check on it");
        };

        let dir = self.conversations_dir.clone();
        let history: Vec<_> = load_conversation(&dir, &conversation_id)?.iter().flat_map(|c| &c.messages).map(to_message).collect();
        let content = format!("Message from {}:\n\n{message}", self.caller);
        let user = ConversationMessage {
            id: message_id(),
            role: ChatRole::User,
            content: content.clone(),
            created_at: now_millis(),
            usage: None,
            attachments: Vec::new(),
            generated_files: Vec::new(), tools_used: Vec::new(),
        };
        // Saved before B starts, so the person sees the message while B works on it.
        append_to_conversation(&dir, &conversation_id, &thread_title(&self.caller, to), Some(to), true, vec![user])?;
        self.notify(&conversation_id);

        let (done_tx, done_rx) = tokio::sync::oneshot::channel::<Result<String, String>>();
        let this = self.clone();
        let to_owned = to.to_string();
        let thread = conversation_id.clone();
        tokio::spawn(async move {
            let _claim = claim;
            let outcome = target.orchestrator.handle_turn(&history, &content, Vec::new(), target.persona.as_deref()).await;
            let reply = match outcome {
                Ok(outcome) => {
                    let saved = append_to_conversation(&dir, &thread, "", Some(&to_owned), false, vec![assistant_message(&outcome)]);
                    match saved {
                        Ok(_) => Ok(outcome.content),
                        Err(err) => Err(format!("{err:#}")),
                    }
                }
                Err(err) => {
                    // The error goes in the conversation too, so whoever opens it sees why B stopped.
                    let note = ConversationMessage {
                        id: message_id(),
                        role: ChatRole::Assistant,
                        content: format!("(could not answer: {err:#})"),
                        created_at: now_millis(),
                        usage: None,
                        attachments: Vec::new(),
                        generated_files: Vec::new(), tools_used: Vec::new(),
                    };
                    let _ = append_to_conversation(&dir, &thread, "", Some(&to_owned), false, vec![note]);
                    Err(format!("{err:#}"))
                }
            };
            this.notify(&thread);
            let _ = done_tx.send(reply);
        });

        if !wait {
            return Ok(json!({
                "status": "sent",
                "to": to,
                "note": format!("'{to}' is answering in the background, in the conversation '{}'. Use action 'read' later to get the answer.", thread_title(&self.caller, to)),
            }));
        }
        match tokio::time::timeout(self.wait_timeout, done_rx).await {
            Ok(Ok(Ok(answer))) => Ok(json!({ "status": "answered", "to": to, "answer": answer })),
            Ok(Ok(Err(err))) => anyhow::bail!("'{to}' could not answer: {err}"),
            Ok(Err(_)) => anyhow::bail!("'{to}' stopped before answering"),
            Err(_) => Ok(json!({
                "status": "still_answering",
                "to": to,
                "note": format!("'{to}' is taking a while; it keeps working in the background. Use action 'read' later."),
            })),
        }
    }

    fn read(&self, to: &str) -> anyhow::Result<Value> {
        let config = self.load()?;
        self.check_recipient(&config, to)?;
        let conversation_id = thread_id(&self.caller, to);
        let busy = is_busy(&self.thread_path(&conversation_id));
        let Some(conversation) = load_conversation(&self.conversations_dir, &conversation_id)? else {
            return Ok(json!({ "to": to, "status": "no_messages", "note": format!("you haven't messaged '{to}' yet (or the person deleted that conversation)") }));
        };
        // Everything after the last message sent to B — its answer, or answers if the person also
        // talked to B in that conversation since.
        let last_user = conversation.messages.iter().rposition(|m| m.role == ChatRole::User);
        let replies: Vec<&str> = conversation.messages[last_user.map_or(0, |i| i + 1)..]
            .iter()
            .filter(|m| m.role == ChatRole::Assistant)
            .map(|m| m.content.as_str())
            .collect();
        let status = if busy {
            "still_answering"
        } else if replies.is_empty() {
            "no_answer"
        } else {
            "answered"
        };
        Ok(json!({ "to": to, "status": status, "answers": replies }))
    }
}

#[async_trait]
impl Tool for MessageAgentTool {
    fn spec(&self) -> ToolSpec {
        // Read on every spec so an agent created mid-turn shows up; an unreadable file lists nobody.
        let recipients = self.load().map(|c| Self::recipients(&c, &self.caller)).unwrap_or_default();
        let listing = recipients
            .iter()
            .map(|(id, persona)| format!("- {id}: {}", persona.chars().take(200).collect::<String>()))
            .collect::<Vec<_>>()
            .join("\n");
        ToolSpec {
            name: "message_agent".to_string(),
            description: format!(
                "Leave a message for another of the user's agents — a colleague, not a subordinate. The message goes \
                 into a conversation between you and that agent, which the user can see and join; the agent answers \
                 there in the background, as itself (its own persona, model and tools), remembering your earlier \
                 messages to it. Use 'send' (with wait: true to get the answer now) and 'read' to collect an answer \
                 later. Only one message at a time per agent. Agents:\n{listing}"
            ),
            parameters: json!({
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["send", "read"] },
                    "agent_id": {
                        "type": "string",
                        "enum": recipients.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>(),
                        "description": "Which agent to message or read from."
                    },
                    "message": {
                        "type": "string",
                        "description": "For 'send': what you want to tell or ask. It knows your earlier messages to it, \
                                        but not this conversation — include what it needs."
                    },
                    "wait": {
                        "type": "boolean",
                        "description": "For 'send': wait for the answer (up to a few minutes) instead of returning at once."
                    }
                },
                "required": ["action", "agent_id"]
            }),
        }
    }

    async fn call(&self, args: Value) -> anyhow::Result<Value> {
        let action = args.get("action").and_then(Value::as_str).ok_or_else(|| anyhow::anyhow!("missing required 'action' argument"))?;
        let to = args.get("agent_id").and_then(Value::as_str).ok_or_else(|| anyhow::anyhow!("missing required 'agent_id' argument"))?;
        match action {
            "send" => {
                let message = args.get("message").and_then(Value::as_str).ok_or_else(|| anyhow::anyhow!("missing required 'message' argument"))?;
                let wait = args.get("wait").and_then(Value::as_bool).unwrap_or(false);
                self.send(to, message, wait).await
            }
            "read" => self.read(to),
            other => anyhow::bail!("unknown action '{other}' — use 'send' or 'read'"),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use warden_core::memory::Vault;
    use warden_core::model::{response_stream, ChatStream, Message, ModelProvider, Response, Role};

    use super::*;
    use crate::{save_config, scope_to_agent, AgentConfig, AgentExtras};

    /// Answers "<persona>: <message>", after `delay`; `fail` makes every call an error.
    struct Echo {
        delay: Duration,
        fail: bool,
        calls: AtomicUsize,
    }

    #[async_trait]
    impl ModelProvider for Echo {
        async fn chat_stream(&self, messages: Vec<Message>, _tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(self.delay).await;
            if self.fail {
                anyhow::bail!("provider down");
            }
            let persona = messages.iter().find(|m| m.role == Role::System).map(|m| m.content.clone()).unwrap_or_default();
            let last = messages.last().unwrap().content.clone();
            Ok(response_stream(Response { content: format!("{persona}: {last}"), tool_calls: Vec::new(), usage: None }))
        }
    }

    fn agent(id: &str, message: bool) -> AgentConfig {
        AgentConfig {
            id: id.into(),
            persona: format!("I am {id}"),
            provider_id: None,
            can_delegate_to_agents: false,
            can_manage_agents: false,
            can_message_agents: message,
            can_manage_tasks: false,
            allowed_tools: None,
            owner: None,
            shared_with: Vec::new(),
        }
    }

    struct Setup {
        dir: PathBuf,
        config_path: PathBuf,
        conversations: PathBuf,
        base: Orchestrator,
        model: Arc<Echo>,
    }

    fn setup(delay: Duration, fail: bool) -> Setup {
        let dir = std::env::temp_dir().join(format!(
            "warden-message-agent-test-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let config_path = dir.join("config.toml");
        save_config(&config_path, &FileConfig { agents: vec![agent("ana", true), agent("bia", false)], ..FileConfig::default() }).unwrap();
        let model = Arc::new(Echo { delay, fail, calls: AtomicUsize::new(0) });
        let base = Orchestrator::new(model.clone(), Arc::new(Vault::new(dir.join("vault"))));
        Setup { conversations: dir.join("conversations"), dir, config_path, base, model }
    }

    impl Setup {
        fn tool(&self) -> MessageAgentTool {
            MessageAgentTool::new("ana", &self.config_path, &self.conversations, self.base.clone())
        }

        fn thread(&self) -> crate::Conversation {
            load_conversation(&self.conversations, &thread_id("ana", "bia")).unwrap().expect("the conversation exists")
        }
    }

    impl Drop for Setup {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.dir).ok();
        }
    }

    #[tokio::test]
    async fn send_and_wait_saves_the_message_and_the_answer_in_the_pairs_conversation() {
        let s = setup(Duration::ZERO, false);
        let result = s.tool().call(json!({ "action": "send", "agent_id": "bia", "message": "hello", "wait": true })).await.unwrap();
        assert_eq!(result["status"], "answered");
        assert_eq!(result["answer"], "I am bia: Message from ana:\n\nhello");

        let thread = s.thread();
        assert_eq!(thread.title, "ana → bia");
        assert_eq!(thread.agent_id.as_deref(), Some("bia"));
        let roles: Vec<_> = thread.messages.iter().map(|m| m.role).collect();
        assert_eq!(roles, vec![ChatRole::User, ChatRole::Assistant]);

        // A second message continues the same conversation, with the first one as history.
        s.tool().call(json!({ "action": "send", "agent_id": "bia", "message": "again", "wait": true })).await.unwrap();
        assert_eq!(s.thread().messages.len(), 4);
    }

    #[tokio::test]
    async fn without_wait_it_returns_at_once_and_read_collects_the_answer_later() {
        let s = setup(Duration::from_millis(300), false);
        let notified = Arc::new(Mutex::new(Vec::<String>::new()));
        let seen = notified.clone();
        let tool = s.tool().on_changed(Some(Arc::new(move |id: &str| seen.lock().unwrap().push(id.to_string()))));

        let sent = tool.call(json!({ "action": "send", "agent_id": "bia", "message": "hello" })).await.unwrap();
        assert_eq!(sent["status"], "sent");
        // The message is already there for the person to see, and only one message at a time.
        assert_eq!(s.thread().messages.len(), 1);
        assert_eq!(tool.call(json!({ "action": "read", "agent_id": "bia" })).await.unwrap()["status"], "still_answering");
        // Other tests run in parallel, so only "at least this one" can be checked.
        assert!(answers_in_flight() >= 1);
        let busy = tool.call(json!({ "action": "send", "agent_id": "bia", "message": "more" })).await.unwrap_err();
        assert!(busy.to_string().contains("still answering"), "{busy:#}");

        eventually(|| async { tool.call(json!({ "action": "read", "agent_id": "bia" })).await.unwrap()["status"] == "answered" }).await;
        let read = tool.call(json!({ "action": "read", "agent_id": "bia" })).await.unwrap();
        assert_eq!(read["status"], "answered");
        assert_eq!(read["answers"], json!(["I am bia: Message from ana:\n\nhello"]));
        let thread = thread_id("ana", "bia");
        assert_eq!(*notified.lock().unwrap(), vec![thread.clone(), thread]);
    }

    #[tokio::test]
    async fn a_wait_that_runs_out_leaves_the_colleague_working() {
        let s = setup(Duration::from_millis(300), false);
        let tool = s.tool().with_wait_timeout(Duration::from_millis(20));
        let result = tool.call(json!({ "action": "send", "agent_id": "bia", "message": "hello", "wait": true })).await.unwrap();
        assert_eq!(result["status"], "still_answering");
        eventually(|| async { s.thread().messages.len() == 2 }).await;
    }

    /// Polls `done` until it holds, failing after a deadline far above what a turn takes even on a
    /// busy machine (every turn runs the vault's semantic search, which loads a model).
    async fn eventually<F, Fut>(mut done: F)
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = bool>,
    {
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while !done().await {
            assert!(std::time::Instant::now() < deadline, "condition not met within 30s");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    #[tokio::test]
    async fn bad_requests_are_refused_before_any_model_call() {
        let s = setup(Duration::ZERO, false);
        let tool = s.tool();
        for (args, expected) in [
            (json!({ "action": "send", "agent_id": "ana", "message": "hi" }), "yourself"),
            (json!({ "action": "send", "agent_id": "ghost", "message": "hi" }), "no agent named 'ghost'"),
            (json!({ "action": "send", "agent_id": "bia", "message": "   " }), "empty"),
            (json!({ "action": "send", "agent_id": "bia", "message": "x".repeat(MAX_MESSAGE_CHARS + 1) }), "too long"),
            (json!({ "action": "shout", "agent_id": "bia" }), "unknown action"),
        ] {
            let err = tool.call(args).await.unwrap_err();
            assert!(err.to_string().contains(expected), "expected '{expected}', got {err:#}");
        }
        assert_eq!(s.model.calls.load(Ordering::SeqCst), 0);
        assert_eq!(tool.call(json!({ "action": "read", "agent_id": "bia" })).await.unwrap()["status"], "no_messages");
        // The spec never offers the caller itself.
        assert_eq!(tool.spec().parameters["properties"]["agent_id"]["enum"], json!(["bia"]));
    }

    #[tokio::test]
    async fn a_failed_answer_is_reported_and_written_in_the_conversation() {
        let s = setup(Duration::ZERO, true);
        let err = s.tool().call(json!({ "action": "send", "agent_id": "bia", "message": "hello", "wait": true })).await.unwrap_err();
        assert!(err.to_string().contains("provider down"), "{err:#}");
        let thread = s.thread();
        assert!(thread.messages[1].content.contains("could not answer"));
        // The conversation is free again.
        let again = s.tool().call(json!({ "action": "send", "agent_id": "bia", "message": "retry", "wait": true })).await;
        assert!(again.unwrap_err().to_string().contains("provider down"));
    }

    #[test]
    fn thread_ids_are_safe_and_stable_per_direction() {
        let id = thread_id("ana", "bia");
        assert_eq!(id, thread_id("ana", "bia"));
        assert_ne!(id, thread_id("bia", "ana"));
        assert_ne!(thread_id("a", "b c"), thread_id("a b", "c"));
        let weird = thread_id("../x", "y/z");
        assert!(weird.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'));
    }

    #[test]
    fn only_an_agent_with_the_flag_and_a_conversations_dir_gets_the_tool() {
        let s = setup(Duration::ZERO, false);
        let config = load_config_from_path(&s.config_path, true).unwrap();
        let names = |agent_id: &str, dir: Option<PathBuf>| -> Vec<String> {
            let extras = AgentExtras { conversations_dir: dir, on_conversation_changed: None };
            let scoped = scope_to_agent(&s.base, &config, Some(&s.config_path), agent_id, extras).unwrap();
            scoped.orchestrator.tools().iter().map(|t| t.spec().name).collect()
        };
        assert!(names("ana", Some(s.conversations.clone())).contains(&"message_agent".to_string()));
        assert!(!names("ana", None).contains(&"message_agent".to_string()));
        assert!(!names("bia", Some(s.conversations.clone())).contains(&"message_agent".to_string()));
        assert!(scope_to_agent(&s.base, &config, Some(&s.config_path), "ghost", AgentExtras::default()).is_none());
    }
}
