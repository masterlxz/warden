//! Settings over the wire (P78): what `RequestSettings`/`SaveSettings` answer, and the swappable
//! orchestrator a save reloads.
//!
//! A save is all-or-nothing. The pairing key is checked first (a device token alone can't change
//! settings), a new API key is refused on a connection that isn't encrypted or local, and the file
//! must still be the version the screen loaded. Then the new file is written and the host builds a
//! fresh orchestrator from it; if that fails (a provider with no key, say) the old file goes back
//! and nothing changes. Only then does the new orchestrator replace the old one. A turn already
//! running keeps the one it started with.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use anyhow::Context;
use async_trait::async_trait;
use warden_bootstrap::settings::{apply_hub_settings, config_version, hub_settings};
use warden_bootstrap::{load_config_from_path, save_config, FileConfig};
use warden_core::orchestrator::Orchestrator;
use warden_server_protocol::protocol::{HubSettingsDto, HubSettingsUpdate};
use warden_server_protocol::ServerMessage;

/// How long a wrong pairing key waits before its answer, so guessing one over the socket is slow.
pub const WRONG_KEY_DELAY: Duration = Duration::from_secs(1);

/// The hub's orchestrator, which a settings save replaces while connections stay open. Each turn
/// takes `current()` once and keeps it for the whole turn.
#[derive(Clone)]
pub struct SharedOrchestrator(Arc<RwLock<Arc<Orchestrator>>>);

impl SharedOrchestrator {
    pub fn new(orchestrator: Orchestrator) -> Self {
        Self::from(Arc::new(orchestrator))
    }

    pub fn current(&self) -> Arc<Orchestrator> {
        self.0.read().unwrap().clone()
    }

    pub fn replace(&self, orchestrator: Orchestrator) {
        *self.0.write().unwrap() = Arc::new(orchestrator);
    }
}

impl From<Arc<Orchestrator>> for SharedOrchestrator {
    fn from(orchestrator: Arc<Orchestrator>) -> Self {
        Self(Arc::new(RwLock::new(orchestrator)))
    }
}

/// What a hub process knows about its own settings: where its config file is and how it builds an
/// orchestrator from it (the standalone `warden-server` and the desktop's embedded hub differ in
/// both). Without one, the hub answers every settings request with an error.
#[async_trait]
pub trait SettingsHost: Send + Sync {
    fn config_path(&self) -> PathBuf;

    /// A fresh orchestrator from the file at `config_path()`, built the way this hub started.
    async fn build(&self) -> anyhow::Result<Orchestrator>;

    /// Caveats outside the file, e.g. a command-line flag that wins over it. One sentence each.
    fn notes(&self) -> Vec<String> {
        Vec::new()
    }

    /// Called with the new orchestrator once a save has put it in place — the desktop hands it to
    /// its own chat too.
    fn installed(&self, _orchestrator: &Orchestrator) {}
}

/// Whether a connection may carry a new API key: TLS, or a peer on this same machine.
pub fn is_secure(tls: bool, peer: std::net::IpAddr) -> bool {
    let local = match peer {
        std::net::IpAddr::V4(ip) => ip.is_loopback(),
        std::net::IpAddr::V6(ip) => ip.is_loopback() || ip.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback()),
    };
    tls || local
}

