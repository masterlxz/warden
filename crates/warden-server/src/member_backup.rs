//! Backup and restore of the members' data (P84, fatia 4).
//!
//! A member's data is already encrypted on the hub's disk, so a backup is a copy: the owner can
//! make one, keep it anywhere, and still can't read a byte of it. What makes it worth having is the
//! second file: `members.toml` holds each member's entry from `config.toml` — their key wrapped by
//! the password and by the recovery code — so a backup restored on another machine, or after the
//! member was removed by mistake, opens with the member's password or recovery code and nothing else.
//!
//! Layout of a backup folder:
//!
//! ```text
//! members.toml                 [[users]] of every member in it
//! users/<id>/…                 their folder (the vault, the generated files, the encrypted marker)
//! conversations/<id>/…         their conversations
//! ```
//!
//! A member whose data isn't encrypted yet (from before fatia 4, and who hasn't signed in since) is
//! left out and reported: a copy of it would be readable, so it isn't made without saying so.

use std::path::Path;

use anyhow::Context;
use serde::{Deserialize, Serialize};
use warden_bootstrap::member_crypto;
use warden_bootstrap::users::{user_conversations_dir, UserConfig};
use warden_bootstrap::{load_config_from_path, save_config};

const MEMBERS_FILE: &str = "members.toml";

#[derive(Serialize, Deserialize, Default)]
struct BackupFile {
    #[serde(default)]
    users: Vec<UserConfig>,
}

/// What a backup did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct BackupReport {
    pub backed_up: Vec<String>,
    /// Members left out, each with the reason.
    pub skipped: Vec<(String, String)>,
}

/// Copies the encrypted data of the members (all, or just `only`) into `out`, with their entries
/// from the config file. Refuses a folder that already holds a backup.
pub fn backup_members(config_path: &Path, users_dir: &Path, conversations_root: &Path, out: &Path, only: Option<&str>) -> anyhow::Result<BackupReport> {
    let config = load_config_from_path(config_path, false)?;
    let wanted: Vec<&UserConfig> = match only {
        Some(id) => vec![config.users.iter().find(|u| u.id == id).ok_or_else(|| anyhow::anyhow!("no member named '{id}'"))?],
        None => config.users.iter().collect(),
    };
    anyhow::ensure!(!out.join(MEMBERS_FILE).exists(), "{} already holds a backup — pick an empty folder", out.display());

    let mut report = BackupReport::default();
    let mut file = BackupFile::default();
    for user in wanted {
        let folder = users_dir.join(&user.id);
        if user.key.is_none() || member_crypto::needs_migration(&folder) {
            report.skipped.push((user.id.clone(), "their data isn't encrypted yet — it is once they sign in with their password".to_string()));
            continue;
        }
        copy_tree(&folder, &out.join("users").join(&user.id))?;
        copy_tree(&user_conversations_dir(conversations_root, &user.id), &out.join("conversations").join(&user.id))?;
        file.users.push(user.clone());
        report.backed_up.push(user.id.clone());
    }
    std::fs::create_dir_all(out).with_context(|| format!("failed to create {}", out.display()))?;
    std::fs::write(out.join(MEMBERS_FILE), toml::to_string_pretty(&file)?).with_context(|| format!("failed to write {}", out.join(MEMBERS_FILE).display()))?;
    Ok(report)
}

