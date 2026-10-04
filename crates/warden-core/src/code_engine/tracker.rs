//! Turns the opencode's event stream (`GET /event`, one JSON object per SSE message) into what the Warden shows. Pure —
//! no I/O — so the whole mapping is tested against transcripts.
//!
//! What the stream gives and what is done with it (event names from the OpenAPI of 1.18.34):
//! - `message.updated` says whether a message is the user's or the assistant's. The user's own prompt also comes back
//!   as a text part, so text is held back until its message is known to be the assistant's, then shown.
//! - `message.part.updated` carries a whole part: the text so far, or a tool call with its state; `message.part.delta`
//!   carries only the new characters of a text part.
//! - `permission.asked` is a question to answer; `session.idle` ends the task; `session.error` fails it;
//!   `session.status` with `retry` is the engine waiting to try again.
//! - A task can start child sessions (the engine's sub-agents), and their permission asks must be answered too, so
//!   `session.created` with a known parent adds the child to the family whose events count.

use std::collections::{HashMap, HashSet};

use serde_json::Value;

use super::{CodeEvent, ToolEvent, ToolStatus};

/// An action the engine wants the person's yes for before it does it.
#[derive(Debug, Clone, PartialEq)]
pub struct PermissionAsk {
    pub id: String,
    /// What kind of action: `bash`, `edit`, `external_directory`…
    pub permission: String,
    /// What it would touch: commands, paths, globs.
    pub patterns: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Signal {
    Event(CodeEvent),
    Ask(PermissionAsk),
    /// The task is over.
    Idle,
    /// The task failed, with the engine's reason.
    Failed(String),
}

struct TextPart {
    message: String,
    text: String,
    /// How much of `text` was already handed out as events.
    emitted: usize,
}

pub struct Tracker {
    /// The task's own session. Its idle or error is the task's end.
    root: String,
    family: HashSet<String>,
    assistant: HashSet<String>,
    /// Text parts by id, and the order they first appeared in (the answer is read in that order).
    texts: HashMap<String, TextPart>,
    order: Vec<String>,
    /// The last (status, title) shown for each tool call, so an unchanged update says nothing.
    tools: HashMap<String, (ToolStatus, String)>,
    tools_used: Vec<String>,
}

impl Tracker {
    pub fn new(session_id: &str) -> Self {
        Self {
            root: session_id.to_string(),
            family: HashSet::from([session_id.to_string()]),
            assistant: HashSet::new(),
            texts: HashMap::new(),
            order: Vec::new(),
            tools: HashMap::new(),
            tools_used: Vec::new(),
        }
    }

    /// The assistant's text so far, its parts apart by a blank line.
    pub fn text(&self) -> String {
        let parts = self.order.iter().filter_map(|id| self.texts.get(id)).filter(|p| self.assistant.contains(&p.message) && !p.text.trim().is_empty());
        parts.map(|p| p.text.trim_end()).collect::<Vec<_>>().join("\n\n")
    }

    pub fn tools_used(&self) -> &[String] {
        &self.tools_used
    }

