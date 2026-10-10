//! The people of the workspace (P84, fatia 1). The root is whoever holds the hub's pairing key, as
//! before; `[[users]]` in `config.toml` lists the **members** the root created. A member pairs a
//! device with their username and password instead of the key, and gets their own vault and
//! conversations. No `[[users]]` at all is the single-person Warden, unchanged.
//!
//! Passwords are only ever stored as an Argon2id hash (PHC string). The file syncs between the
//! root's machines, so the hashes travel with it — the root's choice (Sessão 110).

use std::path::{Path, PathBuf};

use argon2::password_hash::rand_core::OsRng;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use rand::Rng;
use serde::{Deserialize, Serialize};

use crate::member_crypto::{self, MemberKey};
use crate::recovery::{self, RecoveryPolicy};
use crate::{AgentConfig, FileConfig};

/// Kept for the root's own directories (`conversations-server/root`); no member can take it.
pub const ROOT_ID: &str = "root";
pub const MAX_USER_ID_LEN: usize = 32;
pub const MIN_PASSWORD_LEN: usize = 8;
const TEMP_PASSWORD_LEN: usize = 14;

#[derive(Deserialize, Serialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum UserRole {
    /// Uses the workspace's agents with their own memory; no administration.
    #[default]
    Member,
}

/// What a member may do with the workspace's one organization of agents (P120): the owner sets it per member. There is a single tree,
/// the owner's; this only says who besides the owner sees it and who changes it.
#[derive(Deserialize, Serialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum OrgAccess {
    /// Does not see the tree (what every member had before this existed).
    #[default]
    None,
    /// Sees the tree: each agent's id, position and superior, nothing of what it says or can do.
    View,
    /// Sees it and changes it: position, superior, adding and removing agents, with the same rules as the owner's edits.
    Edit,
}

impl OrgAccess {
    pub fn as_str(self) -> &'static str {
        match self {
            OrgAccess::None => "none",
            OrgAccess::View => "view",
            OrgAccess::Edit => "edit",
        }
    }

    /// `none`, `view` or `edit`; anything else is not an access.
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "none" => Some(OrgAccess::None),
            "view" => Some(OrgAccess::View),
            "edit" => Some(OrgAccess::Edit),
            _ => None,
        }
    }

    pub fn can_view(self) -> bool {
        self != OrgAccess::None
    }

    pub fn can_edit(self) -> bool {
        self == OrgAccess::Edit
    }
}

impl OrgAccess {
    fn is_none(&self) -> bool {
        *self == OrgAccess::None
    }
}

/// One member (TOML `[[users]]`).
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct UserConfig {
    /// The username: 1-32 lowercase letters, digits, `-` or `_`. Also the name of their directories.
    pub id: String,
    /// How they're shown.
    pub name: String,
    #[serde(default)]
    pub role: UserRole,
    /// Argon2id PHC string — never the password.
    pub password_hash: String,
    /// Set by the root (a new member, a reset): until they choose their own, the hub only lets them
    /// change it.
    #[serde(default)]
    pub must_change_password: bool,
    /// The tools this member may use (P84, fatia 2), by name. `None` is the safe default
    /// (`default_member_tool`); a list the owner set replaces it. A name the hub doesn't have is
    /// ignored, and `NEVER_FOR_MEMBERS` stays out whatever the list says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<String>>,
    /// The key to this member's data, wrapped (P84 fatia 4). `None`: their data isn't encrypted yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<KeyWraps>,
    /// The owner reset the password, which the password wrap can't follow: only the recovery code
    /// opens the data now, and the member gives it when they choose their new password.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub key_needs_recovery: bool,
    /// Every time the owner recovered their data with the workspace's recovery key (parte B).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recoveries: Vec<RecoveryEvent>,
    /// The TruthID identity this member linked with an invite (P84 fatia 5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truthid: Option<TruthIdLink>,
    /// An invite the owner made and nobody has used yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invite: Option<Invite>,
    /// The member turned off the assistant learning from their conversations (P104/P115). It only
    /// ever narrows `[learning] enabled`: a member can't switch on what the workspace left off.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub learning_opt_out: bool,
    /// A provider or combo id the owner picked for the assistant's learning from *this* member's conversations
    /// (P115), in place of `[learning] provider`. Set in the config file; an id the hub doesn't have falls back
    /// to the conversation's own model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub learning_provider: Option<String>,
    /// The folders of the hub's machine this member may pick as a conversation's working folder (P102), and anything
    /// inside them: absolute paths, set in the config file. Empty (the default) is none — a member never gets a folder
    /// the owner didn't name, because the folder is a place on the owner's machine. The owner has no list: every
    /// folder is theirs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workdirs: Vec<String>,
    /// The same for folders on nodes (P102 fatia 2): each entry is a folder inside what node `node` lends, and
    /// anything inside it. `path` is relative to the node's lent folder (empty is all of it), never `..`. A member
    /// gets no folder on a node the owner didn't name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub node_workdirs: Vec<NodeFolder>,
    /// Whether this member sees and changes the workspace's organization of agents (P120). Only the owner sets it.
    #[serde(default, skip_serializing_if = "OrgAccess::is_none")]
    pub org_access: OrgAccess,
}

/// A folder on a node a member may work in (`[[users]] node_workdirs`).
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NodeFolder {
    /// The node's device id (`node-<name>-<8 hex>`).
    pub node: String,
    /// Relative to the folder the node lends.
    #[serde(default)]
    pub path: String,
}

impl NodeFolder {
    /// Whether `path` (relative to node `node`'s lent folder) is this folder or inside it.
    pub fn covers(&self, node: &str, path: &str) -> bool {
        self.node == node && std::path::Path::new(path).starts_with(&self.path)
    }
}

/// A TruthID identity tied to a member. Saying who it is proves nothing by itself: signing in with
/// it will need a signature from one of that identity's devices.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TruthIdLink {
    /// The TruthID username, as the registry has it.
    pub username: String,
    /// The registry's id for that identity; what a device is later checked against.
    pub identity_id: u64,
    /// When it was linked, in seconds since the epoch.
    pub linked_at: u64,
}

/// An unused, single-use invitation to link a TruthID. Only a hash of the secret is kept.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Invite {
    pub secret_hash: String,
    /// Seconds since the epoch.
    pub expires_at: u64,
}

/// One key, wrapped twice (`member_crypto`): whoever knows the password, or the recovery code,
/// opens it. Base64 text; neither the key nor either secret is in it.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct KeyWraps {
    pub by_password: String,
    /// Opens with the recovery code alone — except under `consent`, where it holds the key already
    /// sealed to the owner, so the code only gets one to the owner's half.
    pub by_recovery: String,
    /// The recovery policy these wraps implement — the one the person accepted (parte B).
    #[serde(default, skip_serializing_if = "RecoveryPolicy::is_private")]
    pub policy: RecoveryPolicy,
    /// `company` only: the key sealed to the owner's public key, which their private key alone opens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by_escrow: Option<String>,
    /// Which of the owner's public keys the seals above used (`recovery::escrow_id`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub escrow_id: Option<String>,
}

/// The owner recovered a member's data with the workspace's recovery key (parte B): recorded, and
/// shown to the person at their next sign-in.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RecoveryEvent {
    /// When, in milliseconds since the epoch.
    pub at_ms: i64,
    /// The policy it was done under: `consent` or `company`.
    pub kind: RecoveryPolicy,
    /// The person has seen it.
    #[serde(default)]
    pub seen: bool,
}

/// Shares an agent with every member (`AgentConfig::shared_with`).
pub const EVERYONE: &str = "*";

/// A folder of the owner's vault shared with members (P84 fatia 3, TOML `[[spaces]]`). It stays the
/// owner's — synced and backed up with the rest of their vault; a member sees it inside their own
/// vault at `compartilhado/<id>/`. It is also how the owner decides what an agent may know when it
/// talks to someone else: a member's turn sees these folders and never the rest of the owner's vault.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SpaceConfig {
    /// Its name, and the folder name members see it under: 1-32 lowercase letters, digits, `-` or `_`.
    pub id: String,
    /// The folder in the owner's vault, relative to its root (`casa`, `viagens/2026`).
    pub folder: String,
    /// Who reads it: usernames, or `"*"` for everyone.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub readers: Vec<String>,
    /// Who also writes in it (writers read too).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub writers: Vec<String>,
}

fn check_space_folder(folder: &str) -> anyhow::Result<()> {
    let path = Path::new(folder);
    anyhow::ensure!(!folder.is_empty() && folder.len() <= 200 && !folder.contains('\\'), "'{folder}' is not a folder of your vault");
    for component in path.components() {
        match component {
            std::path::Component::Normal(name) if !name.to_string_lossy().starts_with('.') => {}
            _ => anyhow::bail!("'{folder}' is not a folder of your vault (no '..', no leading '/', no hidden folders)"),
        }
    }
    let first = path.components().next().map(|c| c.as_os_str().to_string_lossy().to_string()).unwrap_or_default();
    anyhow::ensure!(first != warden_core::memory::SKILLS_DIR, "skills can't be shared as a space");
    Ok(())
}

/// Creates (`original_id` = `None`) or replaces a space, with its people kept to members that exist.
pub fn save_space(config: &mut FileConfig, original_id: Option<&str>, space: SpaceConfig) -> anyhow::Result<()> {
    let id = space.id.trim().to_ascii_lowercase();
    anyhow::ensure!(is_valid_user_id(&id), "a space's name is 1-{MAX_USER_ID_LEN} lowercase letters, digits, '-' or '_'");
    let folder = space.folder.trim().trim_matches('/').to_string();
    check_space_folder(&folder)?;
    let others = config.spaces.iter().filter(|s| Some(s.id.as_str()) != original_id);
    for other in others {
        anyhow::ensure!(other.id != id, "there's already a space named '{id}'");
        anyhow::ensure!(other.folder != folder, "the folder '{folder}' is already the space '{}'", other.id);
    }
    let writers = clean_shares(space.writers, &config.users);
    let readers: Vec<String> = clean_shares(space.readers, &config.users).into_iter().filter(|r| !writers.contains(r)).collect();
    let space = SpaceConfig { id, folder, readers, writers };
    match original_id {
        Some(original) => {
            let i = config.spaces.iter().position(|s| s.id == original).ok_or_else(|| anyhow::anyhow!("no space named '{original}'"))?;
            config.spaces[i] = space;
        }
        None => config.spaces.push(space),
    }
    Ok(())
}

