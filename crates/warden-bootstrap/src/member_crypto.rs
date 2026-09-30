//! Encryption of a member's data at rest (P84, fatia 4): the key, how it's kept, and what is done
//! with the files already on disk.
//!
//! Each member has one random 32-byte key. It never touches the disk in the clear: `config.toml`
//! holds it twice, wrapped by the member's password and by their recovery code, so either opens it
//! and nobody else can — not even the root, who only ever sees the provisional password. The key
//! is created from the member's *own* password (never the provisional one, or the root could open
//! it) and lives in the hub's memory from sign-in until the process ends: a device that only sends
//! its token, or a Warden API key, never has the password, so the hub holds the key for them.
//!
//! What it covers: the vault (contents and file names), the semantic index and the member's
//! conversations. Not the generated documents (see PENDING P108).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use anyhow::Context;
use argon2::Argon2;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use rand::RngCore;
use warden_core::memory::{base32_decode, base32_encode, VaultCipher, MAX_NAME_BYTES};
use zeroize::Zeroizing;

pub type MemberKey = Zeroizing<[u8; 32]>;

/// A folder holding this file is encrypted: read it only with the member's key.
pub const MARKER: &str = ".encrypted";
/// Present in a member's folder while `encrypt_member_data` is running, and left behind if it was
/// cut short — so the next sign-in finishes the job.
const UNFINISHED: &str = ".encrypting";
const SALT_LEN: usize = 16;
const CODE_BYTES: usize = 20;

pub fn new_key() -> MemberKey {
    let mut key = Zeroizing::new([0u8; 32]);
    rand::rngs::OsRng.fill_bytes(&mut *key);
    key
}

fn derive_kek(secret: &[u8], salt: &[u8]) -> anyhow::Result<MemberKey> {
    let mut kek = Zeroizing::new([0u8; 32]);
    Argon2::default().hash_password_into(secret, salt, &mut *kek).map_err(|e| anyhow::anyhow!("failed to derive a key: {e}"))?;
    Ok(kek)
}

/// `key` sealed by a key derived from `secret`, as base64 text (`salt` then the sealed key).
fn wrap(key: &MemberKey, secret: &[u8]) -> anyhow::Result<String> {
    let mut salt = [0u8; SALT_LEN];
    rand::rngs::OsRng.fill_bytes(&mut salt);
    let kek = derive_kek(secret, &salt)?;
    let sealed = VaultCipher::new(&kek).seal(&key[..]);
    Ok(BASE64.encode([&salt[..], &sealed].concat()))
}

fn unwrap(wrapped: &str, secret: &[u8]) -> anyhow::Result<MemberKey> {
    let bytes = BASE64.decode(wrapped.trim()).context("the stored key is damaged")?;
    anyhow::ensure!(bytes.len() > SALT_LEN, "the stored key is damaged");
    let (salt, sealed) = bytes.split_at(SALT_LEN);
    let kek = derive_kek(secret, salt)?;
    let opened = Zeroizing::new(VaultCipher::new(&kek).open(sealed)?);
    let mut key = Zeroizing::new([0u8; 32]);
    anyhow::ensure!(opened.len() == key.len(), "the stored key is damaged");
    key.copy_from_slice(&opened);
    Ok(key)
}

pub fn wrap_with_password(key: &MemberKey, password: &str) -> anyhow::Result<String> {
    wrap(key, password.as_bytes())
}

/// Fails for a wrong password.
pub fn unwrap_with_password(wrapped: &str, password: &str) -> anyhow::Result<MemberKey> {
    unwrap(wrapped, password.as_bytes()).map_err(|_| anyhow::anyhow!("the password doesn't open this member's data"))
}

/// A new recovery code: 160 random bits spelled `ABCD-EFGH-…`, to be written down. Shown once.
pub fn generate_recovery_code() -> String {
    let mut bytes = [0u8; CODE_BYTES];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let text = base32_encode(&bytes).to_ascii_uppercase();
    text.as_bytes().chunks(4).map(|c| std::str::from_utf8(c).unwrap_or("")).collect::<Vec<_>>().join("-")
}

