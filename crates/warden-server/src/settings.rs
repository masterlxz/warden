//! Settings over the wire (P78): what `RequestSettings`/`SaveSettings` answer, and the swappable
//! orchestrator a save reloads.
//!
//! A save is all-or-nothing. The pairing key is checked first (a device token alone can't change
//! settings), a new API key is refused on a connection that isn't encrypted or local, a change to what
//! reaches this machine (shell, MCP servers, SSH hosts, folders, the embedded hub; P119) is refused unless
//! the hub was started with `--allow-machine-settings` and the connection is encrypted or local, and the
//! file must still be the version the screen loaded. Then the new file is written and the host builds a
//! fresh orchestrator from it; if that fails (a provider with no key, say) the old file goes back
//! and nothing changes. Only then does the new orchestrator replace the old one. A turn already
//! running keeps the one it started with.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use anyhow::Context;
use async_trait::async_trait;
use warden_bootstrap::settings::{apply_hub_settings, config_version, hub_settings};
use warden_bootstrap::{load_config_from_path, render_config, FileConfig};
use warden_core::orchestrator::Orchestrator;
use warden_core::tool::Tool;
use warden_server_protocol::protocol::{HubSettingsDto, HubSettingsUpdate};
use warden_server_protocol::ServerMessage;

/// How long a wrong pairing key waits before its answer, so guessing one over the socket is slow.
pub const WRONG_KEY_DELAY: Duration = Duration::from_secs(1);

/// The hub's orchestrator, which a settings save replaces while connections stay open. Each turn
/// takes `current()` once and keeps it for the whole turn.
///
/// Also carries the tools only the hub has (P93): the node tools, fixed (`set_extra_tools`), and the
/// tools of the MCP servers the connected nodes lend, which come and go with them
/// (`set_dynamic_tools`). Both sit on the current orchestrator and on every one a `replace` puts in,
/// so chat, the Warden API and scheduled tasks all get them.
#[derive(Clone)]
pub struct SharedOrchestrator {
    current: Arc<RwLock<Arc<Orchestrator>>>,
    parts: Arc<Mutex<Parts>>,
}

/// What `current` is built from, changed under one lock so two changes never lose each other.
struct Parts {
    /// The orchestrator as the settings built it, without the hub's own tools.
    base: Orchestrator,
    extras: Vec<Arc<dyn Tool>>,
    dynamic: Vec<Arc<dyn Tool>>,
}

impl SharedOrchestrator {
    pub fn new(orchestrator: Orchestrator) -> Self {
        Self::from(Arc::new(orchestrator))
    }

    pub fn current(&self) -> Arc<Orchestrator> {
        self.current.read().unwrap().clone()
    }

    pub fn replace(&self, orchestrator: Orchestrator) {
        self.change(|parts| parts.base = orchestrator);
    }

    /// Tools on every orchestrator from now on. Meant to be called once, when the hub starts serving.
    pub fn set_extra_tools(&self, tools: Vec<Arc<dyn Tool>>) {
        self.change(|parts| parts.extras = tools);
    }

    /// Replaces the tools that come and go (a node's MCP tools, P93) — called when a node joins or leaves.
    pub fn set_dynamic_tools(&self, tools: Vec<Arc<dyn Tool>>) {
        self.change(|parts| parts.dynamic = tools);
    }

    fn change(&self, edit: impl FnOnce(&mut Parts)) {
        let mut parts = self.parts.lock().unwrap_or_else(|e| e.into_inner());
        edit(&mut parts);
        let built = parts.extras.iter().chain(&parts.dynamic).fold(parts.base.clone(), |o, tool| o.with_tool(tool.clone()));
        *self.current.write().unwrap() = Arc::new(built);
    }
}

