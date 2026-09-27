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
}

/// Shares an agent with every member (`AgentConfig::shared_with`).
pub const EVERYONE: &str = "*";

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
    Ok(())
}

/// The member picks their own password; `old` has to be the current one.
pub fn change_password(config: &mut FileConfig, id: &str, old: &str, new: &str) -> anyhow::Result<()> {
    let user = find_mut(config, id)?;
    anyhow::ensure!(verify_password(&user.password_hash, old), "the current password is wrong");
    anyhow::ensure!(old != new, "pick a password different from the current one");
    user.password_hash = hash_password(new)?;
    user.must_change_password = false;
    Ok(())
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

        assert!(change_password(&mut config, "ana", "wrong-password", "her own pass").is_err());
        change_password(&mut config, "ana", &temp, "her own pass").unwrap();
        assert!(!config.users[0].must_change_password);
        assert!(authenticate_user(&config.users, "ana", "her own pass").is_some());

        reset_password(&mut config, "ana", "a new temp pw").unwrap();
        assert!(config.users[0].must_change_password && authenticate_user(&config.users, "ana", "her own pass").is_none());
        rename_user(&mut config, "ana", "Ana S.").unwrap();
        remove_user(&mut config, "ana").unwrap();
        assert!(config.users.is_empty() && remove_user(&mut config, "ana").is_err());
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