/// Puts members from a backup back: their data on disk, and — for one that's gone from the config
/// file, say removed by mistake — their entry. A member that's still there with the same key just
/// gets their data back; one whose key differs (the backup is from another life of that name) is
/// only replaced with `force`. Existing data is only overwritten with `force`.
pub fn restore_members(config_path: &Path, users_dir: &Path, conversations_root: &Path, from: &Path, only: Option<&str>, force: bool) -> anyhow::Result<Vec<String>> {
    let text = std::fs::read_to_string(from.join(MEMBERS_FILE)).with_context(|| format!("{} isn't a backup: there's no {MEMBERS_FILE} in it", from.display()))?;
    let file: BackupFile = toml::from_str(&text).with_context(|| format!("{MEMBERS_FILE} in the backup is damaged"))?;
    let chosen: Vec<&UserConfig> = file.users.iter().filter(|u| only.is_none_or(|id| u.id == id)).collect();
    anyhow::ensure!(!chosen.is_empty(), "{}", match only {
        Some(id) => format!("the backup has no member named '{id}'"),
        None => "the backup has no members".to_string(),
    });

    let mut config = load_config_from_path(config_path, false)?;
    // Checked for everyone before anything is copied, so a refusal leaves nothing half done.
    for user in &chosen {
        if let Some(current) = config.users.iter().find(|u| u.id == user.id) {
            let same_key = current.key.as_ref().zip(user.key.as_ref()).is_some_and(|(a, b)| a.by_recovery == b.by_recovery);
            anyhow::ensure!(same_key || force, "'{}' is in the config file with another key than the backup's — use --force to replace them with the backup's entry", user.id);
        }
        let taken = users_dir.join(&user.id).exists() || user_conversations_dir(conversations_root, &user.id).exists();
        anyhow::ensure!(!taken || force, "'{}' already has data on this hub — use --force to replace it with the backup's", user.id);
    }
    let mut restored = Vec::new();
    for user in chosen {
        for (from_dir, to_dir) in [(from.join("users").join(&user.id), users_dir.join(&user.id)), (from.join("conversations").join(&user.id), user_conversations_dir(conversations_root, &user.id))] {
            if to_dir.exists() {
                std::fs::remove_dir_all(&to_dir).with_context(|| format!("failed to clear {}", to_dir.display()))?;
            }
            if from_dir.exists() {
                copy_tree(&from_dir, &to_dir)?;
            }
        }
        match config.users.iter().position(|u| u.id == user.id) {
            Some(i) if config.users[i].key.as_ref().zip(user.key.as_ref()).is_some_and(|(a, b)| a.by_recovery == b.by_recovery) => {}
            Some(i) => config.users[i] = user.clone(),
            None => config.users.push(user.clone()),
        }
        // Back among the members: no longer one that was removed.
        config.removed_users.retain(|u| u.id != user.id);
        restored.push(user.id.clone());
    }
    save_config(config_path, &config)?;
    Ok(restored)
}

