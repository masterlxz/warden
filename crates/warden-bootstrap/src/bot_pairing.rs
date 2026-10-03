//! P117 — pairing for the Telegram and WhatsApp bots. A stranger who writes to a bot with `pairing`
//! switched on gets a short code to hand to the owner; the owner approves it (`warden bots pair
//! approve`, the desktop or the web) and the sender lands on the bot's allow-list in `config.toml`.
//!
//! The pending requests live in `bot_pairing.json` beside the config, read again on every call: the
//! bots, the CLI and the desktop are separate processes that meet only in these two files, the way
//! `warden-server`'s device registry does. Nothing here is a secret worth guarding hard: a code only
//! tells the owner which pending request to approve, and approving is the owner's act.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};

use crate::{load_config_from_path, save_config};

pub const TELEGRAM: &str = "telegram";
pub const WHATSAPP: &str = "whatsapp";

/// How long a code stays valid.
pub const VALID_FOR_SECS: u64 = 60 * 60;
/// How long an approval waits for its bot: a bot that was down for longer doesn't announce it late.
pub const NOTIFY_WITHIN_SECS: u64 = 24 * 60 * 60;
/// Pending requests kept per channel: past this, a stranger gets nothing until one expires, so a flood
/// of strangers can't grow the file or bury the owner's list.
pub const MAX_PENDING_PER_CHANNEL: usize = 10;

/// One stranger waiting for the owner.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PairingRequest {
    /// `telegram` or `whatsapp`.
    pub channel: String,
    /// What would go on the allow-list: a Telegram user id, or a WhatsApp chat id.
    pub sender: String,
    /// What the owner recognises them by (a name or username), possibly empty.
    #[serde(default)]
    pub label: String,
    /// Eight characters from the sync pairing alphabet, uppercase.
    pub code: String,
    /// Unix seconds.
    pub expires_at: u64,
}

/// What the bot tells a sender once the owner has approved them.
pub const APPROVED_REPLY: &str = "Approved. You can talk to me now.";

/// Someone the owner let in, waiting for their bot to say so.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Approved {
    /// `telegram` or `whatsapp`.
    pub channel: String,
    /// The id the request came from: the chat the bot answers in.
    pub sender: String,
    /// Unix seconds.
    pub approved_at: u64,
}

/// What asking for a code came to.
#[derive(Debug, PartialEq)]
pub enum Issued {
    /// A new request: the bot tells the sender this code (shown as `ABCD-EFGH`, see [`display_code`]).
    Fresh(String),
    /// The sender already has a valid code: the bot stays quiet, so one person can't make it chatter.
    Existing,
    /// Too many strangers are waiting on this channel.
    Full,
}

/// `ABCD-EFGH`: easier to read off a screen and type than eight letters in a row.
pub fn display_code(code: &str) -> String {
    match code.split_at_checked(4) {
        Some((a, b)) if !b.is_empty() => format!("{a}-{b}"),
        _ => code.to_string(),
    }
}

impl From<PairingRequest> for warden_server_protocol::protocol::BotPairingDto {
    fn from(request: PairingRequest) -> Self {
        Self { channel: request.channel, sender: request.sender, label: request.label, code: display_code(&request.code), expires_at: request.expires_at }
    }
}

/// What the owner may type: any case, with or without the dash or spaces.
fn normalize(code: &str) -> String {
    code.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_uppercase()
}

/// The pending requests of one `config.toml`.
pub struct BotPairing {
    path: PathBuf,
}

impl BotPairing {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// The file that goes with the `config.toml` at `config_path`.
    pub fn beside(config_path: &Path) -> Self {
        Self::new(config_path.with_file_name("bot_pairing.json"))
    }

