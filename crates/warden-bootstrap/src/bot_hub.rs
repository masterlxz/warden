//! P117 — a Telegram or WhatsApp chat that speaks as a member of the workspace (P84). The member's vault
//! is encrypted with a key only the hub holds (opened by their password), and the bot is another process,
//! so the bot doesn't open anything itself: it asks the hub, as that member, over the same WebSocket
//! the apps use. The hub runs the turn with the member's vault, tools and limits.
//!
//! `warden bots link <member>` signs in once with the member's password and keeps only the device token
//! the hub issues, in `bot_hub.json` beside the `config.toml` (owner-readable only). The password isn't
//! kept: when the hub restarts, the member's data stays locked until they sign in themselves, and the
//! bot says so instead of unlocking it.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use warden_core::model::Attachment;
use warden_server_protocol::protocol::{BotMemberDto, ClientMessage, ServerMessage, UserInfoDto};
use warden_server_protocol::tls::default_client_config;
use warden_server_protocol::{AuthRejected, ServerConnection};

/// How long to wait for the hub to accept a connection.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// How long a turn may take: a model with tools can run for minutes.
const TURN_TIMEOUT: Duration = Duration::from_secs(180);
/// What the hub lists the bots' device as.
const DEVICE_NAME: &str = "Warden bots";

/// What a chat is told when its member isn't linked, or the hub turned the token away.
pub const NOT_LINKED_REPLY: &str = "I'm not connected to your account any more. Ask the owner to link it again.";
/// What a chat is told when the hub can't be reached or doesn't answer.
pub const HUB_DOWN_REPLY: &str = "I couldn't reach the hub right now. Try again in a moment.";
/// What a chat is told when it speaks as a member but no hub is set up.
pub const NO_HUB_REPLY: &str = "This chat isn't connected to a hub yet. Ask the owner to set one up.";

/// The conversation a chat keeps on the hub: `telegram-<id>` or `whatsapp-<id>`. The hub takes 1 to 64
/// letters, digits, `-` and `_`, so anything else in a WhatsApp id (`@`, `.`) becomes `_`.
pub fn conversation_id(channel: &str, chat: &str) -> String {
    let chat: String = chat.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    let mut id = format!("{channel}-{chat}");
    id.truncate(64);
    id
}

/// The id the hub registers a member's bot device under.
fn device_id(member: &str) -> String {
    format!("warden-bot-{member}")
}

/// One member's link: the device the hub knows, and the token it issued for it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Linked {
    pub device_id: String,
    pub device_token: String,
}

#[derive(Serialize, Deserialize, Default)]
struct TokenFile {
    #[serde(default)]
    members: BTreeMap<String, Linked>,
}

/// The links of one `config.toml`, in `bot_hub.json` beside it. Read again on every call: the CLI that
/// links and the bots that use it are separate processes.
pub struct HubTokens {
    path: PathBuf,
}

impl HubTokens {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// The file that goes with the `config.toml` at `config_path`.
    pub fn beside(config_path: &Path) -> Self {
        Self::new(config_path.with_file_name("bot_hub.json"))
    }

    fn read(&self) -> anyhow::Result<TokenFile> {
        match std::fs::read_to_string(&self.path) {
            Ok(text) => serde_json::from_str(&text).with_context(|| format!("parsing {}", self.path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(TokenFile::default()),
            Err(e) => Err(e).with_context(|| format!("reading {}", self.path.display())),
        }
    }

    fn write(&self, file: &TokenFile) -> anyhow::Result<()> {
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(file)?).with_context(|| format!("writing {}", tmp.display()))?;
        // The token is what lets the bot speak as the member: only the owner's account reads it.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
        }
        std::fs::rename(&tmp, &self.path).with_context(|| format!("replacing {}", self.path.display()))
    }

    pub fn get(&self, member: &str) -> anyhow::Result<Option<Linked>> {
        Ok(self.read()?.members.get(member).cloned())
    }

    pub fn set(&self, member: &str, linked: Linked) -> anyhow::Result<()> {
        let mut file = self.read()?;
        file.members.insert(member.to_string(), linked);
        self.write(&file)
    }

    /// Whether `member` was linked.
    pub fn remove(&self, member: &str) -> anyhow::Result<bool> {
        let mut file = self.read()?;
        let was = file.members.remove(member).is_some();
        if was {
            self.write(&file)?;
        }
        Ok(was)
    }

    /// The members with a link, in order.
    pub fn linked(&self) -> anyhow::Result<Vec<String>> {
        Ok(self.read()?.members.into_keys().collect())
    }
}

