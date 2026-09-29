//! The people of the workspace (P84, fatia 1). The root is whoever holds the hub's pairing key, as
//! before; `[[users]]` in `config.toml` lists the **members** the root created. A member pairs a
//! device with their username and password instead of the key, and gets their own vault and
//! conversations. No `[[users]]` at all is the single-person Warden, unchanged.
//!
//! Passwords are only ever stored as an Argon2id hash (PHC string). The file syncs between the
//! root's machines, so the hashes travel with it — the root's choice (Sessão 110).

use std::path::{Path, PathBuf};

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use rand::Rng;
use serde::{Deserialize, Serialize};

use crate::member_crypto::{self, MemberKey};
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
}

/// One key, wrapped twice (`member_crypto`): whoever knows the password, or the recovery code,
/// opens it. Base64 text; neither the key nor either secret is in it.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct KeyWraps {
    pub by_password: String,
    pub by_recovery: String,
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
    anyhow::ensure!(!warden_core::memory::FIXED_VAULT_FILES.contains(&folder), "'{folder}' is one of the fixed memory files, not a folder");
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
/// `manage_tasks`), or the whole hub's spending (`usage_stats`).
pub const NEVER_FOR_MEMBERS: &[&str] = &["delegate_to_agent", "message_agent", "manage_agents", "manage_tasks", "usage_stats"];

/// The tools a member has when the owner never set a list: their own vault's files and skills, a
/// sub-agent (rebound to their vault too), background jobs, their spending, documents, and web
/// search. Everything else reaches what's the owner's (the hub's shell, SSH hosts, nodes, the MCP
/// servers set up with the owner's accounts) and waits for the owner to grant it.
pub fn default_member_tool(name: &str) -> bool {
    matches!(name, "read_file" | "write_file" | "use_skill" | "read_skill_file" | "manage_skill" | "delegate_task" | "jobs" | "budget" | "generate_document")
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
    };
    let mut users = config.users.clone();
    anyhow::ensure!(!users.iter().any(|u| u.id == user.id), "there's already a user named '{}'", user.id);
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
    let user = find_mut(config, id)?;
    anyhow::ensure!(verify_password(&user.password_hash, old), "the current password is wrong");
    anyhow::ensure!(old != new, "pick a password different from the current one");
    let hash = hash_password(new)?;
    // Everything that can fail comes before anything changes.
    let (key, wraps, new_recovery_code) = match &user.key {
        None if !create_key => (None, None, None),
        None => {
            let (key, wraps, code) = new_key_wraps(new)?;
            (Some(key), Some(wraps), Some(code))
        }
        Some(current) if user.key_needs_recovery => {
            let code = recovery_code.filter(|c| !c.trim().is_empty()).ok_or_else(|| anyhow::anyhow!("the owner reset your password: give your recovery code to keep your data"))?;
            let key = member_crypto::unwrap_with_code(&current.by_recovery, code)?;
            let wraps = KeyWraps { by_password: member_crypto::wrap_with_password(&key, new)?, by_recovery: current.by_recovery.clone() };
            (Some(key), Some(wraps), None)
        }
        Some(current) => {
            let key = member_crypto::unwrap_with_password(&current.by_password, old)?;
            let wraps = KeyWraps { by_password: member_crypto::wrap_with_password(&key, new)?, by_recovery: current.by_recovery.clone() };
            (Some(key), Some(wraps), None)
        }
    };
    user.password_hash = hash;
    user.must_change_password = false;
    user.key = wraps;
    user.key_needs_recovery = false;
    Ok(PasswordChange { key, new_recovery_code })
}

/// A new key, wrapped by `password` and by a new recovery code (returned, to be shown once).
fn new_key_wraps(password: &str) -> anyhow::Result<(MemberKey, KeyWraps, String)> {
    let key = member_crypto::new_key();
    let code = member_crypto::generate_recovery_code();
    let wraps = KeyWraps { by_password: member_crypto::wrap_with_password(&key, password)?, by_recovery: member_crypto::wrap_with_code(&key, &code)? };
    Ok((key, wraps, code))
}

/// A member from before fatia 4, signing in with their own password: turns encryption on with a
/// new key and recovery code. `None` when there's nothing to do (they have a key, or still have
/// the owner's provisional password — `change_password` creates it then).
pub fn enable_encryption(config: &mut FileConfig, id: &str, password: &str) -> anyhow::Result<Option<(MemberKey, String)>> {
    let user = find_mut(config, id)?;
    if user.key.is_some() || user.must_change_password {
        return Ok(None);
    }
    anyhow::ensure!(verify_password(&user.password_hash, password), "the current password is wrong");
    let (key, wraps, code) = new_key_wraps(password)?;
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
    let user = find_mut(config, id)?;
    anyhow::ensure!(verify_password(&user.password_hash, password), "the current password is wrong");
    let wraps = user.key.as_ref().filter(|_| !user.key_needs_recovery).ok_or_else(|| anyhow::anyhow!("your data isn't encrypted with a key of yours yet"))?;
    let key = member_crypto::unwrap_with_password(&wraps.by_password, password)?;
    let code = member_crypto::generate_recovery_code();
    let by_recovery = member_crypto::wrap_with_code(&key, &code)?;
    user.key = Some(KeyWraps { by_password: wraps.by_password.clone(), by_recovery });
    Ok(code)
}

/// Takes the member out of the file, with their own agents and every share naming them. Their vault
/// and conversations stay on disk.
pub fn remove_user(config: &mut FileConfig, id: &str) -> anyhow::Result<()> {
    let i = config.users.iter().position(|u| u.id == id).ok_or_else(|| anyhow::anyhow!("no user named '{id}'"))?;
    config.users.remove(i);
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
        owner: Some(owner.to_string()),
        shared_with: Vec::new(),
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
            owner: owner.map(Into::into),
            shared_with: shared_with.iter().map(|s| s.to_string()).collect(),
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
        for folder in ["../out", ".warden", "skills/x", "_profile.md", "", "a/../b"] {
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
}
