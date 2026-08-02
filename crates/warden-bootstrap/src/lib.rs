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
use warden_core::model::ModelProvider;
use warden_core::orchestrator::Orchestrator;
use warden_core::tool::delegate::DelegateTool;
use warden_core::tool::file_tools::{ReadFileTool, WriteFileTool};
use warden_core::tool::web_search::WebSearchTool;
use warden_core::tool::Tool;

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
    #[serde(default)]
    pub api_keys: ApiKeys,
}

#[derive(Deserialize, Serialize, Default, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ApiKeys {
    pub gemini: Option<String>,
    pub openai: Option<String>,
    pub tavily: Option<String>,
}

/// The model name used when neither an override nor the config file specify one.
pub fn default_model_for(provider: Provider) -> &'static str {
    match provider {
        Provider::Gemini => "gemini-2.5-flash",
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
pub fn bootstrap(
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
        Some(tavily_key) => base_tools.push(Arc::new(WebSearchTool::new(tavily_key))),
        None => eprintln!(
            "note: TAVILY_API_KEY not set — web_search tool disabled (get a free key at https://tavily.com)\n"
        ),
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
            api_keys: ApiKeys {
                gemini: Some("gk".to_string()),
                openai: Some("ok".to_string()),
                tavily: Some("tk".to_string()),
            },
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

    #[test]
    fn resolve_secret_prefers_env_over_file() {
        assert_eq!(
            resolve_secret(Some("from-env".to_string()), Some("from-file".to_string())),
            Some("from-env".to_string())
        );
        assert_eq!(resolve_secret(None, Some("from-file".to_string())), Some("from-file".to_string()));
        assert_eq!(resolve_secret(None, None), None);
    }
}