/// Who a chat may be approved as speaking as: every member in the `config.toml` at `config_path`, in the file's
/// order, and whether the bots are linked to the hub as them. One answer for the desktop and the web, so both
/// show the same choices (an unlinked member can't be chosen: `BotPairing::approve_as` refuses it).
pub fn bot_members(config_path: &Path) -> anyhow::Result<Vec<BotMemberDto>> {
    let config = crate::load_config_from_path(config_path, false)?;
    let linked = HubTokens::beside(config_path).linked()?;
    Ok(config.users.iter().map(|u| BotMemberDto { id: u.id.clone(), name: u.name.clone(), linked: linked.contains(&u.id) }).collect())
}

/// Signs in to the hub at `hub_url` as `member` with their password and keeps the device token it
/// issues (not the password). Says who the hub took them for, so the caller can warn about a locked
/// vault or a password still to change.
pub async fn link(config_path: &Path, hub_url: &str, member: &str, password: &str) -> anyhow::Result<UserInfoDto> {
    let device_id = device_id(member);
    // A bot can't show a recovery code, so it never lets the hub make one at this sign-in (it would
    // seal the member's data with a key nobody could recover): the member's own apps do that.
    let signing_in = ServerConnection::handshake_as_member_showing(hub_url, &device_id, DEVICE_NAME, member, password, None, default_client_config(), false);
    let (_conn, token, user) = tokio::time::timeout(CONNECT_TIMEOUT, signing_in).await.context("the hub didn't answer in time")??;
    let device_token = token.context("the hub issued no device token")?;
    HubTokens::beside(config_path).set(member, Linked { device_id, device_token })?;
    user.context("the hub didn't say who signed in")
}

/// What a member's chat got back.
#[derive(Debug, PartialEq)]
pub enum MemberReply {
    /// The member's answer, with any media a tool produced.
    Text { content: String, attachments: Vec<Attachment> },
    /// The turn didn't happen: the text to tell the chat (the hub's own wording when it refused).
    Failed(String),
}

/// Asks the hub as a member. A trait so the bots' tests don't need a hub.
#[async_trait]
pub trait MemberChat: Send + Sync {
    async fn ask(&self, member: &str, conversation_id: &str, text: &str) -> MemberReply;
}

/// How one turn on a connection ended without an answer.
enum Lost {
    /// The connection went away (or was already gone) before the answer.
    Dropped,
    TimedOut,
}

type Slot = Arc<tokio::sync::Mutex<Option<ServerConnection>>>;

/// The real thing: one lazily opened connection per member, reused between turns. A member's turns go
/// one at a time (the hub doesn't guard two turns of one conversation racing on its file).
pub struct HubMemberChat {
    config_path: PathBuf,
    sessions: std::sync::Mutex<HashMap<String, Slot>>,
}

impl HubMemberChat {
    /// Reads `[bot_hub]` from `config_path` on every turn, so setting the hub up needs no restart.
    pub fn new(config_path: PathBuf) -> Self {
        Self { config_path, sessions: std::sync::Mutex::new(HashMap::new()) }
    }

    fn hub_url(&self) -> Option<String> {
        match crate::bot_access::read_config(&self.config_path) {
            Ok(config) => config.bot_hub.map(|hub| hub.url.trim().to_string()).filter(|url| !url.is_empty()),
            Err(err) => {
                eprintln!("can't read the config from {}: {err:#}", self.config_path.display());
                None
            }
        }
    }

    fn slot(&self, member: &str) -> Slot {
        self.sessions.lock().unwrap().entry(member.to_string()).or_default().clone()
    }
}

/// A connection for `linked`, with the token it holds (no password: the token decides).
async fn connect(url: &str, linked: &Linked) -> anyhow::Result<ServerConnection> {
    let handshake = ServerConnection::handshake(url, &linked.device_id, DEVICE_NAME, "", Some(linked.device_token.clone()), Vec::new());
    let (conn, _) = tokio::time::timeout(CONNECT_TIMEOUT, handshake).await.context("the hub didn't answer in time")??;
    Ok(conn)
}

