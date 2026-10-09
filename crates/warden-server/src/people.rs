//! P84 — who is on a connection, and what that changes. The root is whoever paired with the hub's
//! pairing key; a member paired with their own username and password (`[[users]]`,
//! `warden_bootstrap::users`). A member gets their own vault and conversations, the agents they own
//! or the root shared with them, the tools the root allows them (fatia 2), and none of the hub's
//! administration.
//!
//! Pure functions over the orchestrator, the messages and the conversations directory, kept out of
//! `server.rs` so they're testable without a socket — same split as `conversations.rs`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use warden_bootstrap::member_crypto::{self, DirState};
use warden_bootstrap::users::{agent_visible_to, root_conversations_dir, user_conversations_dir, user_generated_path, user_vault_path, UserConfig, UserRole, ROOT_ID};
use warden_bootstrap::{load_conversation, save_conversation, AgentConfig, Conversation, FileConfig};
use warden_core::memory::{Mount, Vault};
use warden_core::orchestrator::Orchestrator;
use warden_core::spend::SpendContext;
use warden_bootstrap::recovery::RecoveryPolicy;
use warden_server_protocol::protocol::{AgentSettingsDto, RecoveryEventDto, UserInfoDto};
use warden_server_protocol::{ClientMessage, ServerMessage};

use crate::conversations::is_valid_id;

/// A member's own things on the hub.
#[derive(Clone)]
pub struct MemberSpace {
    pub id: String,
    pub name: String,
    pub vault: Arc<Vault>,
    pub generated: PathBuf,
    pub conversations: PathBuf,
}

impl MemberSpace {
    /// The same member with their vault opened again from the folder's current state — after their
    /// data key was created or opened (a connection's first vault was plain, or locked).
    pub fn reopened(&self, users_dir: &Path, conversations_root: &Path) -> Self {
        let user = UserConfig { id: self.id.clone(), name: self.name.clone(), role: UserRole::Member, password_hash: String::new(), must_change_password: false, tools: None, key: None, key_needs_recovery: false, recoveries: Vec::new(), truthid: None, invite: None, learning_opt_out: false, learning_provider: None, workdirs: Vec::new(), node_workdirs: Vec::new() };
        Self::new(&user, users_dir, conversations_root)
    }

    /// `users_dir` holds every member's folder (`warden_bootstrap::users::default_users_dir`).
    pub fn new(user: &UserConfig, users_dir: &Path, conversations_root: &Path) -> Self {
        let vault_path = user_vault_path(users_dir, &user.id);
        // P84 fatia 4: an encrypted member's vault opens with the key the hub holds, and is locked
        // (refuses everything) when it doesn't — never a plain vault beside encrypted files.
        let vault = match member_crypto::dir_state(&users_dir.join(&user.id)) {
            DirState::Plain => Vault::new(vault_path),
            DirState::Unlocked(cipher) => Vault::new_encrypted(vault_path, cipher),
            DirState::Locked => Vault::new_locked(vault_path),
        };
        Self {
            id: user.id.clone(),
            name: user.name.clone(),
            vault: Arc::new(vault),
            generated: user_generated_path(users_dir, &user.id),
            conversations: user_conversations_dir(conversations_root, &user.id),
        }
    }
}

/// One `Vault` per shared folder of the owner's (P84 fatia 3), kept for the hub's life so a
/// folder's semantic model loads once, not once per turn.
pub type SpaceVaults = Arc<Mutex<HashMap<PathBuf, Arc<Vault>>>>;

/// Shows member `member`'s shared spaces inside their vault, as `config` says right now —
/// `owner_vault` is the hub's (the owner's) vault root. Called before each turn and each vault
/// request, so a space shared or taken away counts from the next one.
pub fn mount_member_spaces(member: &MemberSpace, config: &FileConfig, owner_vault: &Path, cache: &SpaceVaults) {
    let mut vaults = cache.lock().unwrap_or_else(|e| e.into_inner());
    let mounts = warden_bootstrap::users::spaces_for(&config.spaces, &member.id)
        .into_iter()
        .map(|(space, writable)| {
            let folder = owner_vault.join(&space.folder);
            let vault = vaults.entry(folder.clone()).or_insert_with(|| Arc::new(Vault::new(folder))).clone();
            Mount { prefix: space.id.clone(), vault, writable }
        })
        .collect();
    member.vault.set_mounts(mounts);
}