/// Compares every byte, so how long a wrong key takes says nothing about how much of it was right.
fn keys_match(provided: &str, expected: &str) -> bool {
    let (a, b) = (provided.as_bytes(), expected.as_bytes());
    if a.is_empty() || a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn tool_names(orchestrator: &Orchestrator) -> Vec<String> {
    let mut names: Vec<String> = orchestrator.tools().iter().map(|t| t.spec().name).collect();
    names.sort();
    names.dedup();
    names
}

fn view(host: &dyn SettingsHost, config: &FileConfig, orchestrator: &Orchestrator) -> HubSettingsDto {
    hub_settings(config, tool_names(orchestrator), host.notes())
}

fn settings_error(request_id: u64, message: impl Into<String>) -> ServerMessage {
    ServerMessage::SettingsError { request_id, message: message.into(), conflict: false, auth_rejected: false }
}

const NO_SETTINGS: &str = "this hub doesn't offer its settings over the network";

/// What the settings handlers need from the hub and from the connection asking.
pub struct SettingsAccess<'a> {
    /// `None` on a hub that doesn't offer its settings.
    pub host: Option<&'a dyn SettingsHost>,
    pub shared: &'a SharedOrchestrator,
    /// Serializes saves on this hub, so two screens saving at once can't both pass the version check.
    pub lock: &'a tokio::sync::Mutex<()>,
    /// The hub's pairing key, which a save must repeat.
    pub auth_key: &'a str,
    /// Whether this connection may carry a new API key (`is_secure`).
    pub secure: bool,
}

/// Answers `RequestSettings`.
pub fn handle_request_settings(access: &SettingsAccess<'_>, request_id: u64) -> ServerMessage {
    let Some(host) = access.host else {
        return settings_error(request_id, NO_SETTINGS);
    };
    let path = host.config_path();
    let loaded = config_version(&path).and_then(|version| Ok((version, load_config_from_path(&path, false)?)));
    match loaded {
        Ok((version, config)) => ServerMessage::Settings {
            request_id,
            settings: view(host, &config, &access.shared.current()),
            version,
            secrets_writable: access.secure,
        },
        Err(err) => settings_error(request_id, format!("{err:#}")),
    }
}

/// Answers `SaveSettings`.
pub async fn handle_save_settings(access: &SettingsAccess<'_>, request_id: u64, pairing_key: &str, base_version: &str, update: HubSettingsUpdate) -> ServerMessage {
    let Some(host) = access.host else {
        return settings_error(request_id, NO_SETTINGS);
    };
    let (shared, secure) = (access.shared, access.secure);
    let _saving = access.lock.lock().await;
    if !keys_match(pairing_key, access.auth_key) {
        tokio::time::sleep(WRONG_KEY_DELAY).await;
        return ServerMessage::SettingsError { request_id, message: "wrong pairing key".to_string(), conflict: false, auth_rejected: true };
    }
    if update.sets_a_secret() && !secure {
        return settings_error(
            request_id,
            "API keys can only be changed over an encrypted connection (https://) or from the hub's own machine — nothing was saved",
        );
    }

    let path = host.config_path();
    let previous = match read_optional(&path) {
        Ok(bytes) => bytes,
        Err(err) => return settings_error(request_id, format!("{err:#}")),
    };
    if warden_core::memory::content_version(previous.as_deref().unwrap_or_default()) != base_version {
        return ServerMessage::SettingsError {
            request_id,
            message: "the settings changed since this screen loaded them — reload to see the current ones".to_string(),
            conflict: true,
            auth_rejected: false,
        };
    }
    let existing = match load_config_from_path(&path, false) {
        Ok(config) => config,
        Err(err) => return settings_error(request_id, format!("{err:#}")),
    };
    let config = match apply_hub_settings(existing, update) {
        Ok(config) => config,
        Err(message) => return settings_error(request_id, message),
    };

    if let Err(err) = write_config(&path, &config) {
        return settings_error(request_id, format!("{err:#}"));
    }
    let orchestrator = match host.build().await {
        Ok(orchestrator) => orchestrator,
        Err(err) => {
            let restored = match &previous {
                Some(bytes) => write_atomic(&path, bytes),
                None => std::fs::remove_file(&path).context("failed to remove the new config file"),
            };
            let mut message = format!("settings not saved — the hub couldn't start with them: {err:#}");
            if let Err(restore_err) = restored {
                message.push_str(&format!(" (and putting the previous file back failed: {restore_err:#})"));
            }
            return settings_error(request_id, message);
        }
    };

    let settings = view(host, &config, &orchestrator);
    host.installed(&orchestrator);
    shared.replace(orchestrator);
    match config_version(&path) {
        Ok(version) => ServerMessage::SettingsSaved { request_id, settings, version },
        Err(err) => settings_error(request_id, format!("saved, but reading it back failed: {err:#}")),
    }
}

