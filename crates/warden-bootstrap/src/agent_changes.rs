//! The agents an agent created or removed (P121), for the feed of activity: one JSON line per change in `agent_changes.jsonl`, next to the
//! `config.toml` the change was saved in.
//!
//! Only `manage_agents` writes here, after the person said yes and the change was saved: what a person does from a screen they already know.
//! The settings only keep the agents that exist, so without this line nothing would say who made an agent, or that a removed one ever was.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The most changes a read hands back (the newest ones).
pub const MAX_CHANGES: usize = 200;

/// One agent created or removed by another.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentChange {
    pub at_ms: u64,
    /// `created` or `removed`.
    pub kind: String,
    /// The agent that made the change; empty for the assistant with no agent.
    pub actor: String,
    /// The agent created or removed.
    pub agent: String,
    /// What the new agent is for (its role, or the start of its persona); empty for a removal.
    #[serde(default)]
    pub detail: String,
}

/// The log that goes with the `config.toml` at `config_path`.
pub fn beside(config_path: &Path) -> PathBuf {
    config_path.with_file_name("agent_changes.jsonl")
}

/// Adds `change` to the log at `path`. A log that can't be written says nothing: the change it describes is already approved and saved.
pub fn record(path: &Path, change: &AgentChange) {
    let Ok(mut line) = serde_json::to_string(change) else { return };
    line.push('\n');
    let _ = OpenOptions::new().create(true).append(true).open(path).and_then(|mut file| file.write_all(line.as_bytes()));
}

/// The changes in the log at `path`, oldest first, at most the newest `MAX_CHANGES`. A missing file is none; a line that can't be read is
/// skipped.
pub fn read_agent_changes(path: &Path) -> Vec<AgentChange> {
    let Ok(text) = std::fs::read_to_string(path) else { return Vec::new() };
    let mut changes: Vec<AgentChange> = text.lines().filter_map(|line| serde_json::from_str(line).ok()).collect();
    let skip = changes.len().saturating_sub(MAX_CHANGES);
    changes.drain(..skip);
    changes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_log() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("warden-agent-changes-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&dir).unwrap();
        beside(&dir.join("config.toml"))
    }

    fn change(at_ms: u64, kind: &str, agent: &str) -> AgentChange {
        AgentChange { at_ms, kind: kind.into(), actor: "chief".into(), agent: agent.into(), detail: String::new() }
    }

    #[test]
    fn written_changes_read_back_in_order() {
        let log = temp_log();
        assert!(read_agent_changes(&log).is_empty(), "no file yet is no change");
        record(&log, &change(1, "created", "poet"));
        record(&log, &change(2, "removed", "poet"));
        assert_eq!(read_agent_changes(&log), vec![change(1, "created", "poet"), change(2, "removed", "poet")]);
        assert_eq!(log.file_name().unwrap(), "agent_changes.jsonl");
    }

    #[test]
    fn a_bad_line_is_skipped_and_only_the_newest_are_kept() {
        let log = temp_log();
        record(&log, &change(1, "created", "a"));
        std::fs::OpenOptions::new().append(true).open(&log).unwrap().write_all(b"not json\n").unwrap();
        for n in 0..MAX_CHANGES as u64 {
            record(&log, &change(10 + n, "created", "b"));
        }
        let changes = read_agent_changes(&log);
        assert_eq!(changes.len(), MAX_CHANGES);
        assert_eq!(changes[0].at_ms, 10, "the oldest one fell off; the bad line was never counted");
    }
}