/// Who a connection speaks for.
#[derive(Clone)]
pub enum Person {
    Root,
    Member(MemberSpace),
}

/// `agents`: the config's, for the member's own agents' ids.
pub fn user_info(user: &UserConfig, agents: &[AgentConfig], workspace_policy: RecoveryPolicy) -> UserInfoDto {
    let member_policy = user.key.as_ref().map(|wraps| wraps.policy);
    UserInfoDto {
        member_policy: member_policy.map(|p| p.as_str().to_string()).unwrap_or_default(),
        // Their data isn't following the workspace's policy yet (a weaker one waits for their yes).
        policy_pending: member_policy.is_some_and(|p| p != workspace_policy),
        recovery_policy: String::new(),
        learning_opt_out: user.learning_opt_out,
        learning_provider: user.learning_provider.clone(),
        learning_enabled: false,
        recoveries: user.recoveries.iter().map(|e| RecoveryEventDto { at_ms: e.at_ms, kind: e.kind.as_str().to_string(), seen: e.seen }).collect(),
        id: user.id.clone(),
        name: user.name.clone(),
        role: match user.role {
            UserRole::Member => "member".to_string(),
        },
        must_change_password: user.must_change_password,
        tools: user.tools.clone(),
        agents: agents.iter().filter(|a| a.owner.as_deref() == Some(user.id.as_str())).map(|a| a.id.clone()).collect(),
        encrypted: user.key.is_some(),
        needs_recovery: user.key_needs_recovery,
        locked: false,
        truthid: user.truthid.as_ref().map(|link| link.username.clone()).unwrap_or_default(),
        invite_open: user.invite.as_ref().is_some_and(|invite| invite.expires_at > unix_now()),
        workdirs: user.workdirs.clone(),
        node_workdirs: user.node_workdirs.iter().map(|f| warden_server_protocol::protocol::NodeFolderDto { node: f.node.clone(), path: f.path.clone() }).collect(),
    }
}

/// Seconds since the epoch.
pub fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// The tools member `id` may use on this hub right now (`warden_bootstrap::users::member_tools`
/// over `base`'s), or none when they're no longer in `config`.
pub fn tools_for(base: &Orchestrator, config: &FileConfig, id: &str) -> Vec<String> {
    let available: Vec<String> = base.tools().iter().map(|t| t.spec().name).collect();
    config.users.iter().find(|u| u.id == id).map(|user| warden_bootstrap::users::member_tools(user, &available)).unwrap_or_default()
}

/// The orchestrator a member's turn runs on: `base` (already scoped to the chosen agent) with only
/// `tools` (what the owner allows them, `tools_for`), reading and writing the member's vault,
/// saving files in their folder, and spending as them on `channel` (`server`, `api`) as `user`.
pub fn member_orchestrator(base: &Orchestrator, member: &MemberSpace, tools: &[String], channel: &str, user: &str) -> Orchestrator {
    base.with_allowed_tools(Some(tools))
        .with_vault(member.vault.clone())
        .with_media_root(member.generated.clone())
        // P104: `search_history` reads their conversations, never someone else's.
        .with_conversations_dir(&member.conversations)
        .with_spend_context(SpendContext::new(channel).with_user(user).with_person(member.id.clone()))
}

const ROOT_ONLY: &str = "only the workspace's owner can do this";