impl From<Arc<Orchestrator>> for SharedOrchestrator {
    fn from(orchestrator: Arc<Orchestrator>) -> Self {
        let parts = Parts { base: orchestrator.as_ref().clone(), extras: Vec::new(), dynamic: Vec::new() };
        Self { current: Arc::new(RwLock::new(orchestrator)), parts: Arc::new(Mutex::new(parts)) }
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
pub(crate) fn keys_match(provided: &str, expected: &str) -> bool {
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

/// Whether this connection may save what reaches the hub's machine (P119), and if not, why: the hub has
/// to have been started to allow it (`--allow-machine-settings`), and the connection has to be encrypted
/// or local, the same rule a new API key is held to.
pub fn machine_gate(allow_machine: bool, secure: bool) -> Result<(), &'static str> {
    if !allow_machine {
        return Err("This hub was started without --allow-machine-settings, so the shell, MCP servers, SSH hosts, folders and embedded hub can only be changed on its own machine");
    }
    if !secure {
        return Err("The shell, MCP servers, SSH hosts, folders and embedded hub only change over an encrypted connection (https://) or from the hub's own machine");
    }
    Ok(())
}

fn view(host: &dyn SettingsHost, config: &FileConfig, orchestrator: &Orchestrator, access: &SettingsAccess<'_>) -> HubSettingsDto {
    let mut settings = hub_settings(config, tool_names(orchestrator), host.notes());
    match machine_gate(access.allow_machine, access.secure) {
        Ok(()) => settings.machine.writable = true,
        Err(reason) => settings.machine.blocked_reason = reason.to_string(),
    }
    settings
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
    /// Whether the hub was started to let a save change what reaches its machine (`--allow-machine-settings`).
    pub allow_machine: bool,
    /// Who is asking, for the log line a machine change leaves. `None` where there is no connection to name.
    pub peer: Option<std::net::IpAddr>,
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
            settings: view(host, &config, &access.shared.current(), access),
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
    // What reaches this machine (P119) is held to a stricter gate than the rest of a save: refused
    // before anything is read, let alone written.
    if update.machine.is_some() {
        if let Err(reason) = machine_gate(access.allow_machine, secure) {
            return settings_error(request_id, format!("{reason} — nothing was saved"));
        }
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
    // A second read of the file, only to tell the log what a machine change changed (`FileConfig` isn't `Clone`).
    let before = update.machine.is_some().then(|| load_config_from_path(&path, false).ok()).flatten();
    let config = match apply_hub_settings(existing, update) {
        Ok(config) => config,
        Err(message) => return settings_error(request_id, message),
    };

    if let Err(err) = write_config(&path, previous.as_deref(), &config) {
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

    // Now it is in force: say so in the hub's log, naming what changed and never a value.
    if let Some(before) = before {
        let from = access.peer.map(|ip| ip.to_string()).unwrap_or_else(|| "an unknown peer".to_string());
        eprintln!("warden-server: machine settings changed from {from}: {}", warden_bootstrap::machine_settings::machine_change_summary(&before, &config));
    }
    let settings = view(host, &config, &orchestrator, access);
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
/// Merged into `previous` (the file as it was read for the version check), keeping its comments
/// (P82).
fn write_config(path: &Path, previous: Option<&[u8]>, config: &FileConfig) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("failed to create config directory at {}", parent.display()))?;
    }
    let contents = render_config(previous.and_then(|bytes| std::str::from_utf8(bytes).ok()), config)?;
    write_atomic(path, contents.as_bytes())
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

    /// A tool that only has a name — enough to see which orchestrator carries it.
    struct Named(&'static str);

    #[async_trait]
    impl Tool for Named {
        fn spec(&self) -> warden_core::tool::ToolSpec {
            warden_core::tool::ToolSpec { name: self.0.to_string(), description: String::new(), parameters: serde_json::json!({ "type": "object" }) }
        }

        async fn call(&self, _args: serde_json::Value) -> anyhow::Result<serde_json::Value> {
            Ok(serde_json::Value::Null)
        }
    }

    struct Silent;

    #[async_trait]
    impl warden_core::model::ModelProvider for Silent {
        async fn chat_stream(&self, _messages: Vec<warden_core::model::Message>, _tools: Vec<warden_core::tool::ToolSpec>) -> anyhow::Result<warden_core::model::ChatStream> {
            anyhow::bail!("not called")
        }
    }

    fn tool_names(shared: &SharedOrchestrator) -> Vec<String> {
        shared.current().tools().iter().map(|t| t.spec().name).collect()
    }

    #[test]
    fn extra_tools_survive_a_replace() {
        let vault = Arc::new(warden_core::memory::Vault::new(std::env::temp_dir().join("warden-shared-extras-test")));
        let shared = SharedOrchestrator::new(Orchestrator::new(Arc::new(Silent), vault.clone()));
        shared.set_extra_tools(vec![Arc::new(Named("list_nodes"))]);
        assert!(tool_names(&shared).contains(&"list_nodes".to_string()));
        // A settings save or a sync puts a fresh orchestrator in: the hub's own tools stay.
        shared.replace(Orchestrator::new(Arc::new(Silent), vault));
        assert_eq!(tool_names(&shared).iter().filter(|n| *n == "list_nodes").count(), 1);

        // A node joins with an MCP tool, then leaves: the fixed tools stay through both.
        shared.set_dynamic_tools(vec![Arc::new(Named("casa__query"))]);
        let names = tool_names(&shared);
        assert!(names.contains(&"casa__query".to_string()) && names.contains(&"list_nodes".to_string()));
        shared.set_dynamic_tools(Vec::new());
        let names = tool_names(&shared);
        assert!(!names.contains(&"casa__query".to_string()) && names.contains(&"list_nodes".to_string()));
    }
    use std::sync::atomic::{AtomicUsize, Ordering};
    use warden_bootstrap::{bootstrap, Overrides};
    use warden_server_protocol::protocol::{AdvancedSettingsDto, BotsSettingsDto, MachineEditDto, McpServerEditDto, ProviderEditDto, SecretEdit, SecretEntryEdit, SshHostDto};

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
enable_shell = false # desligado de propósito
active_provider = "main"

[api_keys]
whisper = "sk-whisper-kept-secret-abcdef"

# o provedor principal
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
        SettingsAccess { host, shared, lock, auth_key: KEY, secure, allow_machine: false, peer: None }
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
                    node: p.node.clone(),
                    api_key: SecretEdit::Keep,
                })
                .collect(),
            active_provider: settings.active_provider.clone(),
            agents: settings.agents.clone(),
            tavily_key: SecretEdit::Keep,
            whisper_key: SecretEdit::Keep,
            limits: settings.limits.clone(),
            prices: settings.prices.clone(),
            git_sync: None,
            combos: None,
            bots: None,
            telegram_token: SecretEdit::Keep,
            advanced: None,
            machine: None,
        }
    }

    async fn save(host: &TestHost, shared: &SharedOrchestrator, secure: bool, key: &str, version: &str, update: HubSettingsUpdate) -> ServerMessage {
        let lock = tokio::sync::Mutex::new(());
        handle_save_settings(&access(Some(host), shared, &lock, secure), 2, key, version, update).await
    }

    /// The same, on a hub started with `--allow-machine-settings` (or not).
    async fn save_machine(host: &TestHost, shared: &SharedOrchestrator, allow: bool, secure: bool, key: &str, version: &str, update: HubSettingsUpdate) -> ServerMessage {
        let lock = tokio::sync::Mutex::new(());
        let access = SettingsAccess { allow_machine: allow, ..access(Some(host), shared, &lock, secure) };
        handle_save_settings(&access, 2, key, version, update).await
    }

    fn machine_edit(settings: &HubSettingsDto) -> MachineEditDto {
        MachineEditDto {
            enable_shell: settings.machine.enable_shell,
            vault_path: settings.machine.vault_path.clone(),
            generated_path: settings.machine.generated_path.clone(),
            mcp_servers: Vec::new(),
            ssh_hosts: settings.machine.ssh_hosts.clone(),
            embedded_server: None,
        }
    }

    fn has_tool(shared: &SharedOrchestrator, name: &str) -> bool {
        shared.current().tools().iter().any(|t| t.spec().name == name)
    }

    /// P119: the view says whether the machine slice can be saved, and why not.
    #[tokio::test]
    async fn the_view_says_whether_the_machine_settings_can_be_saved_and_why_not() {
        let (host, shared) = setup("machine-view").await;
        let reason = |allow: bool, secure: bool| {
            let reply = handle_request_settings(&SettingsAccess { allow_machine: allow, ..access(Some(&host), &shared, &LOCK, secure) }, 1);
            let ServerMessage::Settings { settings, .. } = reply else { panic!("{reply:?}") };
            (settings.machine.writable, settings.machine.blocked_reason)
        };
        let (writable, why) = reason(false, true);
        assert!(!writable && why.contains("--allow-machine-settings"), "{why}");
        let (writable, why) = reason(true, false);
        assert!(!writable && why.contains("encrypted connection"), "{why}");
        assert_eq!(reason(true, true), (true, String::new()));
        std::fs::remove_dir_all(&host.dir).unwrap();
    }

    /// P119: a change to what reaches the machine is refused unless the hub was started to allow it and the
    /// connection is encrypted or local, and a refusal changes nothing; allowed, it is saved, applied on the
    /// rebuilt orchestrator and the file keeps its comments.
    #[tokio::test]
    async fn the_machine_slice_needs_the_flag_and_a_secure_connection() {
        let (host, shared) = setup("machine-gate").await;
        let before = shared.current();
        assert!(!has_tool(&shared, "shell"));
        let (settings, version, _) = load(&host, &shared, true);
        let mut update = untouched(&settings);
        let mut machine = machine_edit(&settings);
        machine.enable_shell = true;
        machine.generated_path = host.dir.join("generated").to_string_lossy().to_string();
        machine.ssh_hosts = vec![SshHostDto { id: "box".into(), host: "box.example.com".into(), user: "deploy".into(), port: 22, ..SshHostDto::default() }];
        update.machine = Some(Box::new(machine));

        // The pairing key is checked first, so a wrong one learns nothing about how the hub was started.
        let reply = save_machine(&host, &shared, false, true, "wrong", &version, update.clone()).await;
        assert!(matches!(reply, ServerMessage::SettingsError { auth_rejected: true, .. }), "{reply:?}");

        let reply = save_machine(&host, &shared, false, true, KEY, &version, update.clone()).await;
        let ServerMessage::SettingsError { message, auth_rejected: false, conflict: false, .. } = reply else { panic!("{reply:?}") };
        assert!(message.contains("--allow-machine-settings") && message.contains("nothing was saved"), "{message}");

        let reply = save_machine(&host, &shared, true, false, KEY, &version, update.clone()).await;
        let ServerMessage::SettingsError { message, .. } = reply else { panic!("{reply:?}") };
        assert!(message.contains("encrypted connection"), "{message}");
        assert_eq!(std::fs::read_to_string(host.config_path()).unwrap(), START, "a refusal writes nothing");
        assert!(Arc::ptr_eq(&before, &shared.current()) && host.installed.load(Ordering::SeqCst) == 0);

        let reply = save_machine(&host, &shared, true, true, KEY, &version, update).await;
        let ServerMessage::SettingsSaved { settings, .. } = reply else { panic!("{reply:?}") };
        assert!(settings.machine.enable_shell && settings.machine.writable);
        assert_eq!(settings.machine.ssh_hosts.len(), 1);
        assert!(has_tool(&shared, "shell"), "the rebuilt orchestrator runs with the shell on");
        let text = std::fs::read_to_string(host.config_path()).unwrap();
        assert!(text.contains("enable_shell = true"), "{text}");
        assert!(text.contains("# o provedor principal\n[[providers]]") && text.contains("sk-whisper-kept-secret-abcdef"), "the rest of the file is as it was: {text}");
        std::fs::remove_dir_all(&host.dir).unwrap();
    }

    /// P119: a new MCP value or Telegram token is a secret like an API key (encrypted or local only), while
    /// clearing the token sends nothing and the delegation settings need no special connection at all.
    #[tokio::test]
    async fn a_telegram_token_is_a_secret_and_the_delegation_settings_are_not() {
        let (host, shared) = setup("telegram").await;
        let (settings, version, _) = load(&host, &shared, false);
        assert!(!settings.telegram_token.set);

        let mut with_token = untouched(&settings);
        with_token.telegram_token = SecretEdit::Set("123456:ABC-token-long-enough".into());
        let reply = save(&host, &shared, false, KEY, &version, with_token.clone()).await;
        assert!(matches!(reply, ServerMessage::SettingsError { .. }), "refused over plain http: {reply:?}");
        let reply = save(&host, &shared, true, KEY, &version, with_token).await;
        let ServerMessage::SettingsSaved { settings, version, .. } = reply else { panic!("{reply:?}") };
        assert!(settings.telegram_token.set);
        assert!(!serde_json::to_string(&settings).unwrap().contains("ABC-token"), "the token never comes back");
        assert!(std::fs::read_to_string(host.config_path()).unwrap().contains("123456:ABC-token-long-enough"));

        let mut tuning = untouched(&settings);
        tuning.advanced = Some(Box::new(AdvancedSettingsDto { delegate_max_depth: Some(3), max_delegated_calls: Some(80), max_parallel_jobs: Some(2), truthid_network: "base-mainnet".into(), truthid_rpc_url: String::new(), truthid_public_url: String::new() }));
        let reply = save(&host, &shared, false, KEY, &version, tuning).await;
        let ServerMessage::SettingsSaved { settings, version, .. } = reply else { panic!("saved over plain http, it holds no secret: {reply:?}") };
        assert_eq!((settings.advanced.delegate_max_depth, settings.advanced.max_delegated_calls), (Some(3), Some(80)));

        let mut cleared = untouched(&settings);
        cleared.telegram_token = SecretEdit::Clear;
        let reply = save(&host, &shared, false, KEY, &version, cleared).await;
        let ServerMessage::SettingsSaved { settings, .. } = reply else { panic!("clearing sends no secret: {reply:?}") };
        assert!(!settings.telegram_token.set);
        std::fs::remove_dir_all(&host.dir).unwrap();
    }

    /// P119: an MCP server's new secret value is refused over a plain connection even where the hub allows machine settings.
    #[tokio::test]
    async fn a_new_mcp_secret_over_a_plain_connection_is_refused() {
        let (host, shared) = setup("mcp-secret").await;
        let (settings, version, _) = load(&host, &shared, true);
        let mut update = untouched(&settings);
        let mut machine = machine_edit(&settings);
        machine.mcp_servers = vec![McpServerEditDto {
            original_name: None,
            name: "notes".into(),
            kind: "http".into(),
            command: String::new(),
            args: Vec::new(),
            env: Vec::new(),
            url: "https://mcp.example.com".into(),
            headers: vec![SecretEntryEdit { key: "Authorization".into(), value: SecretEdit::Set("Bearer abc".into()) }],
        }];
        update.machine = Some(Box::new(machine));
        let reply = save_machine(&host, &shared, true, false, KEY, &version, update).await;
        assert!(matches!(reply, ServerMessage::SettingsError { .. }), "{reply:?}");
        assert_eq!(std::fs::read_to_string(host.config_path()).unwrap(), START);
        std::fs::remove_dir_all(&host.dir).unwrap();
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
        assert!(
            text.contains("enable_shell = false # desligado de propósito") && text.contains("# o provedor principal\n[[providers]]"),
            "comments written by hand survive a save (P82): {text}"
        );
        std::fs::remove_dir_all(&host.dir).unwrap();
    }

    /// P118: the bots block round-trips through the hub, and a save that doesn't carry it leaves it be.
    #[tokio::test]
    async fn the_bots_block_is_saved_shown_and_kept_when_a_save_leaves_it_out() {
        let (host, shared) = setup("bots").await;
        let (settings, version, _) = load(&host, &shared, true);
        assert_eq!(settings.bots.learning_max_per_day, 3);
        assert!(settings.bots.telegram_allowed_users.is_empty());

        let mut update = untouched(&settings);
        update.bots = Some(BotsSettingsDto { telegram_allowed_users: vec![42], whatsapp_allowed_chats: vec!["5511999999999".into()], ..settings.bots.clone() });
        let reply = save(&host, &shared, true, KEY, &version, update).await;
        let ServerMessage::SettingsSaved { settings, version, .. } = reply else { panic!("{reply:?}") };
        assert_eq!(settings.bots.telegram_allowed_users, [42]);
        let text = std::fs::read_to_string(host.config_path()).unwrap();
        assert!(text.contains("allowed_users = [42]") && text.contains("5511999999999"), "{text}");

        let reply = save(&host, &shared, true, KEY, &version, untouched(&settings)).await;
        let ServerMessage::SettingsSaved { settings, .. } = reply else { panic!("{reply:?}") };
        assert_eq!(settings.bots.telegram_allowed_users, [42], "a save without bots keeps them");
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