/// What a message from the hub means for the turn waiting on `conversation_id`: its answer, or nothing
/// (a `Pong`, a list changing, an answer to another conversation).
fn classify(message: ServerMessage, conversation_id: &str) -> Option<MemberReply> {
    let ours = |id: &Option<String>| id.as_deref().is_none_or(|id| id == conversation_id);
    match message {
        ServerMessage::ChatResponse { content, attachments, conversation_id: id, .. } if ours(&id) => Some(MemberReply::Text { content, attachments }),
        ServerMessage::ChatError { message, conversation_id: id, .. } if ours(&id) => Some(MemberReply::Failed(message)),
        // The hub drops a device the owner revoked, mid-connection, with this.
        ServerMessage::AuthError { .. } => Some(MemberReply::Failed(NOT_LINKED_REPLY.to_string())),
        _ => None,
    }
}

async fn turn(conn: &mut ServerConnection, conversation_id: &str, text: &str) -> Result<MemberReply, Lost> {
    let chat = ClientMessage::Chat { message: text.to_string(), conversation_id: Some(conversation_id.to_string()), attachments: Vec::new(), agent_id: None, project_id: None, workdir: None };
    conn.send(&chat).await.map_err(|_| Lost::Dropped)?;
    let answer = async {
        loop {
            match conn.recv().await {
                Ok(Some(message)) => {
                    if let Some(reply) = classify(message, conversation_id) {
                        return Ok(reply);
                    }
                }
                Ok(None) | Err(_) => return Err(Lost::Dropped),
            }
        }
    };
    tokio::time::timeout(TURN_TIMEOUT, answer).await.map_err(|_| Lost::TimedOut)?
}