    /// What one event of the stream means. Events of sessions outside the family, and kinds not listed above, mean
    /// nothing.
    pub fn feed(&mut self, event: &Value) -> Vec<Signal> {
        let props = &event["properties"];
        let mut out = Vec::new();
        match event["type"].as_str().unwrap_or_default() {
            "session.created" => {
                let info = &props["info"];
                if let (Some(id), Some(parent)) = (info["id"].as_str(), info["parentID"].as_str()) {
                    if self.family.contains(parent) {
                        self.family.insert(id.to_string());
                    }
                }
            }
            "message.updated" => {
                let info = &props["info"];
                if self.in_family(info["sessionID"].as_str()) && info["role"].as_str() == Some("assistant") {
                    if let Some(id) = info["id"].as_str() {
                        if self.assistant.insert(id.to_string()) {
                            // Its parts may have arrived first; they are shown now.
                            let held: Vec<String> = self.order.iter().filter(|p| self.texts[*p].message == id).cloned().collect();
                            out.extend(held.iter().filter_map(|p| self.emit(p)));
                        }
                    }
                }
            }
            "message.part.updated" => {
                let part = &props["part"];
                if self.in_family(part["sessionID"].as_str()) {
                    match part["type"].as_str() {
                        Some("text") => {
                            if let (Some(id), Some(message)) = (part["id"].as_str(), part["messageID"].as_str()) {
                                let text = part["text"].as_str().unwrap_or_default();
                                let entry = self.texts.entry(id.to_string()).or_insert_with(|| TextPart { message: message.to_string(), text: String::new(), emitted: 0 });
                                if !self.order.iter().any(|p| p == id) {
                                    self.order.push(id.to_string());
                                }
                                entry.text = text.to_string();
                                out.extend(self.emit(id));
                            }
                        }
                        Some("tool") => out.extend(self.tool(part)),
                        _ => {}
                    }
                }
            }
            "message.part.delta" => {
                if self.in_family(props["sessionID"].as_str()) && props["field"].as_str() == Some("text") {
                    if let (Some(id), Some(delta)) = (props["partID"].as_str(), props["delta"].as_str()) {
                        // A delta for a part never seen as text is a reasoning part's, or one that isn't known yet: not the answer.
                        if let Some(part) = self.texts.get_mut(id) {
                            part.text.push_str(delta);
                            out.extend(self.emit(id));
                        }
                    }
                }
            }
            "permission.asked" => {
                if self.in_family(props["sessionID"].as_str()) {
                    if let (Some(id), Some(permission)) = (props["id"].as_str(), props["permission"].as_str()) {
                        let patterns = props["patterns"].as_array().map(|a| a.iter().filter_map(|p| p.as_str().map(str::to_string)).collect()).unwrap_or_default();
                        out.push(Signal::Ask(PermissionAsk { id: id.to_string(), permission: permission.to_string(), patterns }));
                    }
                }
            }
            "session.status" => {
                if self.in_family(props["sessionID"].as_str()) {
                    let status = &props["status"];
                    if status["type"].as_str() == Some("retry") {
                        let message = status["message"].as_str().unwrap_or("trying again");
                        out.push(Signal::Event(CodeEvent::Notice(format!("{message} (attempt {})", status["attempt"].as_u64().unwrap_or(0)))));
                    }
                }
            }
            // Only the task's own session ending ends the task; a child going idle is a sub-agent finishing.
            "session.idle" => {
                if props["sessionID"].as_str().is_some_and(|s| self.is_root(s)) {
                    out.push(Signal::Idle);
                }
            }
            "session.error" if props["sessionID"].as_str().is_some_and(|s| self.is_root(s)) => {
                let error = &props["error"];
                let reason = error["data"]["message"].as_str().or_else(|| error["name"].as_str()).unwrap_or("the engine failed");
                out.push(Signal::Failed(reason.to_string()));
            }
            _ => {}
        }
        out
    }

    fn in_family(&self, session: Option<&str>) -> bool {
        session.is_some_and(|s| self.family.contains(s))
    }

    fn is_root(&self, session: &str) -> bool {
        self.root == session
    }

    /// The part's new characters as an event, if its message is the assistant's.
    fn emit(&mut self, id: &str) -> Option<Signal> {
        let part = self.texts.get_mut(id)?;
        if !self.assistant.contains(&part.message) {
            return None;
        }
        let fresh = part.text.get(part.emitted..).filter(|s| !s.is_empty()).map(str::to_string);
        part.emitted = part.text.len();
        fresh.map(|text| Signal::Event(CodeEvent::Text(text)))
    }

