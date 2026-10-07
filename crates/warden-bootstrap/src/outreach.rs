//! `message_user` (P121): an agent starts a message to the person, in its own channel, without being asked first.
//!
//! The message is an ordinary assistant message of the agent's channel (`channel_id`): the agent chooses the words and the formatting, and
//! nothing marks it as special. The person sees it in the Agents screen like any other message of that agent, and a client is told through
//! `ConversationsChanged` (the same signal `message_agent` uses) so it can show a notification.
//!
//! The rules, enforced here and not left to the model:
//! 1. Only an agent with an `[[outreach]]` entry in the config gets the tool (`agent_scope::scope_to_agent`). A person switches it on; no
//!    agent grants it to itself or to one it creates, and `manage_agents` cannot write the entry.
//! 2. A rate limit per agent (`MAX_PER_HOUR`), so a monitoring agent that loops cannot flood the person; the tool says when it will be free.
//! 3. The channel and the message are the agent's own: it cannot write in anybody else's.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use warden_core::tool::{Tool, ToolSpec};

use crate::bot_outbox::BotOutbox;
use crate::message_agent::{channel_id, ConversationsChanged};
use crate::{bot_access, bot_pairing};
use crate::{append_to_conversation, message_id, now_millis, ChatRole, ConversationMessage};

/// One agent a person allowed to start messages (TOML `[[outreach]]`). No entry, no tool.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OutreachConfig {
    /// The agent that may message the person first.
    pub agent: String,
    /// Other places the message also goes to, besides the agent's channel (P121): the names of external channels, `telegram` or `whatsapp`.
    /// Read when the message is sent; a name nothing answers to is skipped. It reaches the owner's chats on that bot and only from a run that
    /// is the owner's own (a scheduled task or a webhook), not from a turn a member started.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub forward: Vec<String>,
}

/// The most messages one agent may start in an hour.
pub const MAX_PER_HOUR: usize = 12;
const WINDOW: Duration = Duration::from_secs(3600);
const MAX_MESSAGE_CHARS: usize = 8000;

/// When each agent last started messages, process-wide: the channel may build a fresh tool for every turn.
static SENT: Mutex<Option<HashMap<String, VecDeque<Instant>>>> = Mutex::new(None);

/// Records a message of `agent` at `now` if it is within the limit, or says how long until the oldest one leaves the window.
fn take_slot(agent: &str, now: Instant) -> Result<(), Duration> {
    let mut guard = SENT.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let sent = guard.get_or_insert_with(HashMap::new).entry(agent.to_string()).or_default();
    while sent.front().is_some_and(|at| now.duration_since(*at) >= WINDOW) {
        sent.pop_front();
    }
    if sent.len() >= MAX_PER_HOUR {
        let oldest = *sent.front().expect("a full window has a first");
        return Err(WINDOW.saturating_sub(now.duration_since(oldest)));
    }
    sent.push_back(now);
    Ok(())
}

/// The tool of an agent allowed to start messages. `dir` is where the person's conversations live, the same place `message_agent` writes.
#[derive(Clone)]
pub struct MessageUserTool {
    agent: String,
    dir: PathBuf,
    on_changed: Option<ConversationsChanged>,
    /// Where the message also goes besides the channel: the bots' outbox and the channels the owner listed (`OutreachConfig::forward`).
    forward: Option<(BotOutbox, Vec<String>)>,
}

impl MessageUserTool {
    pub fn new(agent: impl Into<String>, dir: impl Into<PathBuf>) -> Self {
        Self { agent: agent.into(), dir: dir.into(), on_changed: None, forward: None }
    }

    /// Also leaves the message in `outbox` for each of `channels` (`telegram`, `whatsapp`; anything else is skipped). Only for a run
    /// that is the owner's own (a scheduled task, a webhook): the bots write to the owner's chats, so a turn a member started must not
    /// carry the message there.
    pub fn forwarding(mut self, outbox: BotOutbox, channels: Vec<String>) -> Self {
        self.forward = Some((outbox, channels));
        self
    }

    pub fn on_changed(mut self, notify: Option<ConversationsChanged>) -> Self {
        self.on_changed = notify;
        self
    }
}