/// The reply that turns a member away from a request that's the root's — administration and
/// hub-wide views — or `None` when a member may make it.
pub fn member_refusal(message: &ClientMessage) -> Option<ServerMessage> {
    let message_text = ROOT_ONLY.to_string();
    Some(match message {
        ClientMessage::SaveSettings { request_id, .. } | ClientMessage::EditAgentOrg { request_id, .. } => {
            ServerMessage::SettingsError { request_id: *request_id, message: message_text, conflict: false, auth_rejected: true }
        }
        ClientMessage::ListDevices { request_id } | ClientMessage::SetDeviceStatus { request_id, .. } => {
            ServerMessage::DeviceError { request_id: *request_id, message: message_text, auth_rejected: true }
        }
        ClientMessage::ListNodes { request_id } | ClientMessage::SetNodeAccess { request_id, .. } => {
            ServerMessage::NodeError { request_id: *request_id, message: message_text, auth_rejected: true }
        }
        ClientMessage::ListTasks { request_id }
        | ClientMessage::SaveTask { request_id, .. }
        | ClientMessage::SetTaskEnabled { request_id, .. }
        | ClientMessage::DeleteTask { request_id, .. }
        | ClientMessage::RunTask { request_id, .. } => ServerMessage::TaskError { request_id: *request_id, message: message_text, auth_rejected: true },
        ClientMessage::ListWebhooks { request_id }
        | ClientMessage::SaveWebhook { request_id, .. }
        | ClientMessage::SetWebhookEnabled { request_id, .. }
        | ClientMessage::DeleteWebhook { request_id, .. }
        | ClientMessage::CreateWebhookCredential { request_id, .. }
        | ClientMessage::RevokeWebhookCredential { request_id, .. } => ServerMessage::WebhookError { request_id: *request_id, message: message_text, auth_rejected: true },
        ClientMessage::RequestSyncStatus { request_id } | ClientMessage::SyncAction { request_id, .. } => {
            ServerMessage::SyncError { request_id: *request_id, message: message_text, auth_rejected: true }
        }
        ClientMessage::RequestUsage { request_id, .. } | ClientMessage::ExtendLimit { request_id, .. } => {
            ServerMessage::UsageError { request_id: *request_id, message: message_text }
        }
        ClientMessage::ListUsers { request_id }
        | ClientMessage::SaveUser { request_id, .. }
        | ClientMessage::ResetPassword { request_id, .. }
        | ClientMessage::RemoveUser { request_id, .. }
        | ClientMessage::SetUserTools { request_id, .. }
        | ClientMessage::SetUserLearningProvider { request_id, .. }
        | ClientMessage::SetUserWorkdirs { request_id, .. }
        | ClientMessage::ListBotPairings { request_id }
        | ClientMessage::ResolveBotPairing { request_id, .. }
        | ClientMessage::TestProvider { request_id, .. }
        | ClientMessage::SetRecoveryPolicy { request_id, .. }
        | ClientMessage::RecoverMember { request_id, .. }
        | ClientMessage::RestoreUser { request_id, .. }
        | ClientMessage::CreateInvite { request_id, .. }
        | ClientMessage::UnlinkTruthId { request_id, .. }
        | ClientMessage::SaveSpace { request_id, .. }
        | ClientMessage::DeleteSpace { request_id, .. } => {
            ServerMessage::UserError { request_id: *request_id, message: message_text, auth_rejected: true }
        }
        ClientMessage::CallDeviceTool { call_id, .. } => ServerMessage::DeviceToolError { call_id: *call_id, message: message_text },
        _ => return None,
    })
}

const CHANGE_PASSWORD_FIRST: &str = "choose your own password first — you're still on the one you were given";

