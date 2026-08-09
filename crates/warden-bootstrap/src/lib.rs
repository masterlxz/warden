//! Shared bootstrap logic for every Warden channel (CLI, desktop, ...): load the TOML config
//! file, resolve provider/model/vault/API keys with a consistent precedence, and build a
//! ready-to-use `Orchestrator`. Kept out of `warden-core` on purpose — config-file/env-var
//! sourcing is an application-bootstrap concern specific to standalone deployments, not
//! something the model-agnostic engine itself should know about.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use serde::{Deserialize, Serialize};
use warden_core::memory::Vault;
use warden_core::model::gemini::GeminiProvider;
use warden_core::model::openai::OpenAiProvider;
use warden_core::model::{Message, ModelProvider, Usage};
use warden_core::orchestrator::{MessageOutcome, Orchestrator};
use warden_core::tool::delegate::DelegateTool;
use warden_core::tool::file_tools::{ReadFileTool, WriteFileTool};
use warden_core::tool::mcp::McpToolProvider;
use warden_core::tool::shell::ShellTool;
use warden_core::tool::{Tool, ToolProvider};

#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Gemini,
    Openai,
}

/// Config file shape (TOML). Every field is optional — overrides and env vars (for API keys)
/// always win over what's here, and the whole file is optional too.
#[derive(Deserialize, Serialize, Default, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FileConfig {
    pub provider: Option<Provider>,
    pub model: Option<String>,
    pub vault_path: Option<String>,
    /// Opt-in gate for the `shell` tool (Phase 5.5) — off unless explicitly turned on, since it
    /// lets the model run arbitrary commands on this machine with no sandboxing.
    pub enable_shell: Option<bool>,
    #[serde(default)]
    pub api_keys: ApiKeys,
    /// External MCP servers to connect to on startup (Phase 5.2) — empty by default, same
    /// "off unless configured" spirit as the shell tool. Each entry is spawned as a local child
    /// process (stdio transport, the standard for local MCP servers); whatever tools it
    /// advertises get registered alongside the built-in ones.
    #[serde(default)]
    pub mcp_servers: Vec<McpServerConfig>,
}

/// One external MCP server to connect to (TOML: `[[mcp_servers]]`). `command`/`args`/`env`
/// mirror the shape every other MCP client config uses (e.g. Claude Desktop's `mcpServers`) —
/// deliberately, so a user who already has MCP server configs from elsewhere can port them over
/// close to verbatim.
#[derive(Deserialize, Serialize, Default, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct McpServerConfig {
    /// Only used for logging/error messages — not sent to the server.
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,
}

#[derive(Deserialize, Serialize, Default, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ApiKeys {
    pub gemini: Option<String>,
    pub openai: Option<String>,
    pub tavily: Option<String>,
    /// Bot token from @BotFather (Fase 2) — only read by `warden-telegram`, not by `bootstrap()`
    /// itself, since a Telegram bot token isn't a model/tool secret the orchestrator needs.
    pub telegram_bot_token: Option<String>,
}

/// The model name used when neither an override nor the config file specify one.
///
/// These go stale as providers retire old models — `gemini-2.5-flash` (the original default,
/// Sessão 1) started 404ing for new API keys as of Sessão 32 ("no longer available to new
/// users"), confirming the risk flagged in `SESSIONS.md` back then. If a default here starts
/// erroring again, check the provider's current model list before assuming it's a code bug.
pub fn default_model_for(provider: Provider) -> &'static str {
    match provider {
        Provider::Gemini => "gemini-3.5-flash",
        Provider::Openai => "gpt-4o-mini",
    }
}

/// Writes `config` as TOML to `path`, creating the parent directory if it doesn't exist yet
/// (the default OS config dir may never have been created before the first save from a
/// settings UI).
pub fn save_config(path: &Path, config: &FileConfig) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create config directory at {}", parent.display()))?;
    }
    let contents = toml::to_string_pretty(config).context("failed to serialize config")?;
    std::fs::write(path, contents).with_context(|| format!("failed to write config file at {}", path.display()))
}

pub fn default_config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("warden").join("config.toml"))
}