#[async_trait]
impl Tool for MessageUserTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "message_user".to_string(),
            description: format!(
                "Start a message to the user, in your own channel (the conversation you keep with them), without waiting for them to write \
                 first. Use it for what they asked to be told: an alert, a report, a reminder, a question you need answered. Write it as you \
                 would to a person, in your own voice and formatting; it is an ordinary message from you. Do not use it to answer something they \
                 just said (reply normally), and do not repeat yourself: you may send at most {MAX_PER_HOUR} an hour."
            ),
            parameters: json!({
                "type": "object",
                "properties": {
                    "message": { "type": "string", "description": "What to tell the user. Markdown is fine." }
                },
                "required": ["message"]
            }),
        }
    }

    async fn call(&self, args: Value) -> anyhow::Result<Value> {
        let message = args.get("message").and_then(Value::as_str).unwrap_or_default().trim();
        if message.is_empty() {
            anyhow::bail!("the message is empty");
        }
        if message.chars().count() > MAX_MESSAGE_CHARS {
            anyhow::bail!("the message is over {MAX_MESSAGE_CHARS} characters — shorten it");
        }
        if let Err(wait) = take_slot(&self.agent, Instant::now()) {
            anyhow::bail!("you already sent {MAX_PER_HOUR} messages in the last hour — try again in about {} minutes", wait.as_secs().div_ceil(60).max(1));
        }
        let id = channel_id(&self.agent);
        let saved = ConversationMessage {
            id: message_id(),
            role: ChatRole::Assistant,
            content: message.to_string(),
            created_at: now_millis(),
            usage: None,
            attachments: Vec::new(),
            generated_files: Vec::new(),
            tools_used: Vec::new(),
        };
        // The channel is created by its first message; its title is the agent's name.
        append_to_conversation(&self.dir, &id, &self.agent, Some(&self.agent), None, true, vec![saved])?;
        if let Some(notify) = &self.on_changed {
            notify(&id);
        }
        // The outside channels get it as plain text with the agent's name, since there is no channel to tell them whose it is. A failure there
        // does not undo the message, which is already where the person will find it.
        let mut forwarded = Vec::new();
        if let Some((outbox, channels)) = &self.forward {
            let text = format!("{}: {message}", self.agent);
            for channel in channels.iter().filter(|c| matches!(c.as_str(), bot_pairing::TELEGRAM | bot_pairing::WHATSAPP)) {
                match outbox.push(channel, &text, bot_access::unix_now()) {
                    Ok(()) => forwarded.push(channel.clone()),
                    Err(err) => eprintln!("could not leave the message of agent '{}' for {channel}: {err:#}", self.agent),
                }
            }
        }
        Ok(json!({ "status": "sent", "channel": id, "forwarded": forwarded }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::load_conversation;

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("warden-outreach-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn the_message_lands_in_the_agents_channel_as_an_ordinary_message_and_tells_the_ui() {
        let dir = temp_dir();
        let told = std::sync::Arc::new(Mutex::new(Vec::<String>::new()));
        let sink = told.clone();
        let tool = MessageUserTool::new("outreach-a", &dir).on_changed(Some(std::sync::Arc::new(move |id: &str| sink.lock().unwrap().push(id.to_string()))));

        let reply = tool.call(json!({ "message": "  **Build failed** on main  " })).await.unwrap();
        assert_eq!(reply["status"], "sent");

        let id = channel_id("outreach-a");
        let channel = load_conversation(&dir, &id).unwrap().unwrap();
        assert_eq!(channel.title, "outreach-a");
        assert_eq!(channel.agent_id.as_deref(), Some("outreach-a"));
        assert_eq!(channel.messages.len(), 1);
        assert_eq!(channel.messages[0].role, ChatRole::Assistant);
        assert_eq!(channel.messages[0].content, "**Build failed** on main", "the agent's words, trimmed, nothing added");
        assert_eq!(*told.lock().unwrap(), vec![id.clone()]);

        tool.call(json!({ "message": "second" })).await.unwrap();
        assert_eq!(load_conversation(&dir, &id).unwrap().unwrap().messages.len(), 2, "the same channel, appended");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn a_forwarding_agent_leaves_the_message_for_the_bots_it_lists_and_skips_the_rest() {
        let dir = temp_dir();
        let outbox = BotOutbox::new(dir.join("bot_outbox"));
        let tool = MessageUserTool::new("outreach-d", &dir).forwarding(BotOutbox::new(dir.join("bot_outbox")), vec!["telegram".into(), "carrier-pigeon".into()]);

        let reply = tool.call(json!({ "message": "disk is full" })).await.unwrap();
        assert_eq!(reply["forwarded"], json!(["telegram"]), "an unknown channel is skipped");
        let now = bot_access::unix_now();
        let waiting = outbox.take("telegram", now).unwrap();
        assert_eq!(waiting.len(), 1);
        assert_eq!(waiting[0].text, "outreach-d: disk is full", "the agent's name goes ahead of it, where there is no channel to say whose it is");
        assert!(outbox.take("whatsapp", now).unwrap().is_empty());
        assert!(load_conversation(&dir, &channel_id("outreach-d")).unwrap().is_some(), "and it is in the channel as well");

        let quiet = MessageUserTool::new("outreach-e", &dir);
        assert_eq!(quiet.call(json!({ "message": "hi" })).await.unwrap()["forwarded"], json!([]), "no forwarding unless it was asked for");
        assert!(outbox.take("telegram", now).unwrap().is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn an_empty_or_huge_message_is_refused_before_anything_is_written() {
        let dir = temp_dir();
        let tool = MessageUserTool::new("outreach-b", &dir);
        assert!(tool.call(json!({ "message": "   " })).await.is_err());
        assert!(tool.call(json!({})).await.is_err());
        assert!(tool.call(json!({ "message": "x".repeat(MAX_MESSAGE_CHARS + 1) })).await.unwrap_err().to_string().contains("shorten"));
        assert!(load_conversation(&dir, &channel_id("outreach-b")).unwrap().is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_agent_may_start_a_dozen_an_hour_and_is_told_when_it_can_again() {
        let start = Instant::now();
        for i in 0..MAX_PER_HOUR {
            assert!(take_slot("outreach-c", start + Duration::from_secs(i as u64)).is_ok());
        }
        let wait = take_slot("outreach-c", start + Duration::from_secs(60)).unwrap_err();
        assert!(wait <= WINDOW && wait > Duration::from_secs(3400), "{wait:?}");
        assert!(take_slot("outreach-other", start).is_ok(), "the limit is per agent");
        assert!(take_slot("outreach-c", start + WINDOW + Duration::from_secs(1)).is_ok(), "the window moves on");
    }

    #[test]
    fn the_config_entry_reads_with_or_without_forwarding() {
        let plain: OutreachConfig = toml::from_str("agent = \"chief\"").unwrap();
        assert_eq!(plain, OutreachConfig { agent: "chief".into(), forward: Vec::new() });
        let forwarded: OutreachConfig = toml::from_str("agent = \"chief\"\nforward = [\"telegram\"]").unwrap();
        assert_eq!(forwarded.forward, vec!["telegram".to_string()]);
        assert!(toml::from_str::<OutreachConfig>("agent = \"chief\"\nnope = 1").is_err());
    }
}