pub fn remove_space(config: &mut FileConfig, id: &str) -> anyhow::Result<()> {
    let i = config.spaces.iter().position(|s| s.id == id).ok_or_else(|| anyhow::anyhow!("no space named '{id}'"))?;
    config.spaces.remove(i);
    Ok(())
}

/// The spaces member `user` sees, each with whether they may write in it.
pub fn spaces_for<'a>(spaces: &'a [SpaceConfig], user: &str) -> Vec<(&'a SpaceConfig, bool)> {
    let listed = |list: &[String]| list.iter().any(|p| p == EVERYONE || p == user);
    spaces
        .iter()
        .filter_map(|s| {
            if listed(&s.writers) {
                Some((s, true))
            } else if listed(&s.readers) {
                Some((s, false))
            } else {
                None
            }
        })
        .collect()
}

/// Tools a member never gets, whatever the owner lists: each reaches past the member's own space in
/// a way `Orchestrator::with_vault` can't close — other agents' orchestrators and conversations
/// (`delegate_to_agent`, `message_agent`), the owner's agents and tasks (`manage_agents`,
/// `manage_tasks`), the whole hub's spending (`usage_stats`), or the owner's code projects and their folders on this
/// machine (`code_task`).
pub const NEVER_FOR_MEMBERS: &[&str] = &["delegate_to_agent", "message_agent", "manage_agents", "manage_tasks", "usage_stats", "code_task"];

/// The tools a member has when the owner never set a list: their own vault's files and skills, a
/// sub-agent (rebound to their vault too), background jobs, their spending, documents, and web
/// search. Everything else reaches what's the owner's (the hub's shell, SSH hosts, nodes, the MCP
/// servers set up with the owner's accounts) and waits for the owner to grant it.
pub fn default_member_tool(name: &str) -> bool {
    matches!(name, "read_file" | "write_file" | "use_skill" | "read_skill_file" | "manage_skill" | "delegate_task" | "jobs" | "budget" | "generate_document" | "search_history")
        || name.starts_with("tavily")
}

/// Which of `available` (the hub's tools) `user` may use.
pub fn member_tools(user: &UserConfig, available: &[String]) -> Vec<String> {
    available
        .iter()
        .filter(|name| !NEVER_FOR_MEMBERS.contains(&name.as_str()))
        .filter(|name| match &user.tools {
            Some(list) => list.iter().any(|t| t == *name),
            None => default_member_tool(name),
        })
        .cloned()
        .collect()
}

/// Whether `viewer` (`None` = the owner, `Some(id)` = that member) sees and may talk to `agent`:
/// the owner their own agents, a member theirs plus those the owner shared with them.
pub fn agent_visible_to(agent: &AgentConfig, viewer: Option<&str>) -> bool {
    match (viewer, agent.owner.as_deref()) {
        (None, owner) => owner.is_none(),
        (Some(me), Some(owner)) => owner == me,
        (Some(me), None) => agent.shared_with.iter().any(|s| s == EVERYONE || s == me),
    }
}

pub fn is_valid_user_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_USER_ID_LEN && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

pub fn check_users(users: &[UserConfig]) -> anyhow::Result<()> {
    for (i, user) in users.iter().enumerate() {
        anyhow::ensure!(
            is_valid_user_id(&user.id),
            "user '{}': a username is 1-{MAX_USER_ID_LEN} lowercase letters, digits, '-' or '_'",
            user.id
        );
        anyhow::ensure!(user.id != ROOT_ID, "'{ROOT_ID}' is reserved for the workspace's owner");
        anyhow::ensure!(!user.name.trim().is_empty(), "user '{}' needs a name", user.id);
        anyhow::ensure!(PasswordHash::new(&user.password_hash).is_ok(), "user '{}' has no valid password hash", user.id);
        anyhow::ensure!(!users[..i].iter().any(|u| u.id == user.id), "there's already a user named '{}'", user.id);
    }
    Ok(())
}

pub fn hash_password(password: &str) -> anyhow::Result<String> {
    anyhow::ensure!(password.chars().count() >= MIN_PASSWORD_LEN, "a password has at least {MIN_PASSWORD_LEN} characters");
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default().hash_password(password.as_bytes(), &salt).map(|h| h.to_string()).map_err(|e| anyhow::anyhow!("failed to hash the password: {e}"))
}

/// Whether `password` matches `hash`. A malformed hash matches nothing.
pub fn verify_password(hash: &str, password: &str) -> bool {
    PasswordHash::new(hash).is_ok_and(|parsed| Argon2::default().verify_password(password.as_bytes(), &parsed).is_ok())
}

/// A provisional password for the root to hand over: letters and digits without look-alikes.
pub fn generate_temp_password() -> String {
    const CHARS: &[u8] = b"abcdefghjkmnpqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut rng = rand::rngs::OsRng;
    (0..TEMP_PASSWORD_LEN).map(|_| CHARS[rng.gen_range(0..CHARS.len())] as char).collect()
}

/// The member `username` if `password` is theirs. Same answer (`None`) for an unknown name and a
/// wrong password, and the hash is checked either way, so the timing doesn't tell them apart.
pub fn authenticate_user<'a>(users: &'a [UserConfig], username: &str, password: &str) -> Option<&'a UserConfig> {
    // A fixed, valid hash of nothing anyone knows, for the unknown-name path.
    const DECOY: &str = "$argon2id$v=19$m=19456,t=2,p=1$c29tZXNhbHRzb21lc2FsdA$wJ6yOkq2dh6lV9Z6Hn6oS5o8f3sYkQ3sVJ0b1k2Wm0Y";
    let username = username.trim().to_ascii_lowercase();
    match users.iter().find(|u| u.id == username) {
        Some(user) => verify_password(&user.password_hash, password).then_some(user),
        None => {
            let _ = verify_password(DECOY, password);
            None
        }
    }
}

/// Adds a member with a provisional password they must change on first use.
pub fn add_user(config: &mut FileConfig, id: &str, name: &str, temp_password: &str) -> anyhow::Result<()> {
    let user = UserConfig {
        id: id.trim().to_ascii_lowercase(),
        name: name.trim().to_string(),
        role: UserRole::Member,
        password_hash: hash_password(temp_password)?,
        must_change_password: true,
        tools: None,
        key: None,
        key_needs_recovery: false,
        recoveries: Vec::new(),
        truthid: None,
        invite: None,
        learning_opt_out: false,
        learning_provider: None,
        workdirs: Vec::new(),
        node_workdirs: Vec::new(),
        org_access: OrgAccess::None,
    };
    let mut users = config.users.clone();
    anyhow::ensure!(!users.iter().any(|u| u.id == user.id), "there's already a user named '{}'", user.id);
    // Their encrypted data is still in that name's folder: a new person must not inherit it.
    anyhow::ensure!(
        !config.removed_users.iter().any(|u| u.id == user.id),
        "'{}' is a removed member whose data is kept — `warden-server users restore {}` brings them back, `users purge {}` deletes their data for good",
        user.id,
        user.id,
        user.id
    );
    users.push(user);
    check_users(&users)?;
    config.users = users;
    Ok(())
}

pub fn rename_user(config: &mut FileConfig, id: &str, name: &str) -> anyhow::Result<()> {
    anyhow::ensure!(!name.trim().is_empty(), "a user needs a name");
    find_mut(config, id)?.name = name.trim().to_string();
    Ok(())
}

/// How long an invite works.
pub const INVITE_TTL_SECS: u64 = 7 * 24 * 60 * 60;
const INVITE_SECRET_LEN: usize = 20;

/// The owner invites `id` to link a TruthID. Returns the code to hand over — `<username>:<secret>`,
/// shown once, since only a hash of the secret is kept. A new invite replaces an older unused one.
pub fn create_invite(config: &mut FileConfig, id: &str, now: u64) -> anyhow::Result<String> {
    const CHARS: &[u8] = b"abcdefghjkmnpqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut rng = rand::rngs::OsRng;
    let secret: String = (0..INVITE_SECRET_LEN).map(|_| CHARS[rng.gen_range(0..CHARS.len())] as char).collect();
    let secret_hash = hash_password(&secret)?;
    let user = find_mut(config, id)?;
    user.invite = Some(Invite { secret_hash, expires_at: now + INVITE_TTL_SECS });
    Ok(format!("{id}:{secret}"))
}

/// Splits an invite code into the username and the secret.
pub fn split_invite(code: &str) -> Option<(&str, &str)> {
    code.trim().split_once(':').filter(|(id, secret)| is_valid_user_id(id) && !secret.is_empty())
}

/// Whether `code` is a live invite. One answer for every way it can be wrong, so a stranger can't
/// tell a real username from an invalid one.
pub fn check_invite<'a>(users: &'a [UserConfig], code: &str, now: u64) -> anyhow::Result<&'a UserConfig> {
    let invalid = || anyhow::anyhow!("that invite isn't valid (wrong, used or expired)");
    let (id, secret) = split_invite(code).ok_or_else(invalid)?;
    let Some(user) = users.iter().find(|u| u.id == id) else {
        let _ = verify_password("$argon2id$v=19$m=19456,t=2,p=1$c29tZXNhbHRzb21lc2FsdA$wJ6yOkq2dh6lV9Z6Hn6oS5o8f3sYkQ3sVJ0b1k2Wm0Y", secret);
        return Err(invalid());
    };
    match &user.invite {
        Some(invite) if invite.expires_at > now && verify_password(&invite.secret_hash, secret) => Ok(user),
        _ => Err(invalid()),
    }
}

/// Uses the invite: the member is now tied to `link`'s identity, and the invite is gone. Returns
/// the member's username. Another member can't hold the same identity.
pub fn redeem_invite(config: &mut FileConfig, code: &str, link: TruthIdLink, now: u64) -> anyhow::Result<String> {
    let id = check_invite(&config.users, code, now)?.id.clone();
    anyhow::ensure!(
        !config.users.iter().any(|u| u.id != id && u.truthid.as_ref().is_some_and(|t| t.identity_id == link.identity_id)),
        "that TruthID is already linked to another member"
    );
    let user = find_mut(config, &id)?;
    user.truthid = Some(link);
    user.invite = None;
    Ok(id)
}

/// The owner unties a member's TruthID and cancels an invite that's still open.
pub fn unlink_truthid(config: &mut FileConfig, id: &str) -> anyhow::Result<()> {
    let user = find_mut(config, id)?;
    user.truthid = None;
    user.invite = None;
    Ok(())
}