/// Chat message roles as persisted to disk — mirrors the frontend's `ChatRole`
/// (`desktop/src/types.ts`), the only two roles ever shown in the chat UI.
#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ChatRole {
    User,
    Assistant,
}

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ConversationMessage {
    pub id: String,
    pub role: ChatRole,
    pub content: String,
    pub created_at: i64,
    /// Token usage for this message's model call(s), when the provider reported it (Fase 5.8).
    /// `#[serde(default)]` so conversations saved before this field existed still load.
    #[serde(default)]
    pub usage: Option<Usage>,
}

/// A whole conversation as persisted to disk — mirrors the frontend's `Conversation`
/// (`desktop/src/types.ts`) field for field, so a loaded value can be handed straight back to
/// the UI with no reshaping at the IPC boundary.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Conversation {
    pub id: String,
    pub title: String,
    pub messages: Vec<ConversationMessage>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Conversations are opaque app data (unlike the human-browsable markdown vault), so — like
/// the config file — they live under the OS config dir, not the vault.
pub fn default_conversations_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("warden").join("conversations"))
}

/// Writes `conversation` as pretty JSON, one file per conversation named by id — an overwrite
/// of the whole file each time, same as `save_config`, since a conversation is small and there's
/// no concurrent writer to race with. Creates the directory if it doesn't exist yet.
pub fn save_conversation(dir: &Path, conversation: &Conversation) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir)
        .with_context(|| format!("failed to create conversations directory at {}", dir.display()))?;
    let path = dir.join(format!("{}.json", conversation.id));
    let contents = serde_json::to_string_pretty(conversation).context("failed to serialize conversation")?;
    std::fs::write(&path, contents).with_context(|| format!("failed to write conversation file at {}", path.display()))
}

/// Lists every persisted conversation, newest-updated first. A directory that doesn't exist yet
/// just means "no conversations saved" — not an error, mirroring `load_config_from_path`'s
/// non-required case. A file that fails to parse is skipped rather than failing the whole list,
/// so one corrupt conversation can't make every other one disappear from the sidebar.
pub fn list_conversations(dir: &Path) -> anyhow::Result<Vec<Conversation>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => {
            return Err(err).with_context(|| format!("failed to read conversations directory at {}", dir.display()))
        }
    };

    let mut conversations = Vec::new();
    for entry in entries {
        let path = entry.with_context(|| format!("failed to read entry in {}", dir.display()))?.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        if let Ok(contents) = std::fs::read_to_string(&path) {
            if let Ok(conversation) = serde_json::from_str::<Conversation>(&contents) {
                conversations.push(conversation);
            }
        }
    }

    conversations.sort_by_key(|c| std::cmp::Reverse(c.updated_at));
    Ok(conversations)
}

/// Loads a single conversation by id, or `None` if no file exists for it yet — the "one
/// conversation" counterpart to `list_conversations`' "missing directory means empty" case.
/// Channels that thread conversations by a stable external id (Telegram's `chat_id`, Fase 2;
/// WhatsApp's later) use this instead of `list_conversations`, which would mean reading every
/// other conversation's file on every incoming message.
pub fn load_conversation(dir: &Path, id: &str) -> anyhow::Result<Option<Conversation>> {
    let path = dir.join(format!("{id}.json"));
    match std::fs::read_to_string(&path) {
        Ok(contents) => {
            let conversation = serde_json::from_str(&contents)
                .with_context(|| format!("failed to parse conversation file at {}", path.display()))?;
            Ok(Some(conversation))
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err).with_context(|| format!("failed to read conversation file at {}", path.display())),
    }
}

/// Telegram conversations are kept in their own directory rather than mixed into
/// `default_conversations_dir()` — that one is what the desktop sidebar lists in full, and a
/// Telegram chat (keyed by numeric `chat_id`, no client-side title) isn't meant to show up
/// there. A unified cross-channel conversation view is a bigger product question left for later.
pub fn default_telegram_conversations_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("warden").join("conversations-telegram"))
}

/// Same reasoning as `default_telegram_conversations_dir` — WhatsApp chats are keyed by JID, not
/// a client-side title, and shouldn't show up in the desktop sidebar's `default_conversations_dir`.
pub fn default_whatsapp_conversations_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("warden").join("conversations-whatsapp"))
}

fn now_millis() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as i64
}

/// A short, unique-enough id for a `ConversationMessage` — no `uuid` dependency needed just for
/// this, nanosecond-resolution timestamps are already how this crate's own tests get uniqueness
/// (see `temp_dir`/`temp_toml_path` below).
fn message_id() -> String {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos().to_string()
}

