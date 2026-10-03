//! P117 — who may talk to the Telegram and WhatsApp bots. A bot used to answer anyone who wrote to it,
//! with the owner's agent, vault and tools; now it answers only the people listed here, and only in a
//! private chat. An empty list (the default) means nobody.

use std::collections::BTreeMap;
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
    /// Telegram user id (as text, a TOML key) → id of the member of the workspace the chat speaks as
    /// (P84). A listed chat with an entry here is answered by the hub as that member (their vault and
    /// tools, `bot_hub.rs`); one without is answered as the owner. Listing isn't enough: the gate above
    /// still decides who is answered at all.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub members: BTreeMap<String, String>,
}

impl TelegramSettings {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// The member `user_id` speaks as, when the owner mapped them to one.
    pub fn member_for(&self, user_id: i64) -> Option<&str> {
        self.members.get(&user_id.to_string()).map(String::as_str).filter(|m| !m.is_empty())
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
    /// Chat (a number or a whole id, as in `allowed_chats`) → id of the member of the workspace the chat
    /// speaks as (P84). Same meaning as `[telegram] members`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub members: BTreeMap<String, String>,
}

/// Whether the list entry `entry` (a number, or a whole id, a leading `+` ignored) names `chat_id`.
fn entry_names_chat(entry: &str, chat_id: &str) -> bool {
    let entry = entry.trim().trim_start_matches('+');
    !entry.is_empty() && (entry == chat_id || chat_id.strip_prefix(entry).is_some_and(|rest| rest.starts_with('@')))
}

impl WhatsAppSettings {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Whether chat `chat_id` may be answered: a private chat (never a group, a status or a
    /// newsletter) that is listed, by its whole id or by its number.
    pub fn allows(&self, chat_id: &str) -> bool {
        is_private_chat(chat_id) && self.allowed_chats.iter().any(|entry| entry_names_chat(entry, chat_id))
    }

    /// The member `chat_id` speaks as, when the owner mapped them to one. The whole id wins over a bare number.
    pub fn member_for(&self, chat_id: &str) -> Option<&str> {
        let exact = self.members.iter().find(|(entry, _)| entry.trim() == chat_id);
        let found = exact.or_else(|| self.members.iter().find(|(entry, _)| entry_names_chat(entry, chat_id)));
        found.map(|(_, member)| member.as_str()).filter(|m| !m.is_empty())
    }
}

/// `[bot_hub]` in `config.toml`: the hub the bots ask on behalf of a member (`bot_hub.rs`).
#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BotHubSettings {
    /// `ws://host:port` (a LAN hub) or `wss://host:port` (needs a certificate a public authority signed).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub url: String,
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
        let whatsapp = WhatsAppSettings { allowed_chats: vec!["5511999999999".into()], pairing: true, ..Default::default() };
        assert_eq!(toml::from_str::<WhatsAppSettings>(&toml::to_string(&whatsapp).unwrap()).unwrap(), whatsapp);
    }

    #[test]
    fn a_file_without_members_still_reads_and_the_map_round_trips() {
        let old: TelegramSettings = toml::from_str("allowed_users = [42]\n").unwrap();
        assert!(old.members.is_empty());
        assert!(!toml::to_string(&old).unwrap().contains("members"));
        let telegram: TelegramSettings = toml::from_str("allowed_users = [42]\n[members]\n42 = \"ana\"\n").unwrap();
        assert_eq!(telegram.member_for(42), Some("ana"));
        assert_eq!(toml::from_str::<TelegramSettings>(&toml::to_string(&telegram).unwrap()).unwrap(), telegram);
        let whatsapp: WhatsAppSettings = toml::from_str("allowed_chats = [\"5511999999999\"]\n[members]\n5511999999999 = \"ana\"\n").unwrap();
        assert_eq!(whatsapp.member_for("5511999999999@s.whatsapp.net"), Some("ana"));
    }

    #[test]
    fn a_telegram_chat_speaks_as_its_member_and_an_unmapped_one_as_the_owner() {
        let settings = TelegramSettings { members: [("42".to_string(), "ana".to_string()), ("43".to_string(), String::new())].into(), ..Default::default() };
        assert_eq!(settings.member_for(42), Some("ana"));
        assert_eq!(settings.member_for(43), None, "an empty member is no member");
        assert_eq!(settings.member_for(44), None);
    }

    #[test]
    fn a_whatsapp_chat_is_found_by_number_or_whole_id_and_the_whole_id_wins() {
        let settings = WhatsAppSettings {
            members: [("5511999999999".to_string(), "ana".to_string()), ("+5511888888888".to_string(), "bia".to_string()), ("abc123@lid".to_string(), "caio".to_string()), ("5511999999999@s.whatsapp.net".to_string(), "dani".to_string())].into(),
            ..Default::default()
        };
        assert_eq!(settings.member_for("5511999999999@s.whatsapp.net"), Some("dani"), "the whole id beats the number");
        assert_eq!(settings.member_for("5511999999999@lid"), Some("ana"), "the number alone covers the other suffix");
        assert_eq!(settings.member_for("5511888888888@s.whatsapp.net"), Some("bia"), "a leading + is ignored");
        assert_eq!(settings.member_for("abc123@lid"), Some("caio"));
        assert_eq!(settings.member_for("551199999999@s.whatsapp.net"), None, "no prefix of the number");
    }
}