/// The root sets a new provisional password (a forgotten one).
pub fn reset_password(config: &mut FileConfig, id: &str, temp_password: &str) -> anyhow::Result<()> {
    let hash = hash_password(temp_password)?;
    let user = find_mut(config, id)?;
    user.password_hash = hash;
    user.must_change_password = true;
    // The owner can't open the member's key, so the password wrap is now dead weight.
    user.key_needs_recovery = user.key.is_some();
    Ok(())
}

/// What `change_password` hands back when the member's data is now (or still) encrypted.
pub struct PasswordChange {
    /// The member's key, for the hub to hold.
    pub key: Option<MemberKey>,
    /// A recovery code made now — only when this change turned encryption on. Shown once.
    pub new_recovery_code: Option<String>,
}

/// The member picks their own password; `old` has to be the current one. Their data key follows:
/// created from `new` if they have none (never from a provisional password, which the owner
/// knows), re-wrapped by `new` if they do. After an owner's reset the old password can't open it,
/// so `recovery_code` has to.
pub fn change_password(config: &mut FileConfig, id: &str, old: &str, new: &str, recovery_code: Option<&str>) -> anyhow::Result<PasswordChange> {
    change_password_with(config, id, old, new, recovery_code, true)
}

/// `change_password`, where `create_key` is whether the client can show the recovery code that comes
/// with a new key: when it can't, a member without a key stays without one (a code nobody sees is a
/// key nobody has) and gets it later, from a client that can.
pub fn change_password_with(config: &mut FileConfig, id: &str, old: &str, new: &str, recovery_code: Option<&str>, create_key: bool) -> anyhow::Result<PasswordChange> {
    let (policy, escrow_pub) = effective_policy(config);
    let user = find_mut(config, id)?;
    anyhow::ensure!(verify_password(&user.password_hash, old), "the current password is wrong");
    anyhow::ensure!(old != new, "pick a password different from the current one");
    let hash = hash_password(new)?;
    // Everything that can fail comes before anything changes.
    let (key, wraps, new_recovery_code) = match &user.key {
        None if !create_key => (None, None, None),
        None => {
            let (key, wraps, code) = new_key_wraps(new, policy, escrow_pub.as_deref())?;
            (Some(key), Some(wraps), Some(code))
        }
        Some(current) if user.key_needs_recovery => {
            anyhow::ensure!(
                current.policy != RecoveryPolicy::Consent,
                "the owner reset your password, and this workspace's recovery needs the owner's recovery key together with your code — ask them to recover your data"
            );
            let code = recovery_code.filter(|c| !c.trim().is_empty()).ok_or_else(|| anyhow::anyhow!("the owner reset your password: give your recovery code to keep your data"))?;
            let key = member_crypto::unwrap_with_code(&current.by_recovery, code)?;
            let wraps = KeyWraps { by_password: member_crypto::wrap_with_password(&key, new)?, ..current.clone() };
            (Some(key), Some(wraps), None)
        }
        Some(current) => {
            let key = member_crypto::unwrap_with_password(&current.by_password, old)?;
            let wraps = KeyWraps { by_password: member_crypto::wrap_with_password(&key, new)?, ..current.clone() };
            (Some(key), Some(wraps), None)
        }
    };
    user.password_hash = hash;
    user.must_change_password = false;
    user.key = wraps;
    user.key_needs_recovery = false;
    Ok(PasswordChange { key, new_recovery_code })
}

/// The workspace's recovery policy and the owner's public key it needs. A policy that needs the key
/// with none set counts as `private`: nobody's data is left waiting for a key that isn't there.
fn effective_policy(config: &FileConfig) -> (RecoveryPolicy, Option<String>) {
    match (config.recovery_policy, &config.recovery_public_key) {
        (RecoveryPolicy::Private, _) | (_, None) => (RecoveryPolicy::Private, None),
        (policy, Some(public)) => (policy, Some(public.clone())),
    }
}

/// The recovery policy the workspace is really under: what `config` says, unless it needs the
/// owner's recovery key and none is set.
pub fn workspace_policy(config: &FileConfig) -> RecoveryPolicy {
    effective_policy(config).0
}

/// What the recovery code wraps under `policy`: the key itself, or — under `consent` — the key
/// already sealed to the owner, so the code alone gets nobody to the data.
fn recovery_wrap(key: &MemberKey, code: &str, policy: RecoveryPolicy, escrow_pub: Option<&str>) -> anyhow::Result<String> {
    match (policy, escrow_pub) {
        (RecoveryPolicy::Consent, Some(public)) => member_crypto::wrap_blob_with_code(&recovery::escrow_seal(key, public)?, code),
        (RecoveryPolicy::Consent, None) => anyhow::bail!("the recovery policy needs the owner's recovery key, and none is set"),
        _ => member_crypto::wrap_with_code(key, code),
    }
}

/// A new key, wrapped by `password` and by a new recovery code (returned, to be shown once), as
/// `policy` asks: for `company`, also sealed to the owner's key.
fn new_key_wraps(password: &str, policy: RecoveryPolicy, escrow_pub: Option<&str>) -> anyhow::Result<(MemberKey, KeyWraps, String)> {
    let key = member_crypto::new_key();
    let code = member_crypto::generate_recovery_code();
    let by_escrow = match (policy, escrow_pub) {
        (RecoveryPolicy::Company, Some(public)) => Some(BASE64.encode(recovery::escrow_seal(&key, public)?)),
        _ => None,
    };
    let wraps = KeyWraps {
        by_password: member_crypto::wrap_with_password(&key, password)?,
        by_recovery: recovery_wrap(&key, &code, policy, escrow_pub)?,
        policy,
        by_escrow,
        escrow_id: escrow_pub.filter(|_| !policy.is_private()).map(recovery::escrow_id),
    };
    Ok((key, wraps, code))
}

/// A member from before fatia 4, signing in with their own password: turns encryption on with a
/// new key and recovery code. `None` when there's nothing to do (they have a key, or still have
/// the owner's provisional password — `change_password` creates it then).
pub fn enable_encryption(config: &mut FileConfig, id: &str, password: &str) -> anyhow::Result<Option<(MemberKey, String)>> {
    let (policy, escrow_pub) = effective_policy(config);
    let user = find_mut(config, id)?;
    if user.key.is_some() || user.must_change_password {
        return Ok(None);
    }
    anyhow::ensure!(verify_password(&user.password_hash, password), "the current password is wrong");
    let (key, wraps, code) = new_key_wraps(password, policy, escrow_pub.as_deref())?;
    user.key = Some(wraps);
    Ok(Some((key, code)))
}

/// The member's key at sign-in. `None` when they have none yet, or need their recovery code first.
pub fn open_key(user: &UserConfig, password: &str) -> anyhow::Result<Option<MemberKey>> {
    match &user.key {
        Some(wraps) if !user.key_needs_recovery => Ok(Some(member_crypto::unwrap_with_password(&wraps.by_password, password)?)),
        _ => Ok(None),
    }
}