/// While a member is on a provisional password, everything but changing it (and keeping the
/// connection up) is turned away with this reply. `None` lets the message through.
pub fn password_gate(message: &ClientMessage) -> Option<ServerMessage> {
    let text = CHANGE_PASSWORD_FIRST.to_string();
    Some(match message {
        ClientMessage::ChangePassword { .. } | ClientMessage::Ping { .. } | ClientMessage::Goodbye { .. } | ClientMessage::Hello { .. } => return None,
        ClientMessage::Chat { conversation_id, .. } => ServerMessage::ChatError { message: text, conversation_id: conversation_id.clone(), spend_limit_id: None },
        ClientMessage::RequestHistory { request_id, .. } => ServerMessage::HistoryError { request_id: *request_id, message: text },
        ClientMessage::ListConversations { request_id }
        | ClientMessage::RenameConversation { request_id, .. }
        | ClientMessage::DeleteConversation { request_id, .. }
        | ClientMessage::MoveConversation { request_id, .. } => {
            ServerMessage::ConversationError { request_id: *request_id, message: text }
        }
        ClientMessage::ListSkills { request_id } | ClientMessage::SaveSkill { request_id, .. } | ClientMessage::DeleteSkill { request_id, .. } => {
            ServerMessage::SkillError { request_id: *request_id, message: text }
        }
        ClientMessage::ListProjects { request_id } | ClientMessage::SaveProject { request_id, .. } | ClientMessage::DeleteProject { request_id, .. } => {
            ServerMessage::ProjectError { request_id: *request_id, message: text }
        }
        ClientMessage::ListVaultFiles { request_id }
        | ClientMessage::ReadVaultNote { request_id, .. }
        | ClientMessage::SaveVaultNote { request_id, .. }
        | ClientMessage::DeleteVaultNote { request_id, .. }
        | ClientMessage::SearchVault { request_id, .. } => ServerMessage::VaultError { request_id: *request_id, message: text, conflict: false },
        ClientMessage::Transcribe { request_id, .. } => ServerMessage::TranscriptionError { request_id: *request_id, message: text },
        ClientMessage::RequestSettings { request_id } | ClientMessage::SaveOwnAgent { request_id, .. } | ClientMessage::DeleteOwnAgent { request_id, .. } => {
            ServerMessage::SettingsError { request_id: *request_id, message: text, conflict: false, auth_rejected: true }
        }
        ClientMessage::ListApiKeys { request_id } | ClientMessage::CreateApiKey { request_id, .. } | ClientMessage::RevokeApiKey { request_id, .. } => {
            ServerMessage::ApiKeyError { request_id: *request_id, message: text, auth_rejected: true }
        }
        ClientMessage::ListSpaces { request_id }
        | ClientMessage::RegenerateRecoveryCode { request_id, .. }
        | ClientMessage::AcceptRecoveryPolicy { request_id, .. }
        | ClientMessage::AckRecoveryNotices { request_id }
        | ClientMessage::SetLearning { request_id, .. } => {
            ServerMessage::UserError { request_id: *request_id, message: text, auth_rejected: true }
        }
        other => return member_refusal(other),
    })
}

/// What a member's `RequestSettings` shows: the agents they see (their own in full, with `owner`
/// set; the shared ones by name only) and, as `tool_names`, the tools they have — for their own
/// agents' editor. Nothing about providers, keys, limits or the owner's agents' instructions.
pub fn member_settings_view(message: ServerMessage, config: &FileConfig, member: &str, tools: Vec<String>) -> ServerMessage {
    match message {
        ServerMessage::Settings { request_id, mut settings, version, .. } => {
            settings.providers.clear();
            settings.active_provider.clear();
            settings.combos.clear();
            settings.limits = None;
            settings.default_limits.clear();
            settings.prices.clear();
            settings.default_models.clear();
            settings.notes.clear();
            settings.git_sync.remote_url.clear();
            // The owner's bots (their allow-lists, the learning model), the Telegram token's status, the
            // delegation/TruthID settings and everything that reaches the machine are the owner's: a
            // member's screen gets the empty ones (P119; `bots` was missing from this list until then).
            settings.bots = Default::default();
            settings.telegram_token = Default::default();
            settings.advanced = Default::default();
            settings.machine = Default::default();
            settings.tool_names = tools;
            settings.agents = member_agents_view(config, member);
            ServerMessage::Settings { request_id, settings, version, secrets_writable: false }
        }
        other => other,
    }
}