/// Copies a folder and everything in it, leaving symlinks out. A folder that isn't there is an
/// empty one.
fn copy_tree(from: &Path, to: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(to).with_context(|| format!("failed to create {}", to.display()))?;
    let Ok(entries) = std::fs::read_dir(from) else { return Ok(()) };
    for entry in entries {
        let entry = entry?;
        let target = to.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target).with_context(|| format!("failed to copy {}", entry.path().display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use warden_bootstrap::users::{add_user, change_password};
    use warden_bootstrap::FileConfig;

    struct Setup {
        dir: std::path::PathBuf,
        config_path: std::path::PathBuf,
        users_dir: std::path::PathBuf,
        conversations: std::path::PathBuf,
        code: String,
    }

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("warden-backup-{tag}-{}-{n}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Ana, encrypted, with a note and a conversation on disk; Bruno, who never signed in since
    /// before fatia 4 (no key, plain data).
    fn setup() -> Setup {
        let dir = temp_dir("hub");
        let (config_path, users_dir, conversations) = (dir.join("config.toml"), dir.join("users"), dir.join("conversations"));
        let mut config = FileConfig::default();
        add_user(&mut config, "ana", "Ana", "provisional-1").unwrap();
        add_user(&mut config, "bruno", "Bruno", "provisional-2").unwrap();
        let change = change_password(&mut config, "ana", "provisional-1", "anas-own-pass", None).unwrap();
        save_config(&config_path, &config).unwrap();

        let ana_conversations = user_conversations_dir(&conversations, "ana");
        let key = change.key.unwrap();
        // A document her agent made before her data was encrypted: the migration seals it too.
        std::fs::create_dir_all(users_dir.join("ana/generated")).unwrap();
        std::fs::write(users_dir.join("ana/generated/relatorio.txt"), "relatório em texto simples").unwrap();
        member_crypto::encrypt_member_data(&users_dir.join("ana"), &ana_conversations, &key).unwrap();
        let vault = warden_core::memory::Vault::new_encrypted(users_dir.join("ana/vault"), std::sync::Arc::new(warden_core::memory::VaultCipher::new(&key)));
        vault.write("notes/segredo.md", "o feijão da ana").unwrap();
        std::fs::create_dir_all(users_dir.join("bruno/vault")).unwrap();
        std::fs::write(users_dir.join("bruno/vault/plain.md"), "texto simples do bruno").unwrap();
        Setup { dir, config_path, users_dir, conversations, code: change.new_recovery_code.unwrap() }
    }

    fn everything(dir: &Path) -> String {
        let mut seen = String::new();
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            seen.push_str(&path.file_name().unwrap().to_string_lossy());
            seen.push_str(&if path.is_dir() { everything(&path) } else { String::from_utf8_lossy(&std::fs::read(&path).unwrap()).to_string() });
        }
        seen
    }

    #[test]
    fn a_backup_holds_the_encrypted_members_and_nothing_readable() {
        let s = setup();
        let out = temp_dir("out");
        let report = backup_members(&s.config_path, &s.users_dir, &s.conversations, &out, None).unwrap();
        assert_eq!(report.backed_up, ["ana"]);
        assert_eq!(report.skipped.len(), 1);
        assert!(report.skipped[0].0 == "bruno" && report.skipped[0].1.contains("isn't encrypted yet"), "{report:?}");

        let contents = everything(&out);
        for secret in ["feijão", "segredo", "notes", "anas-own-pass", "texto simples", "relatório"] {
            assert!(!contents.contains(secret), "'{secret}' is readable in the backup");
        }
        assert!(!contents.contains(&s.code), "the recovery code itself isn't in it");
        assert!(backup_members(&s.config_path, &s.users_dir, &s.conversations, &out, None).is_err(), "won't write over a backup");
        assert!(backup_members(&s.config_path, &s.users_dir, &s.conversations, &temp_dir("nobody"), Some("carla")).is_err());
    }

    #[test]
    fn a_member_removed_by_mistake_comes_back_from_a_backup_and_opens_with_her_password() {
        let s = setup();
        let out = temp_dir("out");
        backup_members(&s.config_path, &s.users_dir, &s.conversations, &out, Some("ana")).unwrap();

        // Ana is removed, and her folder is lost with the disk it was on.
        let mut config = load_config_from_path(&s.config_path, false).unwrap();
        warden_bootstrap::users::remove_user(&mut config, "ana").unwrap();
        save_config(&s.config_path, &config).unwrap();
        std::fs::remove_dir_all(s.users_dir.join("ana")).unwrap();
        std::fs::remove_dir_all(user_conversations_dir(&s.conversations, "ana")).unwrap();
        member_crypto::lock(&[&s.users_dir.join("ana"), &user_conversations_dir(&s.conversations, "ana")]);

        assert_eq!(restore_members(&s.config_path, &s.users_dir, &s.conversations, &out, None, false).unwrap(), ["ana"]);
        let config = load_config_from_path(&s.config_path, false).unwrap();
        assert!(config.removed_users.is_empty(), "and she's no longer listed as removed");
        let ana = config.users.iter().find(|u| u.id == "ana").expect("her entry is back");
        assert!(warden_bootstrap::users::authenticate_user(&config.users, "ana", "anas-own-pass").is_some());
        let key = warden_bootstrap::users::open_key(ana, "anas-own-pass").unwrap().unwrap();
        let vault = warden_core::memory::Vault::new_encrypted(s.users_dir.join("ana/vault"), std::sync::Arc::new(warden_core::memory::VaultCipher::new(&key)));
        assert_eq!(vault.read("notes/segredo.md").unwrap(), "o feijão da ana");

        // Or with nothing but the recovery code.
        let by_code = member_crypto::unwrap_with_code(&ana.key.as_ref().unwrap().by_recovery, &s.code).unwrap();
        assert_eq!(*by_code, *key);
    }

    #[test]
    fn a_restore_refuses_to_overwrite_data_or_another_key_unless_forced() {
        let s = setup();
        let out = temp_dir("out");
        backup_members(&s.config_path, &s.users_dir, &s.conversations, &out, Some("ana")).unwrap();
        let err = restore_members(&s.config_path, &s.users_dir, &s.conversations, &out, None, false).unwrap_err();
        assert!(err.to_string().contains("already has data"), "{err}");

        // Another Ana with another key is in the file now.
        let mut config = load_config_from_path(&s.config_path, false).unwrap();
        warden_bootstrap::users::remove_user(&mut config, "ana").unwrap();
        warden_bootstrap::users::purge_removed_user(&mut config, "ana").unwrap(); // the name is reserved until then
        add_user(&mut config, "ana", "Ana again", "provisional-3").unwrap();
        change_password(&mut config, "ana", "provisional-3", "another-password", None).unwrap();
        save_config(&s.config_path, &config).unwrap();
        let err = restore_members(&s.config_path, &s.users_dir, &s.conversations, &out, None, true);
        assert!(err.is_ok(), "forced, it replaces the entry with the backup's");
        let config = load_config_from_path(&s.config_path, false).unwrap();
        assert!(warden_bootstrap::users::authenticate_user(&config.users, "ana", "anas-own-pass").is_some());

        assert!(restore_members(&s.config_path, &s.users_dir, &s.conversations, &temp_dir("empty"), None, true).is_err(), "not a backup");
        assert!(restore_members(&s.config_path, &s.users_dir, &s.conversations, &out, Some("carla"), true).is_err());
        let _ = &s.dir;
    }
}