/// The member asks for a new recovery code (the old one stops working). Needs their password.
pub fn regenerate_recovery_code(config: &mut FileConfig, id: &str, password: &str) -> anyhow::Result<String> {
    let escrow_pub = config.recovery_public_key.clone();
    let user = find_mut(config, id)?;
    anyhow::ensure!(verify_password(&user.password_hash, password), "the current password is wrong");
    let wraps = user.key.as_ref().filter(|_| !user.key_needs_recovery).ok_or_else(|| anyhow::anyhow!("your data isn't encrypted with a key of yours yet"))?;
    let key = member_crypto::unwrap_with_password(&wraps.by_password, password)?;
    let code = member_crypto::generate_recovery_code();
    // Under `consent` the code wraps the key sealed to the owner's key as it is now.
    let by_recovery = recovery_wrap(&key, &code, wraps.policy, escrow_pub.as_deref())?;
    let escrow_id = if wraps.policy == RecoveryPolicy::Consent { escrow_pub.as_deref().map(recovery::escrow_id) } else { wraps.escrow_id.clone() };
    user.key = Some(KeyWraps { by_recovery, escrow_id, ..wraps.clone() });
    Ok(code)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// What `sync_recovery_policy` did.
#[derive(Debug, PartialEq, Eq)]
pub struct PolicyOutcome {
    /// The member's data now follows the workspace's policy (or already did).
    pub in_step: bool,
    /// It doesn't yet, and waits for the person: the change is to a weaker policy and they haven't said
    /// yes, or the client can't show the recovery code that comes with it.
    pub pending: bool,
    /// A recovery code made now, to be shown once (entering or leaving `consent` needs one).
    pub new_code: Option<String>,
}

/// Brings a member's data in step with the workspace's recovery policy, with their key open (at
/// sign-in, or when they say yes). A change to a **weaker** policy waits for `may_weaken` — the
/// person's yes — and one that needs a new recovery code waits for a client that can show it.
/// Entering or leaving `consent` (or replacing the owner's key under it) makes a new code, since the
/// old one can't be re-wrapped without being typed.
pub fn sync_recovery_policy(config: &mut FileConfig, id: &str, key: &MemberKey, may_weaken: bool, can_show_code: bool) -> anyhow::Result<PolicyOutcome> {
    let (target, escrow_pub) = effective_policy(config);
    let user = find_mut(config, id)?;
    let Some(current) = user.key.clone() else { return Ok(PolicyOutcome { in_step: true, pending: false, new_code: None }) };
    let target_id = escrow_pub.as_deref().map(recovery::escrow_id);
    let expected_id = if target.is_private() { None } else { target_id.clone() };
    if current.policy == target && current.escrow_id == expected_id {
        return Ok(PolicyOutcome { in_step: true, pending: false, new_code: None });
    }
    let weaker = target.strength() < current.policy.strength();
    let needs_code = target == RecoveryPolicy::Consent || current.policy == RecoveryPolicy::Consent;
    if (weaker && !may_weaken) || (needs_code && !can_show_code) {
        return Ok(PolicyOutcome { in_step: false, pending: true, new_code: None });
    }

    let (by_recovery, new_code) = if needs_code {
        let code = member_crypto::generate_recovery_code();
        (recovery_wrap(key, &code, target, escrow_pub.as_deref())?, Some(code))
    } else {
        (current.by_recovery.clone(), None)
    };
    let by_escrow = match (target, escrow_pub.as_deref()) {
        (RecoveryPolicy::Company, Some(public)) => Some(BASE64.encode(recovery::escrow_seal(key, public)?)),
        _ => None,
    };
    user.key = Some(KeyWraps { by_password: current.by_password, by_recovery, policy: target, by_escrow, escrow_id: expected_id });
    Ok(PolicyOutcome { in_step: true, pending: false, new_code })
}

/// The workspace's recovery policy is set by the owner. A policy that needs the owner's recovery key
/// makes one if there is none (or if `new_key` asks for another): the private half comes back, to be
/// shown once and never stored. Members' data follows at their next sign-in, so a change to a weaker
/// policy is theirs to accept.
pub fn set_recovery_policy(config: &mut FileConfig, policy: RecoveryPolicy, new_key: bool) -> Option<String> {
    let mut secret = None;
    if new_key || (!policy.is_private() && config.recovery_public_key.is_none()) {
        let pair = recovery::generate_escrow_keypair();
        config.recovery_public_key = Some(pair.public_hex);
        secret = Some(pair.secret_text);
    }
    config.recovery_policy = policy;
    secret
}

/// What `recover_member` gives the owner.
pub struct Recovered {
    /// The provisional password the person signs in with, shown once.
    pub temp_password: String,
    /// The policy it was done under.
    pub kind: RecoveryPolicy,
}

/// The owner opens a member's data with the workspace's recovery key — `company`: the key alone;
/// `consent`: the key and the person's recovery code together; `private`: refused, nobody can. Then a
/// new provisional password is set (wrapping the key, which the owner has just opened anyway), the
/// person signs in with it and picks their own, and the recovery is recorded for them to see.
pub fn recover_member(config: &mut FileConfig, id: &str, recovery_key: &str, code: Option<&str>) -> anyhow::Result<Recovered> {
    let user = find_mut(config, id)?;
    let wraps = user.key.clone().ok_or_else(|| anyhow::anyhow!("'{id}' has no encrypted data to recover"))?;
    let key = match wraps.policy {
        RecoveryPolicy::Private => anyhow::bail!("'{id}' is under the private policy: only their password or their recovery code opens their data, not even you"),
        RecoveryPolicy::Company => {
            let blob = BASE64.decode(wraps.by_escrow.as_deref().ok_or_else(|| anyhow::anyhow!("'{id}' has no company recovery set up"))?.trim())?;
            recovery::escrow_open(&blob, recovery_key)?
        }
        RecoveryPolicy::Consent => {
            let code = code.filter(|c| !c.trim().is_empty()).ok_or_else(|| anyhow::anyhow!("'{id}' is under the consent policy: their recovery code is needed together with your key"))?;
            let blob = member_crypto::unwrap_blob_with_code(&wraps.by_recovery, code)?;
            recovery::escrow_open(&blob, recovery_key)?
        }
    };
    let temp_password = generate_temp_password();
    let hash = hash_password(&temp_password)?;
    let by_password = member_crypto::wrap_with_password(&key, &temp_password)?;
    user.password_hash = hash;
    user.must_change_password = true;
    user.key_needs_recovery = false;
    user.key = Some(KeyWraps { by_password, ..wraps.clone() });
    user.recoveries.push(RecoveryEvent { at_ms: now_ms(), kind: wraps.policy, seen: false });
    Ok(Recovered { temp_password, kind: wraps.policy })
}

/// The member has seen the recoveries the owner made.
pub fn ack_recovery_notices(config: &mut FileConfig, id: &str) -> anyhow::Result<()> {
    for event in &mut find_mut(config, id)?.recoveries {
        event.seen = true;
    }
    Ok(())
}

/// The member chooses whether the assistant may learn from their conversations (`enabled`).
pub fn set_learning_opt_out(config: &mut FileConfig, id: &str, opt_out: bool) -> anyhow::Result<()> {
    find_mut(config, id)?.learning_opt_out = opt_out;
    Ok(())
}

/// Whether a turn of member `id` (`None`: the owner) may be learned from: the workspace has learning
/// on and the member hasn't opted out.
pub fn learning_allowed(config: &FileConfig, id: Option<&str>) -> bool {
    config.learning.enabled && !id.and_then(|id| config.users.iter().find(|u| u.id == id)).is_some_and(|u| u.learning_opt_out)
}

/// The provider the assistant's learning uses for member `id` (`None`: the owner): the one the owner set for that
/// member, else the workspace's `[learning] provider`, else none (the conversation's own model).
pub fn learning_provider_for<'a>(config: &'a FileConfig, id: Option<&str>) -> Option<&'a str> {
    id.and_then(|id| config.users.iter().find(|u| u.id == id)).and_then(|u| u.learning_provider.as_deref()).or(config.learning.provider.as_deref())
}

/// Takes the member out of the workspace, with their own agents and every share naming them. Their
/// vault and conversations stay on disk; if those are encrypted, the entry — the wrapped key — goes
/// to `removed_users` so they aren't lost for good (`restore_user`, `purge_removed_user`).
pub fn remove_user(config: &mut FileConfig, id: &str) -> anyhow::Result<()> {
    let i = config.users.iter().position(|u| u.id == id).ok_or_else(|| anyhow::anyhow!("no user named '{id}'"))?;
    let user = config.users.remove(i);
    if user.key.is_some() {
        config.removed_users.push(user);
    }
    let theirs: Vec<String> = config.agents.iter().filter(|a| a.owner.as_deref() == Some(id)).map(|a| a.id.clone()).collect();
    for agent in theirs {
        crate::remove_agent_references(config, &agent);
    }
    for agent in &mut config.agents {
        agent.shared_with.retain(|s| s != id);
    }
    for space in &mut config.spaces {
        space.readers.retain(|s| s != id);
        space.writers.retain(|s| s != id);
    }
    Ok(())
}

/// Brings a removed member back, with the same password, key and data. Their own agents and shares
/// don't come back: those went with the removal.
pub fn restore_user(config: &mut FileConfig, id: &str) -> anyhow::Result<()> {
    let i = config.removed_users.iter().position(|u| u.id == id).ok_or_else(|| anyhow::anyhow!("no removed member named '{id}' — `warden-server users removed` lists them"))?;
    anyhow::ensure!(!config.users.iter().any(|u| u.id == id), "there's already a user named '{id}'");
    let user = config.removed_users.remove(i);
    config.users.push(user);
    check_users(&config.users)
}

/// Forgets a removed member for good: the entry goes, and with it the only way to open their data —
/// the caller deletes the folders. Returns whether there was one.
pub fn purge_removed_user(config: &mut FileConfig, id: &str) -> anyhow::Result<()> {
    let i = config.removed_users.iter().position(|u| u.id == id).ok_or_else(|| anyhow::anyhow!("no removed member named '{id}' — `warden-server users removed` lists them"))?;
    config.removed_users.remove(i);
    Ok(())
}

/// The owner sets which tools a member may use; `None` goes back to the safe default.
pub fn set_user_tools(config: &mut FileConfig, id: &str, tools: Option<Vec<String>>) -> anyhow::Result<()> {
    let tools = tools.map(|list| {
        let mut list: Vec<String> = list.into_iter().map(|t| t.trim().to_string()).filter(|t| !t.is_empty() && !NEVER_FOR_MEMBERS.contains(&t.as_str())).collect();
        list.sort();
        list.dedup();
        list
    });
    find_mut(config, id)?.tools = tools;
    Ok(())
}

/// The owner sets what a member does with the workspace's organization of agents (P120): `none`, `view` or `edit`.
pub fn set_user_org_access(config: &mut FileConfig, id: &str, access: &str) -> anyhow::Result<()> {
    let access = OrgAccess::parse(access).ok_or_else(|| anyhow::anyhow!("'{access}' is not an access to the organization (none, view or edit)"))?;
    find_mut(config, id)?.org_access = access;
    Ok(())
}

/// The owner sets the folders a member may pick as a conversation's working folder (P102): `workdirs` of the hub's own
/// machine (absolute paths) and `node_workdirs` on nodes (relative to what each node lends). Empty lists take them all
/// away. A bad entry refuses the whole change, so what is saved is what was shown.
pub fn set_user_workdirs(config: &mut FileConfig, id: &str, workdirs: Vec<String>, node_workdirs: Vec<NodeFolder>) -> anyhow::Result<()> {
    let mut local: Vec<String> = Vec::new();
    for path in workdirs.into_iter().map(|p| p.trim().to_string()).filter(|p| !p.is_empty()) {
        warden_core::project::validate_workdir(&path)?;
        if !local.contains(&path) {
            local.push(path);
        }
    }
    let mut nodes: Vec<NodeFolder> = Vec::new();
    for folder in node_workdirs {
        let node = folder.node.trim().to_string();
        anyhow::ensure!(!node.is_empty() && !node.contains(':'), "a node folder needs the node's id (no ':')");
        let path = folder.path.trim().trim_matches('/').to_string();
        crate::check_node_path(&path).map_err(|e| anyhow::anyhow!(e))?;
        let entry = NodeFolder { node, path };
        if !nodes.contains(&entry) {
            nodes.push(entry);
        }
    }
    let user = find_mut(config, id)?;
    user.workdirs = local;
    user.node_workdirs = nodes;
    Ok(())
}

/// The owner picks the model the assistant's learning uses for a member (`None`, or blank: back to the
/// workspace's `[learning] provider`). It has to be a provider or a combo the hub has.
pub fn set_user_learning_provider(config: &mut FileConfig, id: &str, provider: Option<String>) -> anyhow::Result<()> {
    let provider = provider.map(|p| p.trim().to_string()).filter(|p| !p.is_empty());
    if let Some(provider) = &provider {
        if !config.providers.iter().any(|p| &p.id == provider) && !config.combos.iter().any(|c| &c.id == provider) {
            anyhow::bail!("no model or combo named '{provider}'");
        }
    }
    find_mut(config, id)?.learning_provider = provider;
    Ok(())
}