/// The agents member `member` sees, as their settings show them.
pub fn member_agents_view(config: &FileConfig, member: &str) -> Vec<AgentSettingsDto> {
    config
        .agents
        .iter()
        .filter(|a| agent_visible_to(a, Some(member)))
        .map(|a| {
            let mine = a.owner.as_deref() == Some(member);
            AgentSettingsDto {
                original_id: None,
                id: a.id.clone(),
                persona: if mine { a.persona.clone() } else { String::new() },
                provider_id: String::new(),
                can_delegate_to_agents: false,
                can_manage_agents: false,
                can_message_agents: false,
                can_manage_tasks: false,
                allowed_tools: if mine { a.allowed_tools.clone() } else { None },
                autonomy: a.autonomy,
                approval_required: a.approval_required.iter().map(|c| c.as_str().to_string()).collect(),
                role: None,
                reports_to: None,
                shared_with: Vec::new(),
                owner: mine.then(|| member.to_string()),
                delegation_models: Vec::new(),
            }
        })
        .collect()
}

/// Moves every device's conversations (P78: `<root>/<device>/`, and the older `<root>/<device>.json`)
/// into the root's one directory (`<root>/root/`), since conversations are a person's now, not a
/// device's. Idempotent; run once when the hub starts. An id that's already taken there (every
/// device has a `default`) is kept as `<id>-<device>`, so nothing is lost. Returns how many moved.
pub fn migrate_device_conversations(conversations_root: &Path) -> anyhow::Result<usize> {
    let target = root_conversations_dir(conversations_root);
    let entries = match std::fs::read_dir(conversations_root) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(err) => return Err(err.into()),
    };
    let mut moved = 0;
    for entry in entries {
        let path = entry?.path();
        let Some(name) = path.file_name().map(|n| n.to_string_lossy().to_string()) else { continue };
        if path.is_dir() {
            if name == ROOT_ID || name == "users" {
                continue;
            }
            for file in std::fs::read_dir(&path)? {
                let file = file?.path();
                if file.extension().is_some_and(|e| e == "json") {
                    let id = file.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                    move_conversation(&path, &id, &file, &name, &target)?;
                    moved += 1;
                }
            }
            // Only if nothing else was left in it.
            let _ = std::fs::remove_dir(&path);
        } else if path.extension().is_some_and(|e| e == "json") {
            // Before P78: the device's single conversation, `<device>.json`, its `default`.
            let device = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            move_conversation(conversations_root, &device, &path, &device, &target)?;
            moved += 1;
        }
    }
    Ok(moved)
}

/// `file` is conversation `id` in `dir`, from `device`. Keeps the id if it's free in `target`.
fn move_conversation(dir: &Path, id: &str, file: &Path, device: &str, target: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(target)?;
    // A pre-P78 single file is the device's `default`.
    let wanted = if dir.join(format!("{device}.json")) == file { "default".to_string() } else { id.to_string() };
    let new_id = free_id(target, &wanted, device);
    match load_conversation(dir, id) {
        Ok(Some(conversation)) => {
            save_conversation(target, &Conversation { id: new_id, ..conversation })?;
            std::fs::remove_file(file)?;
        }
        // Unreadable: moved as it is, so it keeps reporting its parse error instead of vanishing.
        _ => std::fs::rename(file, target.join(format!("{new_id}.json")))?,
    }
    Ok(())
}

fn free_id(target: &Path, wanted: &str, device: &str) -> String {
    let taken = |id: &str| target.join(format!("{id}.json")).exists();
    if is_valid_id(wanted) && !taken(wanted) {
        return wanted.to_string();
    }
    let suffix: String = device.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').take(20).collect();
    let base: String = wanted.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').take(40).collect();
    let mut candidate = format!("{base}-{suffix}");
    let mut n = 2;
    while !is_valid_id(&candidate) || taken(&candidate) {
        candidate = format!("{base}-{suffix}-{n}");
        n += 1;
    }
    candidate
}

#[cfg(test)]
mod tests {
    use super::*;
    use warden_bootstrap::{list_conversations, ChatRole, ConversationMessage};

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("warden-people-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn conversation(id: &str, text: &str) -> Conversation {
        Conversation {
            id: id.into(),
            title: text.into(),
            created_at: 1,
            updated_at: 1,
            messages: vec![ConversationMessage {
                id: "m".into(),
                role: ChatRole::User,
                content: text.into(),
                created_at: 1,
                usage: None,
                answered_by: None,
                attachments: Vec::new(),
                generated_files: Vec::new(), tools_used: Vec::new(),
            }],
            agent_id: None,
            provider_id: None,
            project_id: None,
            engine_session_id: None,
            workdir: None,
            parent: None,
        }
    }

