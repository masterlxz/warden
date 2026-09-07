use std::path::PathBuf;

/// Where the vault-encryption key lives — separate from `sync_manifest.json` because it never
/// needs rewriting once created (unlike the manifest, which is rewritten on every push/pull), and
/// separate from `config.toml` because that struct is `#[serde(deny_unknown_fields)]` and has an
/// entirely different lifecycle (rebuilt by `warden_bootstrap::save_config`, not sync's).
pub fn default_sync_secrets_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("warden").join("sync_secrets.json"))
}

/// Non-secret sync tracking state — file hashes, last tx id, owner address. Rewritten after
/// every successful push/pull.
pub fn default_sync_manifest_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("warden").join("sync_manifest.json"))
}
