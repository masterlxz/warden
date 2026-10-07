//! P121 — what the hub wants the Telegram and WhatsApp bots to say on their own. The bots are other processes that meet the hub only in
//! files beside the `config.toml` (`bot_pairing.json`, `bot_hub.json`); this is one more: a folder `bot_outbox/` with one small file per
//! message, written by the hub when an agent the owner allowed to start messages (`[[outreach]]`) forwards one, and read by the bot of that
//! channel on its next look, which sends it to the owner's chats and removes it.
//!
//! One file per message and not one shared list, so the hub writing and a bot reading never rewrite the same file: no lock, no lost message.
//! A message nobody collects in `EXPIRES_AFTER_SECS` is dropped, so a bot that was down for days does not announce old news, and a channel
//! keeps at most `MAX_QUEUED_PER_CHANNEL`, so a bot that is off cannot let the folder grow without end.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::Context;
use serde::{Deserialize, Serialize};

/// How long a message waits for its bot.
pub const EXPIRES_AFTER_SECS: u64 = 24 * 60 * 60;
/// Messages kept per channel; past this the oldest go.
pub const MAX_QUEUED_PER_CHANNEL: usize = 50;

static NEXT: AtomicU64 = AtomicU64::new(0);

/// One message waiting for a bot.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Outgoing {
    /// `telegram` or `whatsapp`.
    pub channel: String,
    pub text: String,
    /// Unix seconds.
    pub queued_at: u64,
}

/// The outbox that goes with one `config.toml`.
#[derive(Clone, Debug)]
pub struct BotOutbox {
    dir: PathBuf,
}

impl BotOutbox {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// The folder that goes with the `config.toml` at `config_path`.
    pub fn beside(config_path: &Path) -> Self {
        Self::new(config_path.with_file_name("bot_outbox"))
    }

    /// The files of `channel`, oldest first. The file name starts with the channel and a zero-padded time, so the name order is the order sent.
    fn files(&self, channel: &str) -> anyhow::Result<Vec<PathBuf>> {
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e).with_context(|| format!("reading {}", self.dir.display())),
        };
        let prefix = format!("{channel}-");
        let mut files: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "json") && p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(&prefix)))
            .collect();
        files.sort();
        Ok(files)
    }

    /// Leaves `text` for the bot of `channel`. Past `MAX_QUEUED_PER_CHANNEL` the oldest waiting message is dropped to make room.
    pub fn push(&self, channel: &str, text: &str, now: u64) -> anyhow::Result<()> {
        std::fs::create_dir_all(&self.dir).with_context(|| format!("creating {}", self.dir.display()))?;
        let waiting = self.files(channel)?;
        for old in waiting.iter().take((waiting.len() + 1).saturating_sub(MAX_QUEUED_PER_CHANNEL)) {
            let _ = std::fs::remove_file(old);
        }
        let name = format!("{channel}-{now:012}-{}-{:06}.json", std::process::id(), NEXT.fetch_add(1, Ordering::SeqCst));
        let path = self.dir.join(&name);
        let tmp = self.dir.join(format!("{name}.tmp"));
        let text = serde_json::to_string(&Outgoing { channel: channel.to_string(), text: text.to_string(), queued_at: now })?;
        std::fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, &path).with_context(|| format!("replacing {}", path.display()))
    }

    /// The messages waiting for `channel`, oldest first, removed as they are handed over. An expired or unreadable one is removed and
    /// not returned.
    pub fn take(&self, channel: &str, now: u64) -> anyhow::Result<Vec<Outgoing>> {
        let mut taken = Vec::new();
        for path in self.files(channel)? {
            let read = std::fs::read_to_string(&path).ok().and_then(|text| serde_json::from_str::<Outgoing>(&text).ok());
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                // Another reader took it first: not ours to send twice.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e).with_context(|| format!("removing {}", path.display())),
            }
            if let Some(message) = read.filter(|m| now.saturating_sub(m.queued_at) < EXPIRES_AFTER_SECS) {
                taken.push(message);
            }
        }
        Ok(taken)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outbox() -> (PathBuf, BotOutbox) {
        let dir = std::env::temp_dir().join(format!("warden-bot-outbox-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&dir).unwrap();
        let config = dir.join("config.toml");
        (dir, BotOutbox::beside(&config))
    }

    #[test]
    fn a_bot_takes_its_own_channels_messages_once_and_in_order() {
        let (dir, outbox) = outbox();
        outbox.push("telegram", "first", 100).unwrap();
        outbox.push("whatsapp", "other channel", 100).unwrap();
        outbox.push("telegram", "second", 101).unwrap();

        let taken = outbox.take("telegram", 105).unwrap();
        assert_eq!(taken.iter().map(|m| m.text.as_str()).collect::<Vec<_>>(), ["first", "second"]);
        assert!(outbox.take("telegram", 105).unwrap().is_empty(), "handed over once");
        assert_eq!(outbox.take("whatsapp", 105).unwrap()[0].text, "other channel", "the other channel's message waited");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn nothing_waiting_is_not_an_error_and_an_old_message_is_not_announced() {
        let (dir, outbox) = outbox();
        assert!(outbox.take("telegram", 1).unwrap().is_empty(), "no folder yet");
        outbox.push("telegram", "stale", 100).unwrap();
        outbox.push("telegram", "fresh", 100 + EXPIRES_AFTER_SECS - 1).unwrap();
        let taken = outbox.take("telegram", 100 + EXPIRES_AFTER_SECS).unwrap();
        assert_eq!(taken.iter().map(|m| m.text.as_str()).collect::<Vec<_>>(), ["fresh"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_channel_keeps_the_latest_fifty_and_a_broken_file_is_dropped() {
        let (dir, outbox) = outbox();
        for i in 0..MAX_QUEUED_PER_CHANNEL + 5 {
            outbox.push("telegram", &format!("m{i}"), 1000 + i as u64).unwrap();
        }
        std::fs::write(dir.join("bot_outbox").join("telegram-000000000000-broken.json"), "not json").unwrap();
        let taken = outbox.take("telegram", 1100).unwrap();
        assert_eq!(taken.len(), MAX_QUEUED_PER_CHANNEL);
        assert_eq!(taken[0].text, "m5", "the oldest went to make room");
        assert_eq!(taken.last().unwrap().text, format!("m{}", MAX_QUEUED_PER_CHANNEL + 4));
        assert!(std::fs::read_dir(dir.join("bot_outbox")).unwrap().next().is_none(), "the broken file went too");
        std::fs::remove_dir_all(&dir).ok();
    }
}