/// The code's bytes, whatever the case or the dashes and spaces it was typed with.
fn code_bytes(code: &str) -> anyhow::Result<Vec<u8>> {
    let clean: String = code.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase();
    match base32_decode(&clean) {
        Some(bytes) if bytes.len() == CODE_BYTES => Ok(bytes),
        _ => anyhow::bail!("that is not a recovery code"),
    }
}

pub fn wrap_with_code(key: &MemberKey, code: &str) -> anyhow::Result<String> {
    wrap(key, &code_bytes(code)?)
}

/// Fails for a wrong code.
pub fn unwrap_with_code(wrapped: &str, code: &str) -> anyhow::Result<MemberKey> {
    unwrap(wrapped, &code_bytes(code)?).map_err(|_| anyhow::anyhow!("that recovery code doesn't open this member's data"))
}

// ---- keys in use -------------------------------------------------------------------------------

/// The keys the hub holds right now, by the folder they open. Two folders per member (their vault's
/// folder and their conversations'), so the plain file functions can find the key from the folder
/// alone, without threading it through every caller.
static KEYRING: RwLock<Option<HashMap<PathBuf, Arc<VaultCipher>>>> = RwLock::new(None);

/// What a member's folder needs.
#[derive(Clone)]
pub enum DirState {
    /// Never encrypted (the owner's, or a member from before fatia 4 who hasn't signed in since).
    Plain,
    Unlocked(Arc<VaultCipher>),
    /// Encrypted, and the hub doesn't hold the key.
    Locked,
}

pub fn dir_state(dir: &Path) -> DirState {
    if !dir.join(MARKER).exists() {
        return DirState::Plain;
    }
    let ring = KEYRING.read().unwrap_or_else(|e| e.into_inner());
    match ring.as_ref().and_then(|ring| ring.get(dir)) {
        Some(cipher) => DirState::Unlocked(cipher.clone()),
        None => DirState::Locked,
    }
}

/// From now on the hub can open these folders.
pub fn unlock(dirs: &[&Path], key: &MemberKey) {
    let cipher = Arc::new(VaultCipher::new(key));
    let mut ring = KEYRING.write().unwrap_or_else(|e| e.into_inner());
    let ring = ring.get_or_insert_with(HashMap::new);
    for dir in dirs {
        ring.insert(dir.to_path_buf(), cipher.clone());
    }
}

/// The hub forgets the key: the folders are locked until the member signs in again.
pub fn lock(dirs: &[&Path]) {
    if let Some(ring) = KEYRING.write().unwrap_or_else(|e| e.into_inner()).as_mut() {
        for dir in dirs {
            ring.remove(*dir);
        }
    }
}

// ---- turning existing data into encrypted data --------------------------------------------------

/// What `encrypt_member_data` did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Migration {
    pub encrypted: usize,
    /// Files it left alone: a name too long to store encrypted.
    pub skipped: Vec<String>,
}

/// Whether a member's data still has to go through `encrypt_member_data`: it never did, or a run
/// was cut short.
pub fn needs_migration(user_dir: &Path) -> bool {
    !user_dir.join(MARKER).exists() || user_dir.join(UNFINISHED).exists()
}