#[async_trait]
impl MemberChat for HubMemberChat {
    async fn ask(&self, member: &str, conversation_id: &str, text: &str) -> MemberReply {
        let Some(url) = self.hub_url() else { return MemberReply::Failed(NO_HUB_REPLY.to_string()) };
        let linked = match HubTokens::beside(&self.config_path).get(member) {
            Ok(Some(linked)) => linked,
            Ok(None) => return MemberReply::Failed(NOT_LINKED_REPLY.to_string()),
            Err(err) => {
                eprintln!("can't read the hub links: {err:#}");
                return MemberReply::Failed(HUB_DOWN_REPLY.to_string());
            }
        };
        let slot = self.slot(member);
        let mut connection = slot.lock().await;
        // A connection kept from an earlier turn may have gone stale: one fresh try is fair. A fresh
        // connection that fails is the hub's answer, so it isn't tried twice (a turn costs model calls).
        for reused in [true, false] {
            let fresh = connection.is_none();
            if fresh {
                match connect(&url, &linked).await {
                    Ok(conn) => *connection = Some(conn),
                    Err(err) if err.downcast_ref::<AuthRejected>().is_some() => return MemberReply::Failed(NOT_LINKED_REPLY.to_string()),
                    Err(err) => {
                        eprintln!("can't reach the hub at {url} for {member}: {err:#}");
                        return MemberReply::Failed(HUB_DOWN_REPLY.to_string());
                    }
                }
            }
            let conn = connection.as_mut().expect("connected just above");
            match turn(conn, conversation_id, text).await {
                Ok(reply) => return reply,
                Err(Lost::TimedOut) => {
                    *connection = None;
                    eprintln!("the hub took too long to answer {member}");
                    return MemberReply::Failed(HUB_DOWN_REPLY.to_string());
                }
                Err(Lost::Dropped) => {
                    *connection = None;
                    if fresh || !reused {
                        eprintln!("the hub dropped the connection while answering {member}");
                        return MemberReply::Failed(HUB_DOWN_REPLY.to_string());
                    }
                }
            }
        }
        MemberReply::Failed(HUB_DOWN_REPLY.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static NEXT: AtomicU32 = AtomicU32::new(0);

    fn scratch() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("warden-bot-hub-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::SeqCst)));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("config.toml")
    }

    #[test]
    fn a_chat_keeps_its_conversation_under_an_id_the_hub_accepts() {
        assert_eq!(conversation_id("telegram", "42"), "telegram-42");
        assert_eq!(conversation_id("whatsapp", "5511999999999@s.whatsapp.net"), "whatsapp-5511999999999_s_whatsapp_net");
        assert_eq!(conversation_id("whatsapp", "5511999999999@lid"), "whatsapp-5511999999999_lid");
        let long = conversation_id("whatsapp", &"9".repeat(200));
        assert_eq!(long.len(), 64);
        assert!(long.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    }

    #[test]
    fn links_are_kept_per_member_and_removed_one_at_a_time() {
        let config = scratch();
        let tokens = HubTokens::beside(&config);
        assert_eq!(tokens.get("ana").unwrap(), None);
        assert!(tokens.linked().unwrap().is_empty());
        tokens.set("ana", Linked { device_id: "warden-bot-ana".into(), device_token: "t1".into() }).unwrap();
        tokens.set("bia", Linked { device_id: "warden-bot-bia".into(), device_token: "t2".into() }).unwrap();
        assert_eq!(tokens.get("ana").unwrap().unwrap().device_token, "t1");
        assert_eq!(tokens.linked().unwrap(), ["ana", "bia"]);
        assert!(tokens.remove("ana").unwrap());
        assert!(!tokens.remove("ana").unwrap(), "nothing left to remove");
        assert_eq!(tokens.linked().unwrap(), ["bia"]);
    }

    #[test]
    fn the_members_a_chat_may_speak_as_are_listed_with_whether_the_bots_are_linked_to_them() {
        let config = scratch();
        assert!(bot_members(&config).unwrap().is_empty(), "no file, no members");
        let mut file = crate::FileConfig::default();
        crate::users::add_user(&mut file, "ana", "Ana", "provisional-pass").unwrap();
        crate::users::add_user(&mut file, "bia", "Bia", "provisional-pass").unwrap();
        crate::save_config(&config, &file).unwrap();
        let members = |config: &Path| bot_members(config).unwrap().into_iter().map(|m| (m.id, m.name, m.linked)).collect::<Vec<_>>();
        assert_eq!(members(&config), [("ana".to_string(), "Ana".to_string(), false), ("bia".to_string(), "Bia".to_string(), false)]);

        HubTokens::beside(&config).set("bia", Linked { device_id: "warden-bot-bia".into(), device_token: "t".into() }).unwrap();
        // A link to someone who isn't (or is no longer) a member lists nobody extra.
        HubTokens::beside(&config).set("ghost", Linked { device_id: "warden-bot-ghost".into(), device_token: "t".into() }).unwrap();
        assert_eq!(members(&config), [("ana".to_string(), "Ana".to_string(), false), ("bia".to_string(), "Bia".to_string(), true)]);
    }

    #[cfg(unix)]
    #[test]
    fn the_token_file_is_for_the_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let config = scratch();
        let tokens = HubTokens::beside(&config);
        tokens.set("ana", Linked { device_id: "d".into(), device_token: "secret".into() }).unwrap();
        let mode = std::fs::metadata(config.with_file_name("bot_hub.json")).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn an_answer_of_the_waiting_turn_ends_it_and_other_messages_do_not() {
        let response = |id: Option<&str>| ServerMessage::ChatResponse { content: "hi".into(), usage: None, attachments: Vec::new(), conversation_id: id.map(String::from), fallbacks: Vec::new() };
        assert_eq!(classify(response(Some("telegram-1")), "telegram-1"), Some(MemberReply::Text { content: "hi".into(), attachments: Vec::new() }));
        assert!(classify(response(None), "telegram-1").is_some(), "an answer that names no conversation is ours");
        assert_eq!(classify(response(Some("other")), "telegram-1"), None, "a late answer to another conversation");
        assert_eq!(classify(ServerMessage::Pong { nonce: 1 }, "telegram-1"), None);
        let error = ServerMessage::ChatError { message: warden_core::memory::LOCKED_MESSAGE.into(), conversation_id: Some("telegram-1".into()), spend_limit_id: None };
        assert_eq!(classify(error, "telegram-1"), Some(MemberReply::Failed(warden_core::memory::LOCKED_MESSAGE.into())), "the hub's own words reach the chat");
        assert_eq!(classify(ServerMessage::AuthError { reason: "revoked".into() }, "telegram-1"), Some(MemberReply::Failed(NOT_LINKED_REPLY.into())));
    }

    #[tokio::test]
    async fn a_member_that_was_never_linked_or_a_missing_hub_is_told_so_not_answered_by_the_owner() {
        let config = scratch();
        std::fs::write(&config, "").unwrap();
        let chat = HubMemberChat::new(config.clone());
        assert_eq!(chat.ask("ana", "telegram-1", "hi").await, MemberReply::Failed(NO_HUB_REPLY.into()), "no [bot_hub]");
        std::fs::write(&config, "[bot_hub]\nurl = \"ws://127.0.0.1:1\"\n").unwrap();
        assert_eq!(chat.ask("ana", "telegram-1", "hi").await, MemberReply::Failed(NOT_LINKED_REPLY.into()), "no link");
        HubTokens::beside(&config).set("ana", Linked { device_id: "d".into(), device_token: "t".into() }).unwrap();
        assert_eq!(chat.ask("ana", "telegram-1", "hi").await, MemberReply::Failed(HUB_DOWN_REPLY.into()), "nothing listening there");
    }
}