/// Same truncation the frontend's `titleFromMessage` (`desktop/src/App.tsx`) uses for a new
/// conversation's title: collapse whitespace, cut to 40 chars with an ellipsis.
fn title_from(content: &str) -> String {
    let collapsed = content.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() > 40 {
        format!("{}…", collapsed.chars().take(40).collect::<String>())
    } else {
        collapsed
    }
}

fn to_message(message: &ConversationMessage) -> Message {
    match message.role {
        ChatRole::User => Message::user(message.content.clone()),
        ChatRole::Assistant => Message::assistant(message.content.clone()),
    }
}

/// Runs one turn of a conversation threaded by a stable external id: loads its prior history (or
/// starts a fresh `Conversation`, titled from `title_seed`), calls `orchestrator.handle_message`,
/// appends both the user and assistant messages (with token usage on the assistant one, Fase 5.8)
/// and persists the result under `conversations_dir` before returning the model's answer. Shared
/// by every channel that threads conversations this way — Telegram today, WhatsApp later.
/// `warden-cli`'s REPL and the desktop app don't use this: the CLI has no persistence at all, and
/// desktop's frontend already does its own read/append/save around the IPC boundary.
pub async fn handle_turn(
    orchestrator: &Orchestrator,
    conversations_dir: &Path,
    conversation_id: &str,
    title_seed: &str,
    user_input: &str,
) -> anyhow::Result<MessageOutcome> {
    let mut conversation = load_conversation(conversations_dir, conversation_id)?.unwrap_or_else(|| {
        let now = now_millis();
        Conversation { id: conversation_id.to_string(), title: title_from(title_seed), messages: Vec::new(), created_at: now, updated_at: now }
    });

    let history: Vec<Message> = conversation.messages.iter().map(to_message).collect();
    let outcome = orchestrator.handle_message(&history, user_input).await?;

    conversation.messages.push(ConversationMessage {
        id: message_id(),
        role: ChatRole::User,
        content: user_input.to_string(),
        created_at: now_millis(),
        usage: None,
    });
    conversation.messages.push(ConversationMessage {
        id: message_id(),
        role: ChatRole::Assistant,
        content: outcome.content.clone(),
        created_at: now_millis(),
        usage: outcome.usage,
    });
    conversation.updated_at = now_millis();

    save_conversation(conversations_dir, &conversation)?;
    Ok(outcome)
}

/// Loads the config file. An explicit path that doesn't exist is an error (the caller asked
/// for it by name); the default OS config path is optional — most users won't have one yet.
pub fn load_config(explicit_path: Option<&str>) -> anyhow::Result<FileConfig> {
    match explicit_path {
        Some(p) => load_config_from_path(&PathBuf::from(p), true),
        None => match default_config_path() {
            Some(p) => load_config_from_path(&p, false),
            None => Ok(FileConfig::default()),
        },
    }
}