/// A member creates (`original_id` = `None`) or edits one of their own agents. Whatever it says, it
/// stays theirs, unshared and without any `can_*` power; its tools are cut to what the member has
/// (`available`: the hub's tools), and its model is the hub's default unless it names a model the
/// hub has.
pub fn save_member_agent(config: &mut FileConfig, owner: &str, original_id: Option<&str>, agent: AgentConfig, available: &[String]) -> anyhow::Result<()> {
    let user = config.users.iter().find(|u| u.id == owner).ok_or_else(|| anyhow::anyhow!("no user named '{owner}'"))?;
    let allowed = member_tools(user, available);
    let id = agent.id.trim().to_string();
    anyhow::ensure!(!id.is_empty(), "every agent needs a name");
    anyhow::ensure!(!agent.persona.trim().is_empty(), "describe the agent in its persona");
    let taken = config.agents.iter().any(|a| a.id == id && Some(a.id.as_str()) != original_id);
    anyhow::ensure!(!taken, "the name '{id}' is taken — pick another");
    let checked = AgentConfig {
        id: id.clone(),
        persona: agent.persona.trim().to_string(),
        provider_id: agent.provider_id.filter(|p| config.providers.iter().any(|x| &x.id == p) || config.combos.iter().any(|c| &c.id == p)),
        can_delegate_to_agents: false,
        can_manage_agents: false,
        can_message_agents: false,
        can_manage_tasks: false,
        allowed_tools: agent.allowed_tools.map(|list| list.into_iter().filter(|t| allowed.contains(t)).collect()),
        // A member never picks a level for their own agent; it stays at what an agent had before levels existed.
        autonomy: crate::default_autonomy(),
        approval_required: Vec::new(),
        // A member's agent is outside the organization.
        role: None,
        reports_to: None,
        owner: Some(owner.to_string()),
        shared_with: Vec::new(),
        delegation_models: Vec::new(),
        can_start_tasks: true,
        can_create_workers: true,
    };
    match original_id {
        Some(original) => {
            let i = config
                .agents
                .iter()
                .position(|a| a.id == original && a.owner.as_deref() == Some(owner))
                .ok_or_else(|| anyhow::anyhow!("you have no agent named '{original}'"))?;
            config.agents[i] = checked;
        }
        None => config.agents.push(checked),
    }
    Ok(())
}

/// A member deletes one of their own agents.
pub fn delete_member_agent(config: &mut FileConfig, owner: &str, id: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        config.agents.iter().any(|a| a.id == id && a.owner.as_deref() == Some(owner)),
        "you have no agent named '{id}'"
    );
    crate::remove_agent_references(config, id);
    Ok(())
}

/// Keeps `shared_with` to members that exist (or `EVERYONE`), without repeats.
pub fn clean_shares(shared_with: Vec<String>, users: &[UserConfig]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for share in shared_with.into_iter().map(|s| s.trim().to_ascii_lowercase()) {
        if (share == EVERYONE || users.iter().any(|u| u.id == share)) && !out.contains(&share) {
            out.push(share);
        }
    }
    if out.iter().any(|s| s == EVERYONE) {
        return vec![EVERYONE.to_string()];
    }
    out
}

fn find_mut<'a>(config: &'a mut FileConfig, id: &str) -> anyhow::Result<&'a mut UserConfig> {
    config.users.iter_mut().find(|u| u.id == id).ok_or_else(|| anyhow::anyhow!("no user named '{id}'"))
}

/// Where each member's own things live on the hub: `~/.config/warden/users/<id>/` — outside the
/// root's vault, which syncs and which the root's agents read.
pub fn default_users_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("warden").join("users"))
}

/// A member's vault under `users_dir`.
pub fn user_vault_path(users_dir: &Path, id: &str) -> PathBuf {
    users_dir.join(id).join("vault")
}

/// Where a member's generated files (documents, oversized media) go.
pub fn user_generated_path(users_dir: &Path, id: &str) -> PathBuf {
    users_dir.join(id).join("generated")
}

/// A member's conversations under the hub's conversations directory.
pub fn user_conversations_dir(conversations_root: &Path, id: &str) -> PathBuf {
    conversations_root.join("users").join(id)
}