    fn read(&self, now: u64) -> anyhow::Result<Vec<PairingRequest>> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e).with_context(|| format!("reading {}", self.path.display())),
        };
        let mut requests: Vec<PairingRequest> = serde_json::from_str(&text).with_context(|| format!("parsing {}", self.path.display()))?;
        requests.retain(|r| r.expires_at > now);
        Ok(requests)
    }

    fn write(&self, requests: &[PairingRequest]) -> anyhow::Result<()> {
        let text = serde_json::to_string_pretty(requests)?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, &self.path).with_context(|| format!("replacing {}", self.path.display()))
    }

    /// The valid requests, oldest first.
    pub fn list(&self, now: u64) -> anyhow::Result<Vec<PairingRequest>> {
        self.read(now)
    }

    /// A code for `sender` on `channel`: the one they already have, or a new one.
    pub fn request(&self, channel: &str, sender: &str, label: &str, now: u64) -> anyhow::Result<Issued> {
        let mut requests = self.read(now)?;
        if requests.iter().any(|r| r.channel == channel && r.sender == sender) {
            return Ok(Issued::Existing);
        }
        if requests.iter().filter(|r| r.channel == channel).count() >= MAX_PENDING_PER_CHANNEL {
            return Ok(Issued::Full);
        }
        let code = loop {
            let code = warden_sync::pairing::protocol::generate_pairing_code();
            if !requests.iter().any(|r| r.code == code) {
                break code;
            }
        };
        requests.push(PairingRequest { channel: channel.to_string(), sender: sender.to_string(), label: label.trim().to_string(), code: code.clone(), expires_at: now + VALID_FOR_SECS });
        self.write(&requests)?;
        Ok(Issued::Fresh(code))
    }

    /// Drops a request without letting the sender in.
    pub fn deny(&self, code: &str, now: u64) -> anyhow::Result<PairingRequest> {
        let mut requests = self.read(now)?;
        let wanted = normalize(code);
        let Some(at) = requests.iter().position(|r| r.code == wanted) else { bail!("no pending pairing with code {}: it may have expired", display_code(&wanted)) };
        let removed = requests.remove(at);
        self.write(&requests)?;
        Ok(removed)
    }

    /// Lets the sender in: puts them on the bot's allow-list in the `config.toml` at `config_path`
    /// (once, however many times) and drops the request.
    pub fn approve(&self, code: &str, now: u64, config_path: &Path) -> anyhow::Result<PairingRequest> {
        self.approve_as(code, now, config_path, None)
    }

    /// Same as [`approve`](Self::approve), and with a `member` the chat speaks as them (P117): the hub
    /// answers it with their vault and tools instead of the owner's assistant. Refused, with nothing
    /// changed, when there's no such member, no `[bot_hub]`, or the member isn't linked (`bot_hub::link`):
    /// a chat approved as someone the bot can't reach would be a silent dead end.
    pub fn approve_as(&self, code: &str, now: u64, config_path: &Path, member: Option<&str>) -> anyhow::Result<PairingRequest> {
        let requests = self.read(now)?;
        let wanted = normalize(code);
        let Some(request) = requests.iter().find(|r| r.code == wanted).cloned() else { bail!("no pending pairing with code {}: it may have expired", display_code(&wanted)) };

        let mut config = load_config_from_path(config_path, false)?;
        if let Some(member) = member {
            if !config.users.iter().any(|u| u.id == member) {
                bail!("there is no member '{member}' in this workspace");
            }
            if config.bot_hub.as_ref().is_none_or(|hub| hub.url.trim().is_empty()) {
                bail!("a chat can speak as a member only through a hub: set `url` under [bot_hub] in config.toml (or run `warden bots link {member} --hub <url>`)");
            }
            if crate::bot_hub::HubTokens::beside(config_path).get(member)?.is_none() {
                bail!("'{member}' isn't linked to the bots yet: run `warden bots link {member}` first, and have them type their password");
            }
        }
        match request.channel.as_str() {
            TELEGRAM => {
                let id: i64 = request.sender.parse().ok().filter(|id| *id > 0).with_context(|| format!("'{}' is not a Telegram user id", request.sender))?;
                if !config.telegram.allowed_users.contains(&id) {
                    config.telegram.allowed_users.push(id);
                }
                if let Some(member) = member {
                    config.telegram.members.insert(id.to_string(), member.to_string());
                }
            }
            WHATSAPP => {
                if !config.whatsapp.allowed_chats.contains(&request.sender) {
                    config.whatsapp.allowed_chats.push(request.sender.clone());
                }
                if let Some(member) = member {
                    config.whatsapp.members.insert(request.sender.clone(), member.to_string());
                }
            }
            other => bail!("unknown channel '{other}'"),
        }
        save_config(config_path, &config)?;
        self.deny(&wanted, now)?;
        self.mark_approved(&request, now)?;
        Ok(request)
    }

    /// Beside the requests: the approvals the bots haven't announced yet.
    fn approved_path(&self) -> PathBuf {
        self.path.with_file_name("bot_pairing_approved.json")
    }

    fn read_approved(&self, now: u64) -> anyhow::Result<Vec<Approved>> {
        let path = self.approved_path();
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        let mut approved: Vec<Approved> = serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        approved.retain(|a| a.approved_at + NOTIFY_WITHIN_SECS > now);
        Ok(approved)
    }

    fn write_approved(&self, approved: &[Approved]) -> anyhow::Result<()> {
        let path = self.approved_path();
        let text = serde_json::to_string_pretty(approved)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, &path).with_context(|| format!("replacing {}", path.display()))
    }

    /// Notes that `request`'s sender is in, once per sender, for their bot to pick up.
    fn mark_approved(&self, request: &PairingRequest, now: u64) -> anyhow::Result<()> {
        let mut approved = self.read_approved(now)?;
        if approved.iter().any(|a| a.channel == request.channel && a.sender == request.sender) {
            return Ok(());
        }
        approved.push(Approved { channel: request.channel.clone(), sender: request.sender.clone(), approved_at: now });
        self.write_approved(&approved)
    }

    /// The approvals on `channel` the bot hasn't announced, removed as they are handed over (other
    /// channels' and expired ones aside): the bot tells each sender it can write now.
    pub fn take_approved(&self, channel: &str, now: u64) -> anyhow::Result<Vec<Approved>> {
        let approved = self.read_approved(now)?;
        if !approved.iter().any(|a| a.channel == channel) {
            return Ok(Vec::new());
        }
        let (mine, rest): (Vec<_>, Vec<_>) = approved.into_iter().partition(|a| a.channel == channel);
        self.write_approved(&rest)?;
        Ok(mine)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static NEXT: AtomicU32 = AtomicU32::new(0);

    /// A scratch directory with a `config.toml` holding `config`.
    fn setup(config: &str) -> (PathBuf, BotPairing) {
        let dir = std::env::temp_dir().join(format!("warden-bot-pairing-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::SeqCst)));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, config).unwrap();
        let store = BotPairing::beside(&path);
        (path, store)
    }

    fn fresh(issued: Issued) -> String {
        match issued {
            Issued::Fresh(code) => code,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_sender_gets_one_code_and_asking_again_stays_quiet() {
        let (_, store) = setup("");
        let code = fresh(store.request(TELEGRAM, "42", "Ana", 100).unwrap());
        assert_eq!(code.len(), 8);
        assert_eq!(store.request(TELEGRAM, "42", "Ana", 200).unwrap(), Issued::Existing);
        // The same id on the other channel is another sender.
        assert!(matches!(store.request(WHATSAPP, "42", "", 200).unwrap(), Issued::Fresh(_)));
        let pending = store.list(300).unwrap();
        assert_eq!(pending.len(), 2);
        assert_eq!((pending[0].sender.as_str(), pending[0].label.as_str(), pending[0].expires_at), ("42", "Ana", 100 + VALID_FOR_SECS));
    }

    #[test]
    fn a_code_expires_and_the_sender_can_ask_again() {
        let (path, store) = setup("");
        let first = fresh(store.request(TELEGRAM, "42", "", 100).unwrap());
        assert_eq!(store.list(100 + VALID_FOR_SECS - 1).unwrap().len(), 1);
        assert!(store.list(100 + VALID_FOR_SECS).unwrap().is_empty(), "gone at the deadline");
        assert!(store.approve(&first, 100 + VALID_FOR_SECS, &path).is_err(), "an expired code approves nothing");
        assert!(load_config_from_path(&path, false).unwrap().telegram.allowed_users.is_empty());
        fresh(store.request(TELEGRAM, "42", "", 100 + VALID_FOR_SECS).unwrap());
        assert_eq!(store.list(100 + VALID_FOR_SECS).unwrap().len(), 1);
    }

    #[test]
    fn strangers_past_the_cap_get_nothing_and_the_cap_is_per_channel() {
        let (_, store) = setup("");
        for id in 0..MAX_PENDING_PER_CHANNEL {
            fresh(store.request(TELEGRAM, &(id + 1).to_string(), "", 100).unwrap());
        }
        assert_eq!(store.request(TELEGRAM, "999", "", 100).unwrap(), Issued::Full);
        assert!(matches!(store.request(WHATSAPP, "5511999999999@s.whatsapp.net", "", 100).unwrap(), Issued::Fresh(_)));
        assert!(matches!(store.request(TELEGRAM, "999", "", 100 + VALID_FOR_SECS).unwrap(), Issued::Fresh(_)), "room again once they expire");
    }

    #[test]
    fn approving_puts_the_sender_on_the_right_list_once_and_drops_the_request() {
        let (path, store) = setup("[telegram]\nallowed_users = [7]\n");
        let code = fresh(store.request(TELEGRAM, "42", "Ana", 100).unwrap());
        let wa = fresh(store.request(WHATSAPP, "5511999999999@s.whatsapp.net", "", 100).unwrap());

        // Any case, with the dash: the way a person types it.
        let typed = format!("{} ", display_code(&code).to_lowercase());
        let approved = store.approve(&typed, 200, &path).unwrap();
        assert_eq!((approved.channel.as_str(), approved.sender.as_str()), (TELEGRAM, "42"));
        store.approve(&wa, 200, &path).unwrap();

        let config = load_config_from_path(&path, false).unwrap();
        assert_eq!(config.telegram.allowed_users, [7, 42], "the one that was there stays");
        assert_eq!(config.whatsapp.allowed_chats, ["5511999999999@s.whatsapp.net"]);
        assert!(store.list(200).unwrap().is_empty());
        assert!(store.approve(&code, 200, &path).is_err(), "a code works once");

        // The same person pairing again doesn't double their entry.
        let again = fresh(store.request(TELEGRAM, "42", "Ana", 300).unwrap());
        store.approve(&again, 300, &path).unwrap();
        assert_eq!(load_config_from_path(&path, false).unwrap().telegram.allowed_users, [7, 42]);
    }

    #[test]
    fn an_approval_is_handed_to_its_own_bot_once() {
        let (path, store) = setup("");
        let tg = fresh(store.request(TELEGRAM, "42", "", 100).unwrap());
        let wa = fresh(store.request(WHATSAPP, "5511999999999@lid", "", 100).unwrap());
        assert!(store.take_approved(TELEGRAM, 150).unwrap().is_empty(), "nothing approved yet");
        store.approve(&tg, 200, &path).unwrap();
        store.approve(&wa, 200, &path).unwrap();

        let taken = store.take_approved(TELEGRAM, 210).unwrap();
        assert_eq!(taken, [Approved { channel: TELEGRAM.into(), sender: "42".into(), approved_at: 200 }]);
        assert!(store.take_approved(TELEGRAM, 220).unwrap().is_empty(), "announced once");
        let whatsapp = store.take_approved(WHATSAPP, 220).unwrap();
        assert_eq!(whatsapp.len(), 1, "the other channel's approval was left alone");
        assert_eq!(whatsapp[0].sender, "5511999999999@lid");
    }

    #[test]
    fn an_approval_waiting_for_a_bot_that_was_down_goes_stale() {
        let (path, store) = setup("");
        let code = fresh(store.request(TELEGRAM, "42", "", 100).unwrap());
        store.approve(&code, 200, &path).unwrap();
        assert!(store.take_approved(TELEGRAM, 200 + NOTIFY_WITHIN_SECS).unwrap().is_empty());
    }

    #[test]
    fn approving_twice_before_the_bot_looks_announces_once() {
        let (path, store) = setup("");
        let first = fresh(store.request(TELEGRAM, "42", "", 100).unwrap());
        store.approve(&first, 150, &path).unwrap();
        let again = fresh(store.request(TELEGRAM, "42", "", 160).unwrap());
        store.approve(&again, 170, &path).unwrap();
        assert_eq!(store.take_approved(TELEGRAM, 180).unwrap().len(), 1);
    }

    #[test]
    fn denying_announces_nothing() {
        let (_, store) = setup("");
        let code = fresh(store.request(TELEGRAM, "42", "", 100).unwrap());
        store.deny(&code, 150).unwrap();
        assert!(store.take_approved(TELEGRAM, 160).unwrap().is_empty());
    }

    #[test]
    fn denying_drops_the_request_and_changes_no_list() {
        let (path, store) = setup("");
        let code = fresh(store.request(TELEGRAM, "42", "", 100).unwrap());
        store.deny(&code, 150).unwrap();
        assert!(store.list(150).unwrap().is_empty());
        assert!(store.deny(&code, 150).is_err());
        assert!(load_config_from_path(&path, false).unwrap().telegram.allowed_users.is_empty());
    }

    #[test]
    fn a_telegram_sender_that_is_not_a_user_id_is_refused_and_stays_pending() {
        let (path, store) = setup("");
        let code = fresh(store.request(TELEGRAM, "-100123", "", 100).unwrap());
        assert!(store.approve(&code, 150, &path).is_err());
        assert_eq!(store.list(150).unwrap().len(), 1, "still there for the owner to deny");
    }

    #[test]
    fn the_code_reads_in_two_halves() {
        assert_eq!(display_code("ABCD2345"), "ABCD-2345");
        assert_eq!(normalize(" abcd-2345 "), "ABCD2345");
    }
}