/// Encrypts what a member already has on disk — their vault (`<user_dir>/vault`) and conversations
/// — and, when it's all done, marks both folders encrypted. Safe to run again, and to run on a
/// folder that's empty: whatever is already encrypted is skipped, so a run cut short is finished
/// by the next (`needs_migration` says so). Nothing else should use the member's data meanwhile;
/// the caller runs it before their connection gets a vault.
pub fn encrypt_member_data(user_dir: &Path, conversations_dir: &Path, key: &MemberKey) -> anyhow::Result<Migration> {
    let cipher = VaultCipher::new(key);
    for dir in [user_dir, conversations_dir] {
        std::fs::create_dir_all(dir).with_context(|| format!("failed to create {}", dir.display()))?;
    }
    std::fs::write(user_dir.join(UNFINISHED), b"1").with_context(|| format!("failed to write in {}", user_dir.display()))?;

    let mut done = Migration::default();
    let vault = user_dir.join("vault");
    if vault.is_dir() {
        let mut files = Vec::new();
        plain_files(&vault, Path::new(""), &mut files)?;
        for relative in files {
            match seal_name_path(&cipher, &relative) {
                Ok(sealed) => {
                    let from = vault.join(&relative);
                    let to = vault.join(sealed);
                    let plain = std::fs::read(&from).with_context(|| format!("failed to read {}", from.display()))?;
                    write_sealed(&to, &cipher.seal(&plain))?;
                    std::fs::remove_file(&from).with_context(|| format!("failed to remove {}", from.display()))?;
                    done.encrypted += 1;
                }
                Err(_) => done.skipped.push(relative.to_string_lossy().to_string()),
            }
        }
        remove_empty_plain_dirs(&vault);
    }
    // What their agent made: documents and big media. Names stay as they are (the model chose them),
    // the contents are sealed.
    seal_files_in_place(&cipher, &user_dir.join("generated"), &mut done)?;
    if let Ok(entries) = std::fs::read_dir(conversations_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let bytes = std::fs::read(&path).with_context(|| format!("failed to read {}", path.display()))?;
            if !VaultCipher::is_sealed(&bytes) {
                write_sealed(&path, &cipher.seal(&bytes))?;
                done.encrypted += 1;
            }
        }
    }
    for dir in [user_dir, conversations_dir] {
        std::fs::write(dir.join(MARKER), b"1").with_context(|| format!("failed to mark {} as encrypted", dir.display()))?;
    }
    // Only now can the hub open the folders as encrypted ones.
    unlock(&[user_dir, conversations_dir], key);
    std::fs::remove_file(user_dir.join(UNFINISHED)).ok();
    Ok(done)
}

/// Turns a member's key into what the hub can use: the first time, or after a run cut short, it
/// encrypts what's on disk; otherwise it only holds the key.
pub fn open_member_data(user_dir: &Path, conversations_dir: &Path, key: &MemberKey) -> anyhow::Result<()> {
    if needs_migration(user_dir) {
        encrypt_member_data(user_dir, conversations_dir, key)?;
    } else {
        unlock(&[user_dir, conversations_dir], key);
    }
    Ok(())
}

/// Seals the contents of every file under `dir` that isn't sealed yet, where it is. A folder that
/// isn't there is nothing to do.
fn seal_files_in_place(cipher: &VaultCipher, dir: &Path, done: &mut Migration) -> anyhow::Result<()> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Ok(()) };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            seal_files_in_place(cipher, &path, done)?;
        } else if !entry.file_name().to_string_lossy().ends_with(".warden-tmp") {
            let bytes = std::fs::read(&path).with_context(|| format!("failed to read {}", path.display()))?;
            if !VaultCipher::is_sealed(&bytes) {
                write_sealed(&path, &cipher.seal(&bytes))?;
                done.encrypted += 1;
            }
        }
    }
    Ok(())
}

/// Every file under `dir` (as a path from `root`) that isn't encrypted yet, symlinks left alone.
fn plain_files(dir: &Path, relative_dir: &Path, out: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("failed to read {}", dir.display()))? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if VaultCipher::looks_sealed_name(&name) || entry.file_type()?.is_symlink() {
            continue;
        }
        let relative = relative_dir.join(&name);
        if entry.path().is_dir() {
            plain_files(&entry.path(), &relative, out)?;
        } else {
            out.push(relative);
        }
    }
    Ok(())
}

fn seal_name_path(cipher: &VaultCipher, relative: &Path) -> anyhow::Result<PathBuf> {
    let mut out = PathBuf::new();
    for part in relative.components() {
        let name = part.as_os_str().to_string_lossy();
        anyhow::ensure!(name.len() <= MAX_NAME_BYTES, "'{name}' is too long");
        out.push(cipher.seal_name(&name)?);
    }
    Ok(out)
}