/// The root's conversations under the hub's conversations directory — every device of theirs shares it.
pub fn root_conversations_dir(conversations_root: &Path) -> PathBuf {
    conversations_root.join(ROOT_ID)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_password_is_only_kept_as_a_hash_that_verifies() {
        let hash = hash_password("correct horse").unwrap();
        assert!(hash.starts_with("$argon2id$") && !hash.contains("correct horse"));
        assert!(verify_password(&hash, "correct horse"));
        assert!(!verify_password(&hash, "wrong horse"));
        assert!(!verify_password("not a hash", "correct horse"));
        assert!(hash_password("short").is_err());
        assert_ne!(hash_password("correct horse").unwrap(), hash, "a fresh salt each time");
    }

    #[test]
    fn an_invite_links_a_truthid_once() {
        let mut config = FileConfig::default();
        add_user(&mut config, "ana", "Ana", "temp-password").unwrap();
        add_user(&mut config, "bruno", "Bruno", "temp-password").unwrap();
        let link = |id: u64| TruthIdLink { username: format!("u{id}"), identity_id: id, linked_at: 100 };

        let code = create_invite(&mut config, "ana", 100).unwrap();
        assert!(code.starts_with("ana:") && !config.users[0].invite.as_ref().unwrap().secret_hash.contains(&code[4..]));
        assert!(check_invite(&config.users, &code, 100).is_ok());
        // Wrong secret, unknown person, garbage and another member's invite all fail the same way.
        let wrong = check_invite(&config.users, "ana:nope", 100).unwrap_err().to_string();
        assert_eq!(check_invite(&config.users, "zed:nope", 100).unwrap_err().to_string(), wrong);
        assert_eq!(check_invite(&config.users, "garbage", 100).unwrap_err().to_string(), wrong);
        assert_eq!(check_invite(&config.users, &code.replace("ana:", "bruno:"), 100).unwrap_err().to_string(), wrong);
        assert!(check_invite(&config.users, &code, 100 + INVITE_TTL_SECS).is_err(), "expired");

        assert_eq!(redeem_invite(&mut config, &code, link(7), 101).unwrap(), "ana");
        assert_eq!(config.users[0].truthid, Some(link(7)));
        assert!(config.users[0].invite.is_none());
        assert!(redeem_invite(&mut config, &code, link(7), 102).is_err(), "single use");

        // The same identity can't be Bruno's too.
        let code = create_invite(&mut config, "bruno", 100).unwrap();
        assert!(redeem_invite(&mut config, &code, link(7), 101).is_err());
        assert!(redeem_invite(&mut config, &code, link(8), 101).is_ok());

        unlink_truthid(&mut config, "ana").unwrap();
        assert!(config.users[0].truthid.is_none());
        assert!(create_invite(&mut config, "nobody", 100).is_err());
        // What's saved reads back.
        let text = toml::to_string(&config).unwrap();
        assert_eq!(toml::from_str::<FileConfig>(&text).unwrap().users[1].truthid, Some(link(8)));
    }

    #[test]
    fn a_members_learning_provider_round_trips_through_toml_and_stays_out_of_the_file_when_unset() {
        let mut config = FileConfig::default();
        add_user(&mut config, "ana", "Ana", "temp-pass-1").unwrap();
        let plain = toml::to_string(&config).unwrap();
        assert!(!plain.contains("learning_provider") && !plain.contains("learning_opt_out"), "{plain}");

        config.users[0].learning_provider = Some("cheap".into());
        let text = toml::to_string(&config).unwrap();
        assert!(text.contains("learning_provider = \"cheap\""), "{text}");
        assert_eq!(toml::from_str::<FileConfig>(&text).unwrap().users[0].learning_provider.as_deref(), Some("cheap"));
        assert_eq!(toml::from_str::<FileConfig>(&plain).unwrap().users[0].learning_provider, None, "a config from before loads");
    }

    #[test]
    fn a_members_folders_round_trip_through_toml_stay_out_of_the_file_when_unset_and_cover_what_is_inside() {
        let mut config = FileConfig::default();
        add_user(&mut config, "ana", "Ana", "temp-pass-1").unwrap();
        let plain = toml::to_string(&config).unwrap();
        assert!(!plain.contains("workdirs") && !plain.contains("node_workdirs"), "{plain}");

        config.users[0].workdirs = vec!["/srv/work".into()];
        config.users[0].node_workdirs = vec![NodeFolder { node: "node-a-1".into(), path: "projects".into() }, NodeFolder { node: "node-b-2".into(), path: String::new() }];
        let text = toml::to_string(&config).unwrap();
        let back = toml::from_str::<FileConfig>(&text).unwrap();
        assert_eq!(back.users[0].node_workdirs, config.users[0].node_workdirs, "{text}");
        assert_eq!(back.users[0].workdirs, ["/srv/work"]);
        assert_eq!(toml::from_str::<FileConfig>(&plain).unwrap().users[0].node_workdirs, Vec::<NodeFolder>::new(), "a config from before loads");

        let [projects, all] = &config.users[0].node_workdirs[..] else { panic!() };
        assert!(projects.covers("node-a-1", "projects") && projects.covers("node-a-1", "projects/web/src"));
        assert!(!projects.covers("node-a-1", "projects-not") && !projects.covers("node-a-1", "") && !projects.covers("node-c-3", "projects"));
        assert!(all.covers("node-b-2", "") && all.covers("node-b-2", "anything/at/all") && !all.covers("node-a-1", ""));
    }

    #[test]
    fn the_decoy_hash_is_well_formed() {
        // If it didn't parse, an unknown name would answer faster than a wrong password.
        assert!(argon2::PasswordHash::new("$argon2id$v=19$m=19456,t=2,p=1$c29tZXNhbHRzb21lc2FsdA$wJ6yOkq2dh6lV9Z6Hn6oS5o8f3sYkQ3sVJ0b1k2Wm0Y").is_ok());
    }

    #[test]
    fn members_are_added_changed_and_removed() {
        let mut config = FileConfig::default();
        let temp = generate_temp_password();
        assert_eq!(temp.len(), TEMP_PASSWORD_LEN);
        add_user(&mut config, " Ana ", "Ana Souza", &temp).unwrap();
        assert_eq!((config.users[0].id.as_str(), config.users[0].must_change_password), ("ana", true));
        assert!(add_user(&mut config, "ana", "Other", &temp).is_err(), "unique");
        assert!(add_user(&mut config, "root", "Nope", &temp).is_err(), "reserved");
        assert!(add_user(&mut config, "a b", "Nope", &temp).is_err(), "safe as a folder name");

        assert_eq!(authenticate_user(&config.users, "ANA", &temp).map(|u| u.id.as_str()), Some("ana"));
        assert!(authenticate_user(&config.users, "ana", "wrong-password").is_none());
        assert!(authenticate_user(&config.users, "bruno", &temp).is_none());

        assert!(change_password(&mut config, "ana", "wrong-password", "her own pass", None).is_err());
        change_password(&mut config, "ana", &temp, "her own pass", None).unwrap();
        assert!(!config.users[0].must_change_password);
        assert!(authenticate_user(&config.users, "ana", "her own pass").is_some());

        reset_password(&mut config, "ana", "a new temp pw").unwrap();
        assert!(config.users[0].must_change_password && authenticate_user(&config.users, "ana", "her own pass").is_none());
        rename_user(&mut config, "ana", "Ana S.").unwrap();
        remove_user(&mut config, "ana").unwrap();
        assert!(config.users.is_empty() && remove_user(&mut config, "ana").is_err());
    }

    fn config_with_ana(temp: &str) -> FileConfig {
        let mut config = FileConfig::default();
        add_user(&mut config, "ana", "Ana", temp).unwrap();
        config
    }

    #[test]
    fn the_key_is_born_with_the_members_own_password_and_never_with_the_provisional_one() {
        let mut config = config_with_ana("provisional-1");
        assert!(config.users[0].key.is_none(), "the owner's provisional password never makes a key");
        assert!(enable_encryption(&mut config, "ana", "provisional-1").unwrap().is_none(), "still on the provisional password");

        let change = change_password(&mut config, "ana", "provisional-1", "anas-own-pass", None).unwrap();
        let (key, code) = (change.key.unwrap(), change.new_recovery_code.unwrap());
        let wraps = config.users[0].key.clone().unwrap();
        assert!(member_crypto::unwrap_with_password(&wraps.by_password, "provisional-1").is_err(), "the owner can't open it");
        assert_eq!(*open_key(&config.users[0], "anas-own-pass").unwrap().unwrap(), *key);
        assert_eq!(*member_crypto::unwrap_with_code(&wraps.by_recovery, &code).unwrap(), *key);
    }

    #[test]
    fn changing_the_password_keeps_the_same_key_and_the_same_recovery_code() {
        let mut config = config_with_ana("provisional-1");
        let first = change_password(&mut config, "ana", "provisional-1", "anas-own-pass", None).unwrap();
        let (key, code) = (first.key.unwrap(), first.new_recovery_code.unwrap());

        let second = change_password(&mut config, "ana", "anas-own-pass", "anas-newer-pass", None).unwrap();
        assert!(second.new_recovery_code.is_none(), "no second code");
        assert_eq!(*second.key.unwrap(), *key);
        assert!(open_key(&config.users[0], "anas-own-pass").is_err(), "the old password no longer opens it");
        assert_eq!(*open_key(&config.users[0], "anas-newer-pass").unwrap().unwrap(), *key);
        let wraps = config.users[0].key.clone().unwrap();
        assert_eq!(*member_crypto::unwrap_with_code(&wraps.by_recovery, &code).unwrap(), *key, "the code still works");
    }

    #[test]
    fn after_an_owner_reset_only_the_recovery_code_brings_the_data_back() {
        let mut config = config_with_ana("provisional-1");
        let first = change_password(&mut config, "ana", "provisional-1", "anas-own-pass", None).unwrap();
        let (key, code) = (first.key.unwrap(), first.new_recovery_code.unwrap());

        reset_password(&mut config, "ana", "provisional-2").unwrap();
        assert!(config.users[0].key_needs_recovery);
        assert!(open_key(&config.users[0], "provisional-2").unwrap().is_none(), "the hub can't open it for the provisional password");

        let before = config.users.clone();
        assert!(change_password(&mut config, "ana", "provisional-2", "anas-third-pass", None).is_err(), "no code, no data");
        assert!(change_password(&mut config, "ana", "provisional-2", "anas-third-pass", Some("  ")).is_err());
        let wrong = member_crypto::generate_recovery_code();
        assert!(change_password(&mut config, "ana", "provisional-2", "anas-third-pass", Some(&wrong)).is_err(), "a wrong code");
        assert_eq!(config.users, before, "a failed change leaves everything as it was");

        let back = change_password(&mut config, "ana", "provisional-2", "anas-third-pass", Some(&code.to_ascii_lowercase())).unwrap();
        assert_eq!(*back.key.unwrap(), *key, "the very same key");
        assert!(!config.users[0].key_needs_recovery && !config.users[0].must_change_password);
        assert_eq!(*open_key(&config.users[0], "anas-third-pass").unwrap().unwrap(), *key);
    }

    /// Ana with her own password, under the workspace's `policy` (its key made after the policy was set).
    fn ana_under(policy: RecoveryPolicy) -> (FileConfig, String, MemberKey, String) {
        let mut config = FileConfig::default();
        let secret = set_recovery_policy(&mut config, policy, false).unwrap_or_default();
        add_user(&mut config, "ana", "Ana", "provisional-1").unwrap();
        let change = change_password(&mut config, "ana", "provisional-1", "anas-own-pass", None).unwrap();
        (config, secret, change.key.unwrap(), change.new_recovery_code.unwrap())
    }

    #[test]
    fn under_consent_the_code_alone_and_the_owner_alone_open_nothing_but_together_they_do() {
        let (mut config, owner_key, key, code) = ana_under(RecoveryPolicy::Consent);
        let wraps = config.users[0].key.clone().unwrap();
        assert_eq!((wraps.policy, wraps.by_escrow.is_none()), (RecoveryPolicy::Consent, true));
        assert!(member_crypto::unwrap_with_code(&wraps.by_recovery, &code).is_err(), "the code alone opens nothing");
        assert_eq!(*open_key(&config.users[0], "anas-own-pass").unwrap().unwrap(), *key, "her password still opens it");

        assert!(recover_member(&mut config, "ana", &owner_key, None).is_err(), "the key alone: no");
        assert!(recover_member(&mut config, "ana", &owner_key, Some(&member_crypto::generate_recovery_code())).is_err(), "another code");
        assert!(recover_member(&mut config, "ana", &recovery::generate_escrow_keypair().secret_text, Some(&code)).is_err(), "another key");
        assert!(!config.users[0].must_change_password && config.users[0].recoveries.is_empty(), "the refusals changed nothing");

        let recovered = recover_member(&mut config, "ana", &owner_key, Some(&code)).unwrap();
        assert_eq!(recovered.kind, RecoveryPolicy::Consent);
        assert!(config.users[0].must_change_password && authenticate_user(&config.users, "ana", &recovered.temp_password).is_some());
        assert_eq!(*open_key(&config.users[0], &recovered.temp_password).unwrap().unwrap(), *key, "the very same data key");
        assert!(authenticate_user(&config.users, "ana", "anas-own-pass").is_none(), "her old password is gone");
        assert_eq!(config.users[0].recoveries.len(), 1);
    }

    #[test]
    fn under_company_the_owners_key_alone_recovers_and_is_recorded_for_the_person() {
        let (mut config, owner_key, key, code) = ana_under(RecoveryPolicy::Company);
        let wraps = config.users[0].key.clone().unwrap();
        assert!(wraps.by_escrow.is_some());
        assert_eq!(*member_crypto::unwrap_with_code(&wraps.by_recovery, &code).unwrap(), *key, "her code still opens it, as under private");

        assert!(recover_member(&mut config, "ana", &recovery::generate_escrow_keypair().secret_text, None).is_err());
        let recovered = recover_member(&mut config, "ana", &owner_key, None).unwrap();
        assert_eq!(recovered.kind, RecoveryPolicy::Company);
        assert_eq!(*open_key(&config.users[0], &recovered.temp_password).unwrap().unwrap(), *key);
        let events = &config.users[0].recoveries;
        assert_eq!((events.len(), events[0].kind, events[0].seen), (1, RecoveryPolicy::Company, false));

        // She signs in with the provisional password and picks her own: same key again, and she's seen it.
        let back = change_password(&mut config, "ana", &recovered.temp_password, "anas-third-pass", None).unwrap();
        assert_eq!(*back.key.unwrap(), *key);
        ack_recovery_notices(&mut config, "ana").unwrap();
        assert!(config.users[0].recoveries[0].seen && config.users[0].recoveries.len() == 1, "kept as history");
    }

    #[test]
    fn under_private_nobody_recovers_for_them() {
        let (mut config, _, _, _) = ana_under(RecoveryPolicy::Private);
        let owner = recovery::generate_escrow_keypair();
        let err = recover_member(&mut config, "ana", &owner.secret_text, None).map(|_| ()).unwrap_err();
        assert!(err.to_string().contains("private"), "{err}");
        assert!(recover_member(&mut config, "nobody", &owner.secret_text, None).is_err());
    }

    #[test]
    fn a_policy_that_needs_the_owners_key_without_one_falls_back_to_private() {
        let mut config = FileConfig { recovery_policy: RecoveryPolicy::Consent, ..FileConfig::default() };
        add_user(&mut config, "ana", "Ana", "provisional-1").unwrap();
        let change = change_password(&mut config, "ana", "provisional-1", "anas-own-pass", None).unwrap();
        let wraps = config.users[0].key.clone().unwrap();
        assert_eq!(wraps.policy, RecoveryPolicy::Private);
        assert_eq!(*member_crypto::unwrap_with_code(&wraps.by_recovery, &change.new_recovery_code.unwrap()).unwrap(), *change.key.unwrap());
    }

    #[test]
    fn a_change_to_a_weaker_policy_waits_for_the_person_and_a_stronger_one_does_not() {
        let (mut config, owner_key, key, code) = ana_under(RecoveryPolicy::Private);
        // The owner moves the workspace to company: weaker, so it waits.
        let secret = set_recovery_policy(&mut config, RecoveryPolicy::Company, false).expect("a key is made the first time");
        let waiting = sync_recovery_policy(&mut config, "ana", &key, false, true).unwrap();
        assert_eq!(waiting, PolicyOutcome { in_step: false, pending: true, new_code: None });
        assert_eq!(config.users[0].key.as_ref().unwrap().policy, RecoveryPolicy::Private, "nothing changed without her yes");
        assert!(recover_member(&mut config, "ana", &secret, None).is_err(), "so the owner can't recover yet");

        let accepted = sync_recovery_policy(&mut config, "ana", &key, true, true).unwrap();
        assert_eq!(accepted, PolicyOutcome { in_step: true, pending: false, new_code: None }, "private to company needs no new code");
        assert!(recover_member(&mut config, "ana", &secret, None).is_ok());
        assert_eq!(*member_crypto::unwrap_with_code(&config.users[0].key.as_ref().unwrap().by_recovery, &code).unwrap(), *key, "her code is the same one");
        let _ = owner_key;

        // Back to private: stronger, so it applies without asking, and the owner's way in is gone.
        set_recovery_policy(&mut config, RecoveryPolicy::Private, false);
        let stronger = sync_recovery_policy(&mut config, "ana", &key, false, false).unwrap();
        assert_eq!(stronger, PolicyOutcome { in_step: true, pending: false, new_code: None });
        let wraps = config.users[0].key.clone().unwrap();
        assert!(wraps.by_escrow.is_none() && wraps.escrow_id.is_none() && wraps.policy == RecoveryPolicy::Private);
        assert!(recover_member(&mut config, "ana", &secret, None).is_err());
        assert!(sync_recovery_policy(&mut config, "ana", &key, false, false).unwrap().in_step, "already in step: nothing to do");
    }

    #[test]
    fn entering_and_leaving_consent_makes_a_new_code_that_needs_a_client_that_shows_it() {
        let (mut config, _, key, old_code) = ana_under(RecoveryPolicy::Private);
        let secret = set_recovery_policy(&mut config, RecoveryPolicy::Consent, false).unwrap();
        // Weaker than private, and a new code: both must be satisfied.
        assert!(sync_recovery_policy(&mut config, "ana", &key, true, false).unwrap().pending, "a client that can't show the code waits");
        let entered = sync_recovery_policy(&mut config, "ana", &key, true, true).unwrap();
        let code = entered.new_code.expect("a new code");
        assert_ne!(code, old_code);
        let wraps = config.users[0].key.clone().unwrap();
        assert!(member_crypto::unwrap_with_code(&wraps.by_recovery, &old_code).is_err(), "the old code stopped working");
        assert!(recover_member(&mut config, "ana", &secret, Some(&code)).is_ok(), "and the new one, with the owner's key, recovers");

        // Back to private: stronger, but leaving consent makes yet another code.
        set_recovery_policy(&mut config, RecoveryPolicy::Private, false);
        assert!(sync_recovery_policy(&mut config, "ana", &key, false, false).unwrap().pending, "leaving consent needs a client that shows the code");
        let left = sync_recovery_policy(&mut config, "ana", &key, false, true).unwrap();
        let back_code = left.new_code.expect("a new code");
        let wraps = config.users[0].key.clone().unwrap();
        assert_eq!(*member_crypto::unwrap_with_code(&wraps.by_recovery, &back_code).unwrap(), *key, "plain again: the code alone opens it");
    }

    #[test]
    fn replacing_the_owners_key_moves_members_over_without_asking_again() {
        let (mut config, old_secret, key, _) = ana_under(RecoveryPolicy::Company);
        let new_secret = set_recovery_policy(&mut config, RecoveryPolicy::Company, true).expect("a new pair");
        assert_ne!(old_secret, new_secret);
        let outcome = sync_recovery_policy(&mut config, "ana", &key, false, false).unwrap();
        assert_eq!(outcome, PolicyOutcome { in_step: true, pending: false, new_code: None }, "the same policy: not weaker, no code");
        assert!(recover_member(&mut config, "ana", &old_secret, None).is_err(), "the old key no longer opens it");
        assert!(recover_member(&mut config, "ana", &new_secret, None).is_ok());
    }

    #[test]
    fn a_new_code_under_consent_still_needs_the_owners_key() {
        let (mut config, owner_key, _key, _) = ana_under(RecoveryPolicy::Consent);
        let code = regenerate_recovery_code(&mut config, "ana", "anas-own-pass").unwrap();
        let wraps = config.users[0].key.clone().unwrap();
        assert!(member_crypto::unwrap_with_code(&wraps.by_recovery, &code).is_err(), "the new code alone opens nothing either");
        assert!(recover_member(&mut config, "ana", &owner_key, Some(&code)).is_ok());
    }

    #[test]
    fn after_an_owner_reset_a_consent_member_is_sent_to_the_owners_recovery() {
        let (mut config, owner_key, key, code) = ana_under(RecoveryPolicy::Consent);
        reset_password(&mut config, "ana", "provisional-2").unwrap();
        let err = change_password(&mut config, "ana", "provisional-2", "anas-third-pass", Some(&code)).map(|_| ()).unwrap_err();
        assert!(err.to_string().contains("recovery key"), "{err}");
        let recovered = recover_member(&mut config, "ana", &owner_key, Some(&code)).unwrap();
        assert!(!config.users[0].key_needs_recovery);
        let back = change_password(&mut config, "ana", &recovered.temp_password, "anas-third-pass", None).unwrap();
        assert_eq!(*back.key.unwrap(), *key);
    }

    #[test]
    fn the_policy_fields_round_trip_through_toml_and_add_nothing_under_private() {
        let (config, _, _, _) = ana_under(RecoveryPolicy::Company);
        let text = toml::to_string(&config).unwrap();
        assert!(text.contains("recovery_policy = \"company\"") && text.contains("recovery_public_key") && text.contains("by_escrow"));
        let back: FileConfig = toml::from_str(&text).unwrap();
        assert_eq!((back.recovery_policy, back.recovery_public_key.clone()), (config.recovery_policy, config.recovery_public_key.clone()));
        assert_eq!(back.users, config.users);

        let (private, _, _, _) = ana_under(RecoveryPolicy::Private);
        let plain = toml::to_string(&private).unwrap();
        assert!(!plain.contains("recovery_policy") && !plain.contains("by_escrow") && !plain.contains("recoveries") && !plain.contains("escrow_id"), "{plain}");
    }

    #[test]
    fn removing_a_member_with_encrypted_data_keeps_the_key_so_they_can_come_back() {
        let mut config = config_with_ana("provisional-1");
        add_user(&mut config, "bruno", "Bruno", "provisional-2").unwrap();
        let change = change_password(&mut config, "ana", "provisional-1", "anas-own-pass", None).unwrap();
        let key = change.key.unwrap();

        remove_user(&mut config, "ana").unwrap();
        remove_user(&mut config, "bruno").unwrap();
        assert!(config.users.is_empty());
        assert_eq!(config.removed_users.iter().map(|u| u.id.as_str()).collect::<Vec<_>>(), ["ana"], "only the one with encrypted data is kept");
        assert!(add_user(&mut config, "ana", "Another Ana", "provisional-3").is_err(), "a new person must not inherit her data folder");
        assert!(add_user(&mut config, "bruno", "Bruno again", "provisional-4").is_ok(), "a name with nothing to protect is free");

        restore_user(&mut config, "ana").unwrap();
        assert!(config.removed_users.is_empty());
        let ana = config.users.iter().find(|u| u.id == "ana").unwrap();
        assert_eq!(*open_key(ana, "anas-own-pass").unwrap().unwrap(), *key, "the same password opens the same data");
        assert!(restore_user(&mut config, "ana").is_err());

        remove_user(&mut config, "ana").unwrap();
        purge_removed_user(&mut config, "ana").unwrap();
        assert!(config.removed_users.is_empty() && purge_removed_user(&mut config, "ana").is_err());
        assert!(add_user(&mut config, "ana", "New Ana", "provisional-5").is_ok(), "purged: the name is free");
    }

    #[test]
    fn a_client_that_cannot_show_the_code_does_not_get_a_key_made() {
        let mut config = config_with_ana("provisional-1");
        let change = change_password_with(&mut config, "ana", "provisional-1", "anas-own-pass", None, false).unwrap();
        assert!(change.key.is_none() && change.new_recovery_code.is_none() && config.users[0].key.is_none());
        assert!(!config.users[0].must_change_password && authenticate_user(&config.users, "ana", "anas-own-pass").is_some(), "the password still changed");

        // A client that can gets it on the next change.
        let later = change_password_with(&mut config, "ana", "anas-own-pass", "anas-newer-pass", None, true).unwrap();
        assert!(later.key.is_some() && later.new_recovery_code.is_some());
    }

    #[test]
    fn a_member_from_before_gets_a_key_at_sign_in_and_a_new_recovery_code_replaces_the_old() {
        let mut config = FileConfig::default();
        add_user(&mut config, "ana", "Ana", "provisional-1").unwrap();
        config.users[0].must_change_password = false; // a member from before fatia 4, on their own password
        assert!(enable_encryption(&mut config, "ana", "wrong-password").is_err());
        let (key, old_code) = enable_encryption(&mut config, "ana", "provisional-1").unwrap().unwrap();
        assert!(enable_encryption(&mut config, "ana", "provisional-1").unwrap().is_none(), "only once");
        assert_eq!(*open_key(&config.users[0], "provisional-1").unwrap().unwrap(), *key);

        assert!(regenerate_recovery_code(&mut config, "ana", "wrong-password").is_err());
        let new_code = regenerate_recovery_code(&mut config, "ana", "provisional-1").unwrap();
        let wraps = config.users[0].key.clone().unwrap();
        assert_eq!(*member_crypto::unwrap_with_code(&wraps.by_recovery, &new_code).unwrap(), *key);
        assert!(member_crypto::unwrap_with_code(&wraps.by_recovery, &old_code).is_err(), "the old code stopped working");
    }

    #[test]
    fn a_user_with_a_key_round_trips_through_toml_and_never_shows_a_secret() {
        let mut config = config_with_ana("provisional-1");
        let change = change_password(&mut config, "ana", "provisional-1", "anas-own-pass", None).unwrap();
        let text = toml::to_string(&config).unwrap();
        assert!(!text.contains("anas-own-pass") && !text.contains(&change.new_recovery_code.unwrap()));
        assert_eq!(toml::from_str::<FileConfig>(&text).unwrap().users, config.users);
        let plain = toml::to_string(&config_with_ana("provisional-1")).unwrap();
        assert!(!plain.contains("by_password") && !plain.contains("key_needs_recovery"), "a member without a key adds no fields: {plain}");
    }

    fn agent(id: &str, owner: Option<&str>, shared_with: &[&str]) -> AgentConfig {
        AgentConfig {
            id: id.into(),
            persona: "p".into(),
            provider_id: None,
            can_delegate_to_agents: false,
            can_manage_agents: false,
            can_message_agents: false,
            can_manage_tasks: false,
            allowed_tools: None,
            autonomy: crate::default_autonomy(),
            approval_required: Vec::new(),
            role: None,
            reports_to: None,
            owner: owner.map(Into::into),
            shared_with: shared_with.iter().map(|s| s.to_string()).collect(),
            delegation_models: Vec::new(),
            can_start_tasks: true,
            can_create_workers: true,
        }
    }

    #[test]
    fn who_sees_which_agent() {
        let private = agent("mine", None, &[]);
        let for_ana = agent("home", None, &["ana"]);
        let for_all = agent("house", None, &[EVERYONE]);
        let anas = agent("anas", Some("ana"), &[]);
        for (agent, owner, ana, bruno) in [(&private, true, false, false), (&for_ana, true, true, false), (&for_all, true, true, true), (&anas, false, true, false)] {
            assert_eq!(agent_visible_to(agent, None), owner, "{} for the owner", agent.id);
            assert_eq!(agent_visible_to(agent, Some("ana")), ana, "{} for Ana", agent.id);
            assert_eq!(agent_visible_to(agent, Some("bruno")), bruno, "{} for Bruno", agent.id);
        }
    }

    #[test]
    fn a_members_tools_are_the_default_or_the_owners_list_never_the_forbidden_ones() {
        let hub: Vec<String> = ["read_file", "write_file", "shell", "tavily-search", "delegate_to_agent", "github__issue", "usage_stats"].map(String::from).to_vec();
        let mut config = FileConfig::default();
        add_user(&mut config, "ana", "Ana", "temporary-1").unwrap();
        assert_eq!(member_tools(&config.users[0], &hub), ["read_file", "write_file", "tavily-search"]);
        set_user_tools(&mut config, "ana", Some(vec!["shell".into(), "github__issue".into(), "delegate_to_agent".into(), "not-here".into()])).unwrap();
        assert_eq!(config.users[0].tools.as_deref(), Some(&["github__issue".to_string(), "not-here".to_string(), "shell".to_string()][..]));
        assert_eq!(member_tools(&config.users[0], &hub), ["shell", "github__issue"], "a name the hub lacks is ignored");
        set_user_tools(&mut config, "ana", None).unwrap();
        assert_eq!(member_tools(&config.users[0], &hub), ["read_file", "write_file", "tavily-search"]);
    }

    #[test]
    fn a_member_sees_no_organization_until_the_owner_says_so_and_the_choice_survives_the_file() {
        let mut config = FileConfig::default();
        add_user(&mut config, "ana", "Ana", "temporary-1").unwrap();
        let access = |config: &FileConfig| config.users[0].org_access;
        assert_eq!(access(&config), OrgAccess::None);
        assert!(!access(&config).can_view() && !access(&config).can_edit());

        set_user_org_access(&mut config, "ana", "view").unwrap();
        assert!(access(&config).can_view() && !access(&config).can_edit(), "seeing is not changing");
        set_user_org_access(&mut config, "ana", "edit").unwrap();
        assert!(access(&config).can_view() && access(&config).can_edit());

        assert!(set_user_org_access(&mut config, "ana", "admin").is_err(), "only none, view or edit");
        assert!(set_user_org_access(&mut config, "nobody", "view").is_err(), "a member that exists");
        assert_eq!(access(&config), OrgAccess::Edit, "a refused change leaves the access as it was");

        let text = toml::to_string(&config).unwrap();
        assert!(text.contains("org_access = \"edit\""), "{text}");
        assert_eq!(toml::from_str::<FileConfig>(&text).unwrap().users[0].org_access, OrgAccess::Edit);
        set_user_org_access(&mut config, "ana", "none").unwrap();
        assert!(!toml::to_string(&config).unwrap().contains("org_access"), "none writes nothing, as it never existed");
    }

    #[test]
    fn the_owner_picks_a_members_learning_model_among_the_hubs_providers_and_combos() {
        let provider = |id: &str| crate::ProviderConfig { id: id.to_string(), kind: crate::Provider::Gemini, api_key: None, base_url: None, model: None, node: None };
        let mut config = FileConfig { providers: vec![provider("cheap")], combos: vec![crate::ComboConfig { id: "any".into(), providers: vec!["cheap".into()] }], ..FileConfig::default() };
        add_user(&mut config, "ana", "Ana", "temporary-1").unwrap();

        set_user_learning_provider(&mut config, "ana", Some(" cheap ".into())).unwrap();
        assert_eq!(config.users[0].learning_provider.as_deref(), Some("cheap"));
        set_user_learning_provider(&mut config, "ana", Some("any".into())).unwrap();
        assert_eq!(config.users[0].learning_provider.as_deref(), Some("any"), "a combo counts");

        assert!(set_user_learning_provider(&mut config, "ana", Some("ghost".into())).is_err());
        assert!(set_user_learning_provider(&mut config, "nobody", Some("cheap".into())).is_err());
        assert_eq!(config.users[0].learning_provider.as_deref(), Some("any"), "a refusal changes nothing");

        set_user_learning_provider(&mut config, "ana", Some("  ".into())).unwrap();
        assert_eq!(config.users[0].learning_provider, None, "blank goes back to the workspace's");
        set_user_learning_provider(&mut config, "ana", Some("cheap".into())).unwrap();
        set_user_learning_provider(&mut config, "ana", None).unwrap();
        assert_eq!(config.users[0].learning_provider, None);
    }

    #[test]
    fn a_member_keeps_their_own_agents_to_themselves() {
        let hub: Vec<String> = ["read_file", "shell"].map(String::from).to_vec();
        let mut config = FileConfig { agents: vec![agent("house", None, &["ana"])], ..FileConfig::default() };
        add_user(&mut config, "ana", "Ana", "temporary-1").unwrap();
        add_user(&mut config, "bruno", "Bruno", "temporary-2").unwrap();

        // Whatever she asks for, it stays hers, unshared, powerless, within her tools.
        let wish = AgentConfig { can_manage_agents: true, allowed_tools: Some(vec!["read_file".into(), "shell".into()]), ..agent("cook", None, &[EVERYONE]) };
        save_member_agent(&mut config, "ana", None, wish, &hub).unwrap();
        let cook = config.agents.iter().find(|a| a.id == "cook").unwrap();
        assert_eq!((cook.owner.as_deref(), cook.can_manage_agents, cook.shared_with.len()), (Some("ana"), false, 0));
        assert_eq!(cook.allowed_tools.as_deref(), Some(&["read_file".to_string()][..]));

        assert!(save_member_agent(&mut config, "bruno", None, agent("cook", None, &[]), &hub).is_err(), "the name is taken");
        assert!(save_member_agent(&mut config, "bruno", Some("cook"), agent("cook", None, &[]), &hub).is_err(), "not his to edit");
        assert!(delete_member_agent(&mut config, "bruno", "cook").is_err(), "not his to delete");
        assert!(delete_member_agent(&mut config, "ana", "house").is_err(), "the owner's agent isn't hers");
        save_member_agent(&mut config, "ana", Some("cook"), agent("chef", None, &[]), &hub).unwrap();
        assert!(config.agents.iter().any(|a| a.id == "chef" && a.owner.as_deref() == Some("ana")), "renamed");

        // Removing her takes her agents and her shares along.
        remove_user(&mut config, "ana").unwrap();
        assert_eq!(config.agents.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(), ["house"]);
        assert!(config.agents[0].shared_with.is_empty());
    }

    #[test]
    fn shares_keep_known_people_and_everyone_wins() {
        let mut config = FileConfig::default();
        add_user(&mut config, "ana", "Ana", "temporary-1").unwrap();
        assert_eq!(clean_shares(vec![" ANA ".into(), "ghost".into(), "ana".into()], &config.users), ["ana"]);
        assert_eq!(clean_shares(vec!["ana".into(), EVERYONE.into()], &config.users), [EVERYONE]);
    }

    #[test]
    fn spaces_are_safe_folders_shared_with_people_who_exist() {
        let mut config = FileConfig::default();
        add_user(&mut config, "ana", "Ana", "temporary-1").unwrap();
        add_user(&mut config, "bruno", "Bruno", "temporary-2").unwrap();
        let space = |id: &str, folder: &str, readers: &[&str], writers: &[&str]| SpaceConfig {
            id: id.into(),
            folder: folder.into(),
            readers: readers.iter().map(|s| s.to_string()).collect(),
            writers: writers.iter().map(|s| s.to_string()).collect(),
        };
        for folder in ["../out", ".warden", "skills/x", "", "a/../b"] {
            assert!(save_space(&mut config, None, space("x", folder, &[], &[])).is_err(), "{folder}");
        }
        save_space(&mut config, None, space(" Casa ", "/casa/", &["ana", "ghost", "bruno"], &["bruno"])).unwrap();
        assert_eq!(config.spaces[0], space("casa", "casa", &["ana"], &["bruno"]), "trimmed, unknown people dropped, a writer isn't listed twice");
        assert!(save_space(&mut config, None, space("casa", "other", &[], &[])).is_err(), "name taken");
        assert!(save_space(&mut config, None, space("home", "casa", &[], &[])).is_err(), "folder taken");
        save_space(&mut config, None, space("viagens", "viagens/2026", &["*"], &[])).unwrap();

        let ana: Vec<(&str, bool)> = spaces_for(&config.spaces, "ana").into_iter().map(|(s, w)| (s.id.as_str(), w)).collect();
        assert_eq!(ana, [("casa", false), ("viagens", false)]);
        let bruno: Vec<(&str, bool)> = spaces_for(&config.spaces, "bruno").into_iter().map(|(s, w)| (s.id.as_str(), w)).collect();
        assert_eq!(bruno, [("casa", true), ("viagens", false)]);

        remove_user(&mut config, "bruno").unwrap();
        assert!(config.spaces[0].writers.is_empty());
        remove_space(&mut config, "casa").unwrap();
        assert_eq!(config.spaces.len(), 1);
    }

    #[test]
    fn users_go_through_the_config_file_and_back() {
        let dir = std::env::temp_dir().join(format!("warden-users-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let path = dir.join("config.toml");
        let mut config = FileConfig::default();
        add_user(&mut config, "ana", "Ana", "temporary-1").unwrap();
        crate::save_config(&path, &config).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("[[users]]") && !text.contains("temporary-1"), "{text}");
        assert_eq!(crate::load_config_from_path(&path, true).unwrap().users, config.users);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_member_can_opt_out_of_learning_but_not_opt_in_past_the_workspace() {
        let mut config = FileConfig::default();
        add_user(&mut config, "ana", "Ana", "temp-pass-1").unwrap();
        assert!(!learning_allowed(&config, Some("ana")), "off in the workspace: off for everyone");

        config.learning.enabled = true;
        assert!(learning_allowed(&config, Some("ana")) && learning_allowed(&config, None));

        set_learning_opt_out(&mut config, "ana", true).unwrap();
        assert!(!learning_allowed(&config, Some("ana")), "she opted out");
        assert!(learning_allowed(&config, None), "the owner follows the workspace");
        assert!(set_learning_opt_out(&mut config, "nobody", true).is_err());

        set_learning_opt_out(&mut config, "ana", false).unwrap();
        assert!(learning_allowed(&config, Some("ana")));
    }
}