fn read_optional(path: &Path) -> anyhow::Result<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err).with_context(|| format!("failed to read config file at {}", path.display())),
    }
}

/// `save_config`, but through a temporary file and a rename, so a reader never sees half a file.
fn write_config(path: &Path, config: &FileConfig) -> anyhow::Result<()> {
    let staged = staging_path(path);
    save_config(&staged, config)?;
    std::fs::rename(&staged, path).with_context(|| format!("failed to write config file at {}", path.display()))
}

fn write_atomic(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let staged = staging_path(path);
    std::fs::write(&staged, bytes).with_context(|| format!("failed to write {}", staged.display()))?;
    std::fs::rename(&staged, path).with_context(|| format!("failed to write config file at {}", path.display()))
}

fn staging_path(path: &Path) -> PathBuf {
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "config.toml".to_string());
    path.with_file_name(format!(".{name}.saving"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use warden_bootstrap::{bootstrap, Overrides};
    use warden_server_protocol::protocol::{ProviderEditDto, SecretEdit};

    const KEY: &str = "pairing-key-0123456789";

    struct TestHost {
        dir: PathBuf,
        installed: AtomicUsize,
    }

    #[async_trait]
    impl SettingsHost for TestHost {
        fn config_path(&self) -> PathBuf {
            self.dir.join("config.toml")
        }

        async fn build(&self) -> anyhow::Result<Orchestrator> {
            bootstrap(Some(self.config_path().to_str().unwrap()), Overrides::default(), self.dir.join("vault")).await
        }

        fn notes(&self) -> Vec<String> {
            vec!["--model wins".to_string()]
        }

        fn installed(&self, _orchestrator: &Orchestrator) {
            self.installed.fetch_add(1, Ordering::SeqCst);
        }
    }

    const START: &str = r#"
enable_shell = false
active_provider = "main"

[api_keys]
whisper = "sk-whisper-kept-secret-abcdef"

[[providers]]
id = "main"
kind = "anthropic"
api_key = "sk-ant-original-secret-9999"
"#;

    async fn setup(name: &str) -> (TestHost, SharedOrchestrator) {
        let dir = std::env::temp_dir().join(format!("warden-hub-settings-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.toml"), START).unwrap();
        let host = TestHost { dir, installed: AtomicUsize::new(0) };
        let shared = SharedOrchestrator::new(host.build().await.unwrap());
        (host, shared)
    }

    static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    fn access<'a>(host: Option<&'a dyn SettingsHost>, shared: &'a SharedOrchestrator, lock: &'a tokio::sync::Mutex<()>, secure: bool) -> SettingsAccess<'a> {
        SettingsAccess { host, shared, lock, auth_key: KEY, secure }
    }

    fn load(host: &TestHost, shared: &SharedOrchestrator, secure: bool) -> (HubSettingsDto, String, bool) {
        match handle_request_settings(&access(Some(host), shared, &LOCK, secure), 1) {
            ServerMessage::Settings { settings, version, secrets_writable, .. } => (settings, version, secrets_writable),
            other => panic!("{other:?}"),
        }
    }

    fn untouched(settings: &HubSettingsDto) -> HubSettingsUpdate {
        HubSettingsUpdate {
            providers: settings
                .providers
                .iter()
                .map(|p| ProviderEditDto {
                    original_id: Some(p.id.clone()),
                    id: p.id.clone(),
                    kind: p.kind.clone(),
                    base_url: p.base_url.clone(),
                    model: p.model.clone(),
                    api_key: SecretEdit::Keep,
                })
                .collect(),
            active_provider: settings.active_provider.clone(),
            agents: settings.agents.clone(),
            tavily_key: SecretEdit::Keep,
            whisper_key: SecretEdit::Keep,
            limits: settings.limits.clone(),
            prices: settings.prices.clone(),
        }
    }

    async fn save(host: &TestHost, shared: &SharedOrchestrator, secure: bool, key: &str, version: &str, update: HubSettingsUpdate) -> ServerMessage {
        let lock = tokio::sync::Mutex::new(());
        handle_save_settings(&access(Some(host), shared, &lock, secure), 2, key, version, update).await
    }

    #[tokio::test]
    async fn loading_shows_the_editable_slice_without_any_secret() {
        let (host, shared) = setup("load").await;
        let reply = handle_request_settings(&access(Some(&host), &shared, &LOCK, false), 7);
        let json = serde_json::to_string(&reply).unwrap();
        assert!(!json.contains("original-secret") && !json.contains("kept-secret"), "{json}");

        let (settings, version, writable) = load(&host, &shared, false);
        assert_eq!(version, config_version(&host.config_path()).unwrap());
        assert!(!writable);
        assert_eq!(settings.providers[0].api_key.hint.as_deref(), Some("9999"));
        assert!(settings.tool_names.contains(&"read_file".to_string()));
        assert_eq!(settings.notes[0], "--model wins");
        std::fs::remove_dir_all(&host.dir).unwrap();
    }

    #[tokio::test]
    async fn a_save_writes_the_file_and_swaps_the_orchestrator() {
        let (host, shared) = setup("swap").await;
        let before = shared.current();
        let (settings, version, _) = load(&host, &shared, true);
        let mut update = untouched(&settings);
        update.providers[0].model = "claude-test".into();
        update.providers[0].api_key = SecretEdit::Set("sk-ant-new".into());

        let reply = save(&host, &shared, true, KEY, &version, update).await;
        let ServerMessage::SettingsSaved { settings, version: new_version, .. } = reply else { panic!("{reply:?}") };
        assert_eq!(settings.providers[0].model, "claude-test");
        assert_ne!(new_version, version);
        assert!(!Arc::ptr_eq(&before, &shared.current()), "the hub runs on the new orchestrator");
        assert_eq!(host.installed.load(Ordering::SeqCst), 1);

        let text = std::fs::read_to_string(host.config_path()).unwrap();
        assert!(text.contains("sk-ant-new") && text.contains("sk-whisper-kept-secret-abcdef"), "{text}");
        assert!(text.contains("enable_shell = false"), "what the screen doesn't show is carried over: {text}");
        std::fs::remove_dir_all(&host.dir).unwrap();
    }

    #[tokio::test]
    async fn a_wrong_pairing_key_changes_nothing() {
        let (host, shared) = setup("key").await;
        let (settings, version, _) = load(&host, &shared, true);
        for key in ["", "wrong", "pairing-key-0123456780"] {
            let reply = save(&host, &shared, true, key, &version, untouched(&settings)).await;
            assert!(matches!(reply, ServerMessage::SettingsError { auth_rejected: true, conflict: false, .. }), "{reply:?}");
        }
        assert_eq!(std::fs::read_to_string(host.config_path()).unwrap(), START);
        std::fs::remove_dir_all(&host.dir).unwrap();
    }

    #[tokio::test]
    async fn a_new_key_is_refused_over_a_plain_remote_connection_but_other_changes_go_through() {
        let (host, shared) = setup("plain").await;
        let (settings, version, _) = load(&host, &shared, false);
        let mut with_key = untouched(&settings);
        with_key.whisper_key = SecretEdit::Set("sk-whisper".into());
        let reply = save(&host, &shared, false, KEY, &version, with_key).await;
        let ServerMessage::SettingsError { message, .. } = reply else { panic!("{reply:?}") };
        assert!(message.contains("encrypted connection"), "{message}");
        assert_eq!(std::fs::read_to_string(host.config_path()).unwrap(), START);

        let mut cleared = untouched(&settings);
        cleared.whisper_key = SecretEdit::Clear;
        let reply = save(&host, &shared, false, KEY, &version, cleared).await;
        assert!(matches!(reply, ServerMessage::SettingsSaved { .. }), "clearing a key sends no secret: {reply:?}");
        assert!(!std::fs::read_to_string(host.config_path()).unwrap().contains("sk-whisper"));
        std::fs::remove_dir_all(&host.dir).unwrap();
    }

    #[tokio::test]
    async fn a_file_changed_since_loading_is_a_conflict() {
        let (host, shared) = setup("conflict").await;
        let (settings, version, _) = load(&host, &shared, true);
        let edited = format!("{START}\n# edited by hand\n");
        std::fs::write(host.config_path(), &edited).unwrap();
        let reply = save(&host, &shared, true, KEY, &version, untouched(&settings)).await;
        assert!(matches!(reply, ServerMessage::SettingsError { conflict: true, .. }), "{reply:?}");
        assert_eq!(std::fs::read_to_string(host.config_path()).unwrap(), edited);
        std::fs::remove_dir_all(&host.dir).unwrap();
    }

    #[tokio::test]
    async fn settings_the_hub_cannot_start_with_are_rolled_back() {
        let (host, shared) = setup("rollback").await;
        let before = shared.current();
        let (settings, version, _) = load(&host, &shared, true);
        let mut update = untouched(&settings);
        update.providers[0].api_key = SecretEdit::Clear;
        update.providers[0].kind = "openai_compatible".into();

        let reply = save(&host, &shared, true, KEY, &version, update).await;
        let ServerMessage::SettingsError { message, conflict: false, auth_rejected: false, .. } = reply else { panic!("{reply:?}") };
        assert!(message.starts_with("settings not saved"), "{message}");
        assert_eq!(std::fs::read_to_string(host.config_path()).unwrap(), START, "the previous file is back");
        assert!(Arc::ptr_eq(&before, &shared.current()));
        assert_eq!(host.installed.load(Ordering::SeqCst), 0);
        assert!(!host.dir.join(".config.toml.saving").exists());
        std::fs::remove_dir_all(&host.dir).unwrap();
    }

    #[tokio::test]
    async fn invalid_input_is_refused_before_touching_the_file() {
        let (host, shared) = setup("invalid").await;
        let (settings, version, _) = load(&host, &shared, true);
        let mut update = untouched(&settings);
        update.active_provider = "ghost".into();
        let reply = save(&host, &shared, true, KEY, &version, update).await;
        let ServerMessage::SettingsError { message, .. } = reply else { panic!("{reply:?}") };
        assert!(message.contains("ghost"), "{message}");
        assert_eq!(std::fs::read_to_string(host.config_path()).unwrap(), START);
        std::fs::remove_dir_all(&host.dir).unwrap();
    }

    #[tokio::test]
    async fn a_hub_without_a_settings_host_says_so() {
        let (host, shared) = setup("none").await;
        let none = access(None, &shared, &LOCK, true);
        let reply = handle_request_settings(&none, 3);
        assert!(matches!(reply, ServerMessage::SettingsError { request_id: 3, .. }));
        let settings = hub_settings(&FileConfig::default(), Vec::new(), Vec::new());
        let reply = handle_save_settings(&none, 4, KEY, "", untouched(&settings)).await;
        assert!(matches!(reply, ServerMessage::SettingsError { request_id: 4, auth_rejected: false, .. }));
        std::fs::remove_dir_all(&host.dir).unwrap();
    }

    #[test]
    fn only_tls_or_this_machine_counts_as_secure() {
        let ip = |s: &str| s.parse::<std::net::IpAddr>().unwrap();
        assert!(is_secure(true, ip("192.168.0.9")));
        assert!(is_secure(false, ip("127.0.0.1")));
        assert!(is_secure(false, ip("::1")));
        assert!(is_secure(false, ip("::ffff:127.0.0.1")));
        assert!(!is_secure(false, ip("192.168.0.9")));
        assert!(!is_secure(false, ip("100.64.1.2")), "Tailscale without TLS is still plain http in the browser");
    }

    #[test]
    fn keys_are_compared_whole() {
        assert!(keys_match(KEY, KEY));
        assert!(!keys_match("", ""), "an empty key never matches");
        assert!(!keys_match("pairing-key", KEY));
        assert!(!keys_match(&format!("{KEY}x"), KEY));
    }
}
