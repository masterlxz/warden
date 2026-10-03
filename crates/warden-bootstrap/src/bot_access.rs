//! P117 — who may talk to the Telegram and WhatsApp bots. A bot used to answer anyone who wrote to it,
//! with the owner's agent, vault and tools; now it answers only the people listed here, and only in a
//! private chat. An empty list (the default) means nobody.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The `config.toml` a bot reads its lists from: the `--config` it was given, else the default one.
pub fn config_path(explicit: Option<&str>) -> Option<PathBuf> {
    explicit.map(PathBuf::from).or_else(crate::default_config_path)
}

/// The `config.toml` at `path`, read again (the owner edits the lists, `pairing` and `[learning]`
/// while the bot runs). An error when the file can't be read or parsed, so the caller keeps what it had.
pub fn read_config(path: &Path) -> anyhow::Result<crate::FileConfig> {
    crate::load_config_from_path(path, false)
}

/// Seconds since the Unix epoch, what `bot_pairing` counts expiry in.
pub fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// `[telegram]` in `config.toml`.
#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TelegramSettings {
    /// Telegram user ids (numbers, as `@userinfobot` shows them) who may talk to the bot. A username
    /// isn't used: it can change and may be missing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_users: Vec<i64>,
    /// Gives a stranger in a private chat a code to hand to the owner (`bot_pairing.rs`), instead of
    /// silence. Off unless the owner turns it on.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pairing: bool,
}

impl TelegramSettings {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Whether a message from user `user_id` in a chat of kind `chat_kind` (`private`, `group`,
    /// `supergroup`, `channel`) may be answered: only a listed person, only in a private chat.
    pub fn allows(&self, user_id: Option<i64>, chat_kind: &str) -> bool {
        chat_kind == "private" && user_id.is_some_and(|id| self.allowed_users.contains(&id))
    }
}

/// `[whatsapp]` in `config.toml`.
#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WhatsAppSettings {
    /// Chats that may talk to the bot: a phone number (`5511999999999`) or the whole id the bot logs
    /// (`5511999999999@s.whatsapp.net`, or an `@lid` one).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_chats: Vec<String>,
    /// Gives a stranger in a private chat a code to hand to the owner (`bot_pairing.rs`), instead of
    /// silence. Off unless the owner turns it on.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pairing: bool,
}

impl WhatsAppSettings {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Whether chat `chat_id` may be answered: a private chat (never a group, a status or a
    /// newsletter) that is listed, by its whole id or by its number.
    pub fn allows(&self, chat_id: &str) -> bool {
        is_private_chat(chat_id)
            && self.allowed_chats.iter().any(|entry| {
                let entry = entry.trim().trim_start_matches('+');
                !entry.is_empty() && (entry == chat_id || chat_id.strip_prefix(entry).is_some_and(|rest| rest.starts_with('@')))
            })
    }
}

/// WhatsApp ids ending in `@s.whatsapp.net` (a phone number) or `@lid` are one person; `@g.us` (a group),
/// `status@broadcast` and `@newsletter` are not.
pub fn is_private_chat(chat_id: &str) -> bool {
    chat_id.ends_with("@s.whatsapp.net") || chat_id.ends_with("@lid")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn telegram_answers_only_a_listed_person_in_a_private_chat() {
        let none = TelegramSettings::default();
        assert!(!none.allows(Some(42), "private"), "an empty list is nobody");
        let settings = TelegramSettings { allowed_users: vec![42], ..Default::default() };
        assert!(settings.allows(Some(42), "private"));
        assert!(!settings.allows(Some(43), "private"));
        assert!(!settings.allows(None, "private"), "a message with no sender is never allowed");
        for kind in ["group", "supergroup", "channel", ""] {
            assert!(!settings.allows(Some(42), kind), "{kind}: even a listed person, not in a group");
        }
    }

    #[test]
    fn whatsapp_answers_a_listed_number_or_id_in_a_private_chat_only() {
        let none = WhatsAppSettings::default();
        assert!(!none.allows("5511999999999@s.whatsapp.net"));
        let settings = WhatsAppSettings { allowed_chats: vec!["5511999999999".into(), "+5511888888888".into(), "abc123@lid".into(), "  ".into(), "".into()], ..Default::default() };
        assert!(settings.allows("5511999999999@s.whatsapp.net"), "by number");
        assert!(settings.allows("5511888888888@s.whatsapp.net"), "a leading + is ignored");
        assert!(settings.allows("abc123@lid"), "by the whole id");
        assert!(!settings.allows("551199999999@s.whatsapp.net"), "no prefix of the number");
        assert!(!settings.allows("55119999999990@s.whatsapp.net"), "no longer number either");
        assert!(!settings.allows("5511999999999@g.us"), "a group is never a chat to answer");
        assert!(!settings.allows("status@broadcast"));
        assert!(!settings.allows("@s.whatsapp.net"), "blank entries match nothing");
        let group = WhatsAppSettings { allowed_chats: vec!["5511999999999@g.us".into()], ..Default::default() };
        assert!(!group.allows("5511999999999@g.us"), "listing a group doesn't open it");
    }

    #[test]
    fn the_lists_stay_out_of_the_file_when_empty_and_round_trip_when_set() {
        assert!(!toml::to_string(&TelegramSettings::default()).unwrap().contains("allowed_users"));
        assert!(!toml::to_string(&WhatsAppSettings::default()).unwrap().contains("allowed_chats"));
        let telegram = TelegramSettings { allowed_users: vec![1, 2], ..Default::default() };
        assert_eq!(toml::from_str::<TelegramSettings>(&toml::to_string(&telegram).unwrap()).unwrap(), telegram);
        let whatsapp = WhatsAppSettings { allowed_chats: vec!["5511999999999".into()], pairing: true };
        assert_eq!(toml::from_str::<WhatsAppSettings>(&toml::to_string(&whatsapp).unwrap()).unwrap(), whatsapp);
    }
}
