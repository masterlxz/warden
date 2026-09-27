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

use crate::FileConfig;

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

/// Takes the member out of the file. Their vault and conversations stay on disk.
pub fn remove_user(config: &mut FileConfig, id: &str) -> anyhow::Result<()> {
    let i = config.users.iter().position(|u| u.id == id).ok_or_else(|| anyhow::anyhow!("no user named '{id}'"))?;
    config.users.remove(i);
    Ok(())
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
