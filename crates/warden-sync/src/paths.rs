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

/// Local working clone `GitSyncEngine` (P63) keeps its `bundle.enc` commits in — same
/// `dirs::config_dir()` base as the other paths here. Shared across every push/pull on this
/// device; never touched by anything other than `git.rs`'s own shell-outs.
pub fn default_git_sync_repo_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("warden").join("git-sync-repo"))
}