    fn tool(&mut self, part: &Value) -> Option<Signal> {
        let (call, tool) = (part["callID"].as_str()?, part["tool"].as_str()?);
        let state = &part["state"];
        let status = match state["status"].as_str()? {
            "pending" | "running" => ToolStatus::Running,
            "completed" => ToolStatus::Completed,
            "error" => ToolStatus::Failed,
            _ => return None,
        };
        let title = state["title"].as_str().filter(|t| !t.trim().is_empty()).unwrap_or(tool).to_string();
        if self.tools.get(call) == Some(&(status, title.clone())) {
            return None;
        }
        self.tools.insert(call.to_string(), (status, title.clone()));
        if !self.tools_used.iter().any(|t| t == tool) {
            self.tools_used.push(tool.to_string());
        }
        Some(Signal::Event(CodeEvent::Tool(ToolEvent { call_id: call.to_string(), tool: tool.to_string(), title, status })))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const S: &str = "ses_root";

    fn msg(id: &str, role: &str) -> Value {
        json!({"type": "message.updated", "properties": {"info": {"id": id, "role": role, "sessionID": S}}})
    }
    fn text(part: &str, message: &str, text: &str) -> Value {
        json!({"type": "message.part.updated", "properties": {"sessionID": S, "part": {"id": part, "messageID": message, "sessionID": S, "type": "text", "text": text}}})
    }
    fn delta(part: &str, message: &str, delta: &str) -> Value {
        json!({"type": "message.part.delta", "properties": {"sessionID": S, "messageID": message, "partID": part, "field": "text", "delta": delta}})
    }
    fn tool(call: &str, name: &str, status: &str, title: &str) -> Value {
        let mut state = json!({"status": status, "input": {}});
        if !title.is_empty() {
            state["title"] = json!(title);
        }
        json!({"type": "message.part.updated", "properties": {"sessionID": S, "part": {"id": format!("prt_{call}"), "messageID": "msg_a", "sessionID": S, "type": "tool", "callID": call, "tool": name, "state": state}}})
    }
    fn texts(signals: &[Signal]) -> Vec<&str> {
        signals.iter().filter_map(|s| if let Signal::Event(CodeEvent::Text(t)) = s { Some(t.as_str()) } else { None }).collect()
    }

    #[test]
    fn the_users_own_prompt_is_never_shown_back_as_the_answer() {
        let mut t = Tracker::new(S);
        assert!(t.feed(&msg("msg_u", "user")).is_empty());
        assert!(t.feed(&text("prt_u", "msg_u", "fix the bug")).is_empty());
        assert_eq!(t.text(), "");
    }

    #[test]
    fn text_is_shown_as_it_grows_and_read_back_whole() {
        let mut t = Tracker::new(S);
        t.feed(&msg("msg_a", "assistant"));
        assert_eq!(texts(&t.feed(&text("prt_1", "msg_a", "Looking"))), ["Looking"]);
        assert_eq!(texts(&t.feed(&delta("prt_1", "msg_a", " at it"))), [" at it"]);
        // The whole part arriving again with more is only the new characters.
        assert_eq!(texts(&t.feed(&text("prt_1", "msg_a", "Looking at it now"))), [" now"]);
        assert!(t.feed(&text("prt_1", "msg_a", "Looking at it now")).is_empty(), "nothing new, nothing said");
        t.feed(&text("prt_2", "msg_a", "Done."));
        assert_eq!(t.text(), "Looking at it now\n\nDone.");
    }

    #[test]
    fn parts_that_arrive_before_their_message_is_known_wait_for_it() {
        let mut t = Tracker::new(S);
        assert!(t.feed(&text("prt_1", "msg_a", "early")).is_empty());
        assert_eq!(texts(&t.feed(&msg("msg_a", "assistant"))), ["early"]);
        assert_eq!(t.text(), "early");
    }

    #[test]
    fn a_delta_for_a_part_that_is_not_text_is_not_the_answer() {
        let mut t = Tracker::new(S);
        t.feed(&msg("msg_a", "assistant"));
        assert!(t.feed(&delta("prt_reasoning", "msg_a", "thinking…")).is_empty());
        assert_eq!(t.text(), "");
    }

    #[test]
    fn a_tool_call_is_one_line_that_changes_and_repeats_say_nothing() {
        let mut t = Tracker::new(S);
        let status = |signals: Vec<Signal>| -> Vec<(ToolStatus, String)> {
            signals.into_iter().filter_map(|s| if let Signal::Event(CodeEvent::Tool(e)) = s { Some((e.status, e.title)) } else { None }).collect()
        };
        assert_eq!(status(t.feed(&tool("c1", "bash", "pending", ""))), [(ToolStatus::Running, "bash".into())]);
        assert!(t.feed(&tool("c1", "bash", "pending", "")).is_empty());
        assert_eq!(status(t.feed(&tool("c1", "bash", "running", "cargo test"))), [(ToolStatus::Running, "cargo test".into())]);
        assert_eq!(status(t.feed(&tool("c1", "bash", "completed", "cargo test"))), [(ToolStatus::Completed, "cargo test".into())]);
        assert_eq!(status(t.feed(&tool("c2", "edit", "error", ""))), [(ToolStatus::Failed, "edit".into())]);
        assert_eq!(t.tools_used(), ["bash", "edit"]);
    }

    #[test]
    fn a_permission_ask_of_the_task_or_of_its_children_is_put_to_the_person_and_a_strangers_is_not() {
        let mut t = Tracker::new(S);
        let ask = |session: &str, id: &str| json!({"type": "permission.asked", "properties": {"id": id, "sessionID": session, "permission": "bash", "patterns": ["rm -rf build"], "metadata": {}, "always": []}});
        assert_eq!(t.feed(&ask(S, "per_1")), [Signal::Ask(PermissionAsk { id: "per_1".into(), permission: "bash".into(), patterns: vec!["rm -rf build".into()] })]);
        assert!(t.feed(&ask("ses_child", "per_2")).is_empty(), "not ours yet");
        t.feed(&json!({"type": "session.created", "properties": {"info": {"id": "ses_child", "parentID": S}}}));
        assert_eq!(t.feed(&ask("ses_child", "per_3")).len(), 1, "a sub-agent's ask is ours");
        t.feed(&json!({"type": "session.created", "properties": {"info": {"id": "ses_other", "parentID": "ses_elsewhere"}}}));
        assert!(t.feed(&ask("ses_other", "per_4")).is_empty());
    }

    #[test]
    fn only_the_tasks_own_session_ends_it() {
        let mut t = Tracker::new(S);
        t.feed(&json!({"type": "session.created", "properties": {"info": {"id": "ses_child", "parentID": S}}}));
        assert!(t.feed(&json!({"type": "session.idle", "properties": {"sessionID": "ses_child"}})).is_empty(), "a sub-agent finishing is not the end");
        assert_eq!(t.feed(&json!({"type": "session.idle", "properties": {"sessionID": S}})), [Signal::Idle]);
        let failed = t.feed(&json!({"type": "session.error", "properties": {"sessionID": S, "error": {"name": "APIError", "data": {"message": "no credits"}}}}));
        assert_eq!(failed, [Signal::Failed("no credits".into())]);
        let unnamed = t.feed(&json!({"type": "session.error", "properties": {"sessionID": S, "error": {"name": "UnknownError"}}}));
        assert_eq!(unnamed, [Signal::Failed("UnknownError".into())]);
    }

    #[test]
    fn a_retry_is_a_notice_and_unknown_events_are_ignored() {
        let mut t = Tracker::new(S);
        let retry = t.feed(&json!({"type": "session.status", "properties": {"sessionID": S, "status": {"type": "retry", "attempt": 2, "message": "rate limited"}}}));
        assert_eq!(retry, [Signal::Event(CodeEvent::Notice("rate limited (attempt 2)".into()))]);
        assert!(t.feed(&json!({"type": "session.status", "properties": {"sessionID": S, "status": {"type": "idle"}}})).is_empty());
        assert!(t.feed(&json!({"type": "server.connected", "properties": {}})).is_empty());
        assert!(t.feed(&json!({"type": "file.watcher.updated", "properties": {}})).is_empty());
        assert!(t.feed(&json!("not even an object")).is_empty());
    }
}
