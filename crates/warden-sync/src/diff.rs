use std::path::PathBuf;

use sha2::{Digest, Sha256};
use warden_core::memory::Vault;

use crate::manifest::SyncManifest;

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// What changed in the vault since `manifest` was last saved.
pub struct VaultDiff {
    pub added_or_modified: Vec<PathBuf>,
    pub deleted: Vec<PathBuf>,
}

impl VaultDiff {
    pub fn is_empty(&self) -> bool {
        self.added_or_modified.is_empty() && self.deleted.is_empty()
    }
}

pub fn diff_vault(vault: &Vault, manifest: &SyncManifest) -> anyhow::Result<VaultDiff> {
    let mut added_or_modified = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for relative in vault.list_all_files()? {
        let key = relative.to_string_lossy().to_string();
        seen.insert(key.clone());
        let content = std::fs::read(vault.root().join(&relative))?;
        let hash = sha256_hex(&content);
        if manifest.vault_files.get(&key) != Some(&hash) {
            added_or_modified.push(relative);
        }
    }

    let deleted = manifest
        .vault_files
        .keys()
        .filter(|path| !seen.contains(*path))
        .map(PathBuf::from)
        .collect();

    Ok(VaultDiff { added_or_modified, deleted })
}

pub fn config_changed(config_bytes: Option<&[u8]>, manifest: &SyncManifest) -> bool {
    match (config_bytes, &manifest.config_hash) {
        (Some(bytes), Some(known_hash)) => &sha256_hex(bytes) != known_hash,
        (Some(_), None) => true,
        (None, Some(_)) => true,
        (None, None) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_vault() -> Vault {
        let dir = std::env::temp_dir().join(format!(
            "warden-sync-diff-test-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        Vault::new(dir)
    }

    fn path_strings(paths: &[PathBuf]) -> Vec<String> {
        let mut strings: Vec<String> = paths.iter().map(|p| p.to_string_lossy().to_string()).collect();
        strings.sort();
        strings
    }

    #[test]
    fn new_file_is_added_or_modified() {
        let vault = temp_vault();
        vault.write("a.md", "hello").unwrap();
        let manifest = SyncManifest::default();

        let diff = diff_vault(&vault, &manifest).unwrap();
        assert_eq!(path_strings(&diff.added_or_modified), vec!["a.md"]);
        assert!(diff.deleted.is_empty());
    }

    #[test]
    fn unchanged_file_is_excluded() {
        let vault = temp_vault();
        vault.write("a.md", "hello").unwrap();
        let mut manifest = SyncManifest::default();
        manifest.vault_files.insert("a.md".to_string(), sha256_hex(b"hello"));

        let diff = diff_vault(&vault, &manifest).unwrap();
        assert!(diff.is_empty());
    }

    #[test]
    fn changed_file_is_added_or_modified() {
        let vault = temp_vault();
        vault.write("a.md", "new content").unwrap();
        let mut manifest = SyncManifest::default();
        manifest.vault_files.insert("a.md".to_string(), sha256_hex(b"old content"));

        let diff = diff_vault(&vault, &manifest).unwrap();
        assert_eq!(path_strings(&diff.added_or_modified), vec!["a.md"]);
    }

    #[test]
    fn removed_file_is_deleted() {
        let vault = temp_vault();
        let mut manifest = SyncManifest::default();
        manifest.vault_files.insert("gone.md".to_string(), sha256_hex(b"bye"));

        let diff = diff_vault(&vault, &manifest).unwrap();
        assert!(diff.added_or_modified.is_empty());
        assert_eq!(path_strings(&diff.deleted), vec!["gone.md"]);
    }

    #[test]
    fn config_changed_covers_all_cases() {
        let manifest_empty = SyncManifest::default();
        assert!(!config_changed(None, &manifest_empty));
        assert!(config_changed(Some(b"toml"), &manifest_empty));

        let manifest_with_hash = SyncManifest { config_hash: Some(sha256_hex(b"toml")), ..Default::default() };
        assert!(!config_changed(Some(b"toml"), &manifest_with_hash));
        assert!(config_changed(Some(b"different"), &manifest_with_hash));
        assert!(config_changed(None, &manifest_with_hash));
    }
}