    #[test]
    fn every_devices_conversations_become_the_roots_without_losing_one() {
        let root = temp_dir();
        save_conversation(&root.join("phone"), &conversation("default", "from the phone")).unwrap();
        save_conversation(&root.join("phone"), &conversation("abc", "phone abc")).unwrap();
        save_conversation(&root.join("browser"), &conversation("default", "from the browser")).unwrap();
        save_conversation(&root, &conversation("old-tablet", "the tablet, before P78")).unwrap();
        std::fs::create_dir_all(root.join("users/ana")).unwrap();
        std::fs::create_dir_all(root.join("broken")).unwrap();
        std::fs::write(root.join("broken/default.json"), "not json").unwrap();

        assert_eq!(migrate_device_conversations(&root).unwrap(), 5);
        // Unreadable, but moved rather than lost.
        assert!(std::fs::read_dir(root.join("root")).unwrap().any(|f| std::fs::read_to_string(f.unwrap().path()).unwrap() == "not json"));
        let moved = list_conversations(&root.join("root")).unwrap();
        let mut texts: Vec<String> = moved.iter().map(|c| c.messages[0].content.clone()).collect();
        texts.sort();
        assert_eq!(texts, ["from the browser", "from the phone", "phone abc", "the tablet, before P78"]);
        let ids: Vec<&str> = moved.iter().map(|c| c.id.as_str()).collect();
        assert!(ids.contains(&"abc"), "{ids:?}");
        assert_eq!(ids.iter().filter(|id| id.starts_with("default")).count(), 3, "{ids:?}");
        assert!(!root.join("phone").exists() && !root.join("old-tablet.json").exists());
        assert!(root.join("users/ana").exists(), "members' folders are left alone");
        assert_eq!(migrate_device_conversations(&root).unwrap(), 0, "once is enough");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn members_only_get_the_tools_that_stay_in_their_space_by_default() {
        use warden_bootstrap::users::{default_member_tool, NEVER_FOR_MEMBERS};
        for ok in ["read_file", "write_file", "use_skill", "manage_skill", "delegate_task", "tavily-search", "generate_document"] {
            assert!(default_member_tool(ok), "{ok}");
        }
        for no in ["shell", "ssh_exec", "list_nodes", "node_shell", "home-pc__query", "manage_agents", "manage_tasks", "message_agent", "usage_stats", "delegate_to_agent", "github__create_issue"] {
            assert!(!default_member_tool(no), "{no}");
        }
        for never in ["delegate_to_agent", "message_agent", "manage_agents", "manage_tasks", "usage_stats"] {
            assert!(NEVER_FOR_MEMBERS.contains(&never), "{never}");
        }
    }

    #[test]
    fn a_member_is_turned_away_from_administration_but_not_from_chatting() {
        assert!(matches!(member_refusal(&ClientMessage::ListDevices { request_id: 1 }), Some(ServerMessage::DeviceError { auth_rejected: true, .. })));
        assert!(matches!(member_refusal(&ClientMessage::RequestUsage { request_id: 2, tz_offset_minutes: 0 }), Some(ServerMessage::UsageError { .. })));
        assert!(member_refusal(&ClientMessage::ListConversations { request_id: 3 }).is_none());
        assert!(member_refusal(&ClientMessage::RequestSettings { request_id: 4 }).is_none());
        // On a provisional password, only changing it goes through.
        assert!(matches!(password_gate(&ClientMessage::ListConversations { request_id: 5 }), Some(ServerMessage::ConversationError { .. })));
        assert!(password_gate(&ClientMessage::ChangePassword { request_id: 6, old_password: "a".into(), new_password: "b".into(), recovery_code: None }).is_none());
        assert!(matches!(password_gate(&ClientMessage::ListDevices { request_id: 7 }), Some(ServerMessage::DeviceError { .. })));
    }
}
