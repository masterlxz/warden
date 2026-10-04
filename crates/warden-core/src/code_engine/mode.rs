//! How much a code conversation asks before the engine acts (P103 b), chosen by the person and changeable at any moment,
//! in the middle of a task included: that is why it travels as a `watch` the engine looks at on each ask, not as a
//! value fixed when the task starts.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tokio::sync::watch;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CodeMode {
    /// Every ask goes to the person (what they answer "always" to is remembered).
    #[default]
    Manual,
    /// Changing files is a yes without asking; the rest asks.
    AcceptEdits,
    /// Everything is a yes. The shell is no sandbox, so this is the one to be careful with.
    AcceptAll,
    /// Nothing may change: every ask is a no, and the engine is told to answer with a plan.
    Plan,
}

impl CodeMode {
    /// A name a client sent; anything unknown is `Manual`, the one that asks.
    pub fn parse(name: &str) -> Self {
        match name {
            "acceptEdits" => Self::AcceptEdits,
            "acceptAll" => Self::AcceptAll,
            "plan" => Self::Plan,
            _ => Self::Manual,
        }
    }

    /// What the engine does with an ask of this kind of action without putting it to the person: `Some(true)` is a yes,
    /// `Some(false)` a no, `None` means ask.
    pub fn decides(self, permission: &str) -> Option<bool> {
        match self {
            Self::Manual => None,
            Self::AcceptAll => Some(true),
            Self::AcceptEdits => (permission == "edit").then_some(true),
            Self::Plan => Some(false),
        }
    }

    /// What the engine is told besides the task in this mode.
    pub fn instructions(self) -> Option<&'static str> {
        match self {
            Self::Plan => Some("Plan mode: do not change anything. You may read and search, but any attempt to edit a file or run a command is refused. Investigate, then answer with a plan: what you would change, in which files, and why."),
            _ => None,
        }
    }
}

/// The mode of each conversation, in memory: a restart goes back to `Manual`, which is the safe side to forget to.
#[derive(Clone, Default)]
pub struct CodeModes {
    modes: Arc<Mutex<HashMap<String, watch::Sender<CodeMode>>>>,
}

impl CodeModes {
    pub fn set(&self, conversation_id: &str, mode: CodeMode) {
        let mut modes = self.modes.lock().unwrap_or_else(|e| e.into_inner());
        match modes.get(conversation_id) {
            Some(sender) => {
                sender.send_replace(mode);
            }
            None => {
                modes.insert(conversation_id.to_string(), watch::channel(mode).0);
            }
        }
    }

    /// The conversation's mode, now and as it changes.
    pub fn subscribe(&self, conversation_id: &str) -> watch::Receiver<CodeMode> {
        let mut modes = self.modes.lock().unwrap_or_else(|e| e.into_inner());
        modes.entry(conversation_id.to_string()).or_insert_with(|| watch::channel(CodeMode::Manual).0).subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_parse_and_unknown_ones_ask() {
        assert_eq!(CodeMode::parse("plan"), CodeMode::Plan);
        assert_eq!(CodeMode::parse("acceptEdits"), CodeMode::AcceptEdits);
        assert_eq!(CodeMode::parse("acceptAll"), CodeMode::AcceptAll);
        assert_eq!(CodeMode::parse("manual"), CodeMode::Manual);
        assert_eq!(CodeMode::parse("yolo"), CodeMode::Manual, "unknown is the mode that asks");
        assert_eq!(serde_json::to_string(&CodeMode::AcceptEdits).unwrap(), r#""acceptEdits""#);
    }

    #[test]
    fn each_mode_decides_what_it_decides() {
        assert_eq!(CodeMode::Manual.decides("edit"), None);
        assert_eq!(CodeMode::AcceptEdits.decides("edit"), Some(true));
        assert_eq!(CodeMode::AcceptEdits.decides("bash"), None, "only changing files is let through");
        assert_eq!(CodeMode::AcceptAll.decides("bash"), Some(true));
        assert_eq!(CodeMode::Plan.decides("edit"), Some(false));
        assert_eq!(CodeMode::Plan.decides("bash"), Some(false));
        assert!(CodeMode::Plan.instructions().is_some() && CodeMode::Manual.instructions().is_none());
    }

    #[test]
    fn a_conversation_starts_manual_and_a_change_reaches_who_is_watching() {
        let modes = CodeModes::default();
        let mut watching = modes.subscribe("c1");
        assert_eq!(*watching.borrow(), CodeMode::Manual);
        modes.set("c1", CodeMode::Plan);
        assert!(watching.has_changed().unwrap());
        assert_eq!(*watching.borrow_and_update(), CodeMode::Plan);
        // Set before anyone watches: the first to watch sees it. Other conversations are untouched.
        modes.set("c2", CodeMode::AcceptAll);
        assert_eq!(*modes.subscribe("c2").borrow(), CodeMode::AcceptAll);
        assert_eq!(*modes.subscribe("c3").borrow(), CodeMode::Manual);
    }
}