pub fn load_config_from_path(path: &Path, required: bool) -> anyhow::Result<FileConfig> {
    match std::fs::read_to_string(path) {
        Ok(contents) => {
            toml::from_str(&contents).with_context(|| format!("failed to parse config file at {}", path.display()))
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound && !required => Ok(FileConfig::default()),
        Err(err) => Err(err).with_context(|| format!("failed to read config file at {}", path.display())),
    }
}

/// Env var wins over the config file value — lets you override a saved key for one run
/// without editing the file.
pub fn resolve_secret(from_env: Option<String>, from_file: Option<String>) -> Option<String> {
    from_env.or(from_file)
}

/// Same precedence as `resolve_secret` (env wins over file), but for a boolean flag rather than
/// a secret string — used for the `shell` tool's opt-in gate.
pub fn resolve_flag(from_env: Option<String>, from_file: Option<bool>) -> bool {
    match from_env {
        Some(value) => matches!(value.trim().to_lowercase().as_str(), "1" | "true" | "yes"),
        None => from_file.unwrap_or(false),
    }
}

/// Connects to one MCP server over stdio and registers whatever tools it advertises, or logs a
/// warning and leaves `base_tools` untouched on failure — a misconfigured or unreachable server
/// shouldn't take down the whole orchestrator, same graceful-degradation spirit as a missing
/// `TAVILY_API_KEY`. Shared by the built-in Tavily connection and every user-configured entry in
/// `config.mcp_servers` (Phase 5.2), which both need the exact same connect→list→extend flow.
async fn register_mcp_server_tools(base_tools: &mut Vec<Arc<dyn Tool>>, name: &str, command: &str, args: &[String], env: &[(String, String)]) {
    match McpToolProvider::connect_stdio(name, command, args, env).await {
        Ok(provider) => match provider.tools().await {
            Ok(tools) => base_tools.extend(tools),
            Err(err) => eprintln!("note: MCP server '{name}' connected but failed to list tools: {err:#}\n"),
        },
        Err(err) => eprintln!("note: MCP server '{name}' unavailable, skipping: {err:#}\n"),
    }
}

/// Per-channel overrides (CLI flags today; a desktop settings UI later — see PHASE.md 6.5).
#[derive(Default)]
pub struct Overrides {
    pub provider: Option<Provider>,
    pub model: Option<String>,
    pub vault_path: Option<String>,
}

/// Loads config, resolves provider/model/vault/API keys (override > config file > env >
/// built-in default), and builds a ready-to-use `Orchestrator` (including the delegate
/// sub-orchestrator). `default_vault_path` is the last-resort fallback when neither
/// `overrides.vault_path` nor the config file specify one — deliberately caller-supplied since
/// the right fallback differs per channel (a CLI user picks their own cwd; a GUI app can't).
pub async fn bootstrap(
    explicit_config_path: Option<&str>,
    overrides: Overrides,
    default_vault_path: PathBuf,
) -> anyhow::Result<Orchestrator> {
    let config = load_config(explicit_config_path)?;

    let provider = overrides.provider.or(config.provider).unwrap_or(Provider::Gemini);
    let model_override = overrides.model.or(config.model);
    let vault_path = overrides
        .vault_path
        .map(PathBuf::from)
        .or_else(|| config.vault_path.map(PathBuf::from))
        .unwrap_or(default_vault_path);

    let model_provider: Arc<dyn ModelProvider> = match provider {
        Provider::Gemini => {
            let api_key = resolve_secret(std::env::var("GEMINI_API_KEY").ok(), config.api_keys.gemini).context(
                "GEMINI_API_KEY not set (env var or config file) — get a free key at https://aistudio.google.com/apikey",
            )?;
            let model = model_override.unwrap_or_else(|| default_model_for(Provider::Gemini).to_string());
            Arc::new(GeminiProvider::new(api_key, model))
        }
        Provider::Openai => {
            let api_key = resolve_secret(std::env::var("OPENAI_API_KEY").ok(), config.api_keys.openai)
                .context("OPENAI_API_KEY not set (env var or config file) — export it before running warden")?;
            let model = model_override.unwrap_or_else(|| default_model_for(Provider::Openai).to_string());
            Arc::new(OpenAiProvider::new(api_key, model))
        }
    };

    let vault = Arc::new(Vault::new(vault_path));

    let mut base_tools: Vec<Arc<dyn Tool>> =
        vec![Arc::new(ReadFileTool::new(vault.clone())), Arc::new(WriteFileTool::new(vault.clone()))];

    match resolve_secret(std::env::var("TAVILY_API_KEY").ok(), config.api_keys.tavily) {
        Some(tavily_key) => {
            // Tavily's own MCP server (not a hand-rolled REST call) — gives search plus
            // extract/crawl/map for free, and doubles as real-world validation of the MCP
            // client (Phase 5.2) against a third-party server, not just the hand-written one
            // in warden-core's test suite. Trade-off accepted deliberately: this now needs
            // Node.js/npx on PATH at runtime, which a pure-Rust REST call didn't.
            register_mcp_server_tools(
                &mut base_tools,
                "tavily",
                "npx",
                &["-y".to_string(), "tavily-mcp".to_string()],
                &[("TAVILY_API_KEY".to_string(), tavily_key)],
            )
            .await;
        }
        None => eprintln!(
            "note: TAVILY_API_KEY not set — web search (via tavily-mcp) disabled (get a free key at https://tavily.com)\n"
        ),
    }

    if resolve_flag(std::env::var("WARDEN_ENABLE_SHELL").ok(), config.enable_shell) {
        base_tools.push(Arc::new(ShellTool::new(vault.clone())));
    } else {
        eprintln!(
            "note: shell tool disabled — set WARDEN_ENABLE_SHELL=1 (or enable_shell = true in config.toml) to \
             enable it. It lets the model run arbitrary commands on this machine, with no sandboxing.\n"
        );
    }

    for server in &config.mcp_servers {
        let env: Vec<(String, String)> = server.env.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        register_mcp_server_tools(&mut base_tools, &server.name, &server.command, &server.args, &env).await;
    }

    let mut sub_orchestrator = Orchestrator::new(model_provider.clone(), vault.clone());
    for tool in &base_tools {
        sub_orchestrator.register_tool(tool.clone());
    }

    let mut orchestrator = Orchestrator::new(model_provider, vault);
    for tool in base_tools {
        orchestrator.register_tool(tool);
    }
    orchestrator.register_tool(Arc::new(DelegateTool::new(sub_orchestrator)));

    Ok(orchestrator)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_toml_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "warden-bootstrap-config-test-{name}-{}.toml",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ))
    }

    #[test]
    fn parses_a_valid_config_file() {
        let path = temp_toml_path("valid");
        std::fs::write(
            &path,
            r#"
provider = "openai"
model = "gpt-4o-mini"
vault_path = "/tmp/some-vault"

[api_keys]
gemini = "gk"
openai = "ok"
tavily = "tk"
"#,
        )
        .unwrap();

        let config = load_config(Some(path.to_str().unwrap())).unwrap();

        assert_eq!(config.provider, Some(Provider::Openai));
        assert_eq!(config.model.as_deref(), Some("gpt-4o-mini"));
        assert_eq!(config.vault_path.as_deref(), Some("/tmp/some-vault"));
        assert_eq!(config.api_keys.gemini.as_deref(), Some("gk"));
        assert_eq!(config.api_keys.openai.as_deref(), Some("ok"));
        assert_eq!(config.api_keys.tavily.as_deref(), Some("tk"));
        assert!(config.mcp_servers.is_empty());

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn parses_mcp_servers_from_toml() {
        let path = temp_toml_path("mcp-servers");
        std::fs::write(
            &path,
            r#"
[[mcp_servers]]
name = "anchor"
command = "npx"
args = ["-y", "@anchor/mcp-server"]

[mcp_servers.env]
ANCHOR_API_KEY = "secret"

[[mcp_servers]]
name = "no-args-server"
command = "some-mcp-server"
"#,
        )
        .unwrap();

        let config = load_config(Some(path.to_str().unwrap())).unwrap();

        assert_eq!(config.mcp_servers.len(), 2);
        assert_eq!(config.mcp_servers[0].name, "anchor");
        assert_eq!(config.mcp_servers[0].command, "npx");
        assert_eq!(config.mcp_servers[0].args, vec!["-y".to_string(), "@anchor/mcp-server".to_string()]);
        assert_eq!(config.mcp_servers[0].env.get("ANCHOR_API_KEY").map(String::as_str), Some("secret"));
        assert_eq!(config.mcp_servers[1].name, "no-args-server");
        assert!(config.mcp_servers[1].args.is_empty());
        assert!(config.mcp_servers[1].env.is_empty());

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn missing_non_required_path_falls_back_to_empty_config() {
        // Mirrors the default (no explicit config path) OS config path: most users won't have
        // one yet, so a missing file there should silently mean "no config", not an error.
        let path = temp_toml_path("does-not-exist");
        let config = load_config_from_path(&path, false).unwrap();

        assert!(config.provider.is_none());
        assert!(config.vault_path.is_none());
    }

    #[test]
    fn missing_required_path_errors() {
        // Mirrors an explicit `--config <path>`: the caller named this file, so a missing file
        // is a mistake worth surfacing, not silently ignored.
        let path = temp_toml_path("does-not-exist-explicit");
        let err = load_config_from_path(&path, true).unwrap_err();
        assert!(err.to_string().contains("failed to read config file"), "error was: {err}");
    }

    #[test]
    fn malformed_config_file_errors_clearly() {
        let path = temp_toml_path("malformed");
        std::fs::write(&path, "this is not valid = = toml").unwrap();

        let err = load_config(Some(path.to_str().unwrap())).unwrap_err();
        assert!(err.to_string().contains("failed to parse config file"), "error was: {err}");

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn save_config_round_trips_through_load_config() {
        let path = temp_toml_path("save-round-trip");
        let config = FileConfig {
            provider: Some(Provider::Openai),
            model: Some("gpt-4o-mini".to_string()),
            vault_path: Some("/tmp/some-vault".to_string()),
            enable_shell: Some(true),
            api_keys: ApiKeys {
                gemini: Some("gk".to_string()),
                openai: Some("ok".to_string()),
                tavily: Some("tk".to_string()),
                telegram_bot_token: Some("tt".to_string()),
            },
            mcp_servers: vec![McpServerConfig {
                name: "anchor".to_string(),
                command: "npx".to_string(),
                args: vec!["-y".to_string(), "@anchor/mcp-server".to_string()],
                env: std::collections::HashMap::from([("ANCHOR_API_KEY".to_string(), "secret".to_string())]),
            }],
        };

        save_config(&path, &config).unwrap();
        let loaded = load_config_from_path(&path, true).unwrap();

        assert_eq!(loaded, config);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn save_config_creates_missing_parent_directory() {
        let parent = std::env::temp_dir().join(format!(
            "warden-bootstrap-config-test-missing-parent-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        let path = parent.join("config.toml");
        assert!(!parent.exists());

        save_config(&path, &FileConfig::default()).unwrap();
        assert!(load_config_from_path(&path, true).is_ok());

        std::fs::remove_dir_all(&parent).ok();
    }

    fn temp_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "warden-bootstrap-conversations-test-{name}-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ))
    }

    fn sample_conversation(id: &str, updated_at: i64) -> Conversation {
        Conversation {
            id: id.to_string(),
            title: format!("Conversation {id}"),
            messages: vec![ConversationMessage {
                id: "m1".to_string(),
                role: ChatRole::User,
                content: "hello".to_string(),
                created_at: updated_at,
                usage: None,
            }],
            created_at: updated_at,
            updated_at,
        }
    }

    #[test]
    fn save_conversation_round_trips_through_list_conversations() {
        let dir = temp_dir("round-trip");
        let conversation = sample_conversation("c1", 100);

        save_conversation(&dir, &conversation).unwrap();
        let loaded = list_conversations(&dir).unwrap();

        assert_eq!(loaded, vec![conversation]);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn save_conversation_creates_missing_directory() {
        let dir = temp_dir("missing-dir");
        assert!(!dir.exists());

        save_conversation(&dir, &sample_conversation("c1", 1)).unwrap();
        assert!(dir.exists());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn list_conversations_on_missing_directory_returns_empty() {
        let dir = temp_dir("does-not-exist");
        assert_eq!(list_conversations(&dir).unwrap(), Vec::new());
    }

    #[test]
    fn list_conversations_sorts_newest_updated_first() {
        let dir = temp_dir("sorting");
        save_conversation(&dir, &sample_conversation("old", 100)).unwrap();
        save_conversation(&dir, &sample_conversation("new", 200)).unwrap();

        let loaded = list_conversations(&dir).unwrap();

        assert_eq!(loaded.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), vec!["new", "old"]);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn list_conversations_skips_a_corrupt_file_instead_of_failing() {
        let dir = temp_dir("corrupt");
        save_conversation(&dir, &sample_conversation("good", 1)).unwrap();
        std::fs::write(dir.join("corrupt.json"), "not valid json").unwrap();

        let loaded = list_conversations(&dir).unwrap();

        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, "good");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn resolve_secret_prefers_env_over_file() {
        assert_eq!(
            resolve_secret(Some("from-env".to_string()), Some("from-file".to_string())),
            Some("from-env".to_string())
        );
        assert_eq!(resolve_secret(None, Some("from-file".to_string())), Some("from-file".to_string()));
        assert_eq!(resolve_secret(None, None), None);
    }

    #[test]
    fn resolve_flag_prefers_env_over_file() {
        assert!(resolve_flag(Some("1".to_string()), Some(false)));
        assert!(resolve_flag(Some("true".to_string()), None));
        assert!(!resolve_flag(Some("0".to_string()), Some(true)));
        assert!(!resolve_flag(Some("nonsense".to_string()), Some(true)));
        assert!(resolve_flag(None, Some(true)));
        assert!(!resolve_flag(None, Some(false)));
        assert!(!resolve_flag(None, None));
    }
}