/// Writes through a temporary dotfile so a crash never leaves half a file.
fn write_sealed(path: &Path, sealed: &[u8]) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("warden-tmp");
    std::fs::write(&tmp, sealed).with_context(|| format!("failed to write {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("failed to write {}", path.display()))
}

fn remove_empty_plain_dirs(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if entry.path().is_dir() && !VaultCipher::looks_sealed_name(&name) {
            remove_empty_plain_dirs(&entry.path());
            let _ = std::fs::remove_dir(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("warden-mc-{tag}-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_key_opens_with_its_password_and_with_nothing_else() {
        let key = new_key();
        let wrapped = wrap_with_password(&key, "correct horse").unwrap();
        assert!(!wrapped.contains("correct"));
        assert_eq!(*unwrap_with_password(&wrapped, "correct horse").unwrap(), *key);
        assert!(unwrap_with_password(&wrapped, "wrong horse").is_err());
        assert!(unwrap_with_password("not base64 !!", "correct horse").is_err());
        assert_ne!(wrapped, wrap_with_password(&key, "correct horse").unwrap(), "a fresh salt each time");
    }

    #[test]
    fn a_recovery_code_opens_the_key_however_it_is_typed() {
        let key = new_key();
        let code = generate_recovery_code();
        assert_eq!(code.len(), 32 + 7, "{code}");
        assert!(code.chars().all(|c| c == '-' || c.is_ascii_uppercase() || c.is_ascii_digit()));
        let wrapped = wrap_with_code(&key, &code).unwrap();
        assert_eq!(*unwrap_with_code(&wrapped, &code).unwrap(), *key);
        let sloppy = code.to_ascii_lowercase().replace('-', " ");
        assert_eq!(*unwrap_with_code(&wrapped, &sloppy).unwrap(), *key);
        assert!(unwrap_with_code(&wrapped, &generate_recovery_code()).is_err());
        assert!(unwrap_with_code(&wrapped, "ABCD-EFGH").is_err());
        assert_ne!(code, generate_recovery_code());
    }

    #[test]
    fn a_folder_is_plain_until_marked_and_locked_until_its_key_is_held() {
        let dir = temp_dir("state");
        assert!(matches!(dir_state(&dir), DirState::Plain));
        std::fs::write(dir.join(MARKER), b"1").unwrap();
        assert!(matches!(dir_state(&dir), DirState::Locked));
        unlock(&[&dir], &new_key());
        assert!(matches!(dir_state(&dir), DirState::Unlocked(_)));
        lock(&[&dir]);
        assert!(matches!(dir_state(&dir), DirState::Locked));
    }

    #[test]
    fn existing_data_is_encrypted_once_and_the_run_can_be_repeated() {
        let user_dir = temp_dir("user");
        let conversations = temp_dir("convs");
        let vault = user_dir.join("vault");
        std::fs::create_dir_all(vault.join("diário")).unwrap();
        std::fs::write(vault.join("diário/hoje.md"), "segredo do feijão").unwrap();
        std::fs::write(vault.join("solta.md"), "outra nota").unwrap();
        std::fs::write(vault.join(format!("{}.md", "n".repeat(MAX_NAME_BYTES))), "nome grande demais").unwrap();
        std::fs::write(conversations.join("c1.json"), r#"{"title":"segredo"}"#).unwrap();
        std::fs::create_dir_all(user_dir.join("generated/mcp-media")).unwrap();
        std::fs::write(user_dir.join("generated/relatorio.txt"), "relatório do feijão").unwrap();
        std::fs::write(user_dir.join("generated/mcp-media/1.png"), "bytes de imagem").unwrap();

        let key = new_key();
        let first = encrypt_member_data(&user_dir, &conversations, &key).unwrap();
        assert_eq!((first.encrypted, first.skipped.len()), (5, 1), "{first:?}");
        let report = std::fs::read(user_dir.join("generated/relatorio.txt")).unwrap();
        assert!(VaultCipher::is_sealed(&report) && !String::from_utf8_lossy(&report).contains("feijão"), "generated files are sealed too");
        let again = encrypt_member_data(&user_dir, &conversations, &key).unwrap();
        assert_eq!(again.encrypted, 0, "nothing left to do");

        let opened = warden_core::memory::Vault::new_encrypted(&vault, Arc::new(VaultCipher::new(&key)));
        assert_eq!(opened.read("diário/hoje.md").unwrap(), "segredo do feijão");
        assert_eq!(opened.read("solta.md").unwrap(), "outra nota");
        assert!(!vault.join("diário").exists() && !vault.join("solta.md").exists(), "the plain copies are gone");
        let conversation = std::fs::read(conversations.join("c1.json")).unwrap();
        assert!(VaultCipher::is_sealed(&conversation) && !String::from_utf8_lossy(&conversation).contains("segredo"));
        assert!(matches!(dir_state(&user_dir), DirState::Unlocked(_)));
        assert!(user_dir.join(MARKER).exists() && conversations.join(MARKER).exists());
    }
}
