use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use clap::{Parser, ValueEnum};
use serde::Deserialize;
use warden_core::memory::Vault;
use warden_core::model::gemini::GeminiProvider;
use warden_core::model::openai::OpenAiProvider;
use warden_core::model::ModelProvider;
use warden_core::orchestrator::Orchestrator;
use warden_core::tool::delegate::DelegateTool;
use warden_core::tool::file_tools::{ReadFileTool, WriteFileTool};
use warden_core::tool::web_search::WebSearchTool;
use warden_core::tool::Tool;

#[derive(ValueEnum, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Provider {
    Gemini,
    Openai,
}

#[derive(Parser, Debug)]
#[command(name = "warden", version, about = "Warden — personal, model-agnostic AI agent")]
struct Cli {
    /// Path to the markdown vault (memory). Overrides the config file; defaults to "vault".
    #[arg(long)]
    vault_path: Option<String>,

    /// Which model provider to talk to. Overrides the config file; defaults to gemini.
    #[arg(long, value_enum)]
    provider: Option<Provider>,

    /// Model name passed to the provider. Overrides the config file; provider-specific default otherwise.
    #[arg(long)]
    model: Option<String>,

    /// Path to the config file (TOML). Defaults to the OS config dir (e.g. ~/.config/warden/config.toml on Linux).
    #[arg(long)]
    config: Option<String>,
}

/// Config file shape (TOML). Every field is optional — CLI flags and env vars (for API keys)
/// always win over what's here, and the whole file is optional too.
#[derive(Deserialize, Default, Debug)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    provider: Option<Provider>,
    model: Option<String>,
    vault_path: Option<String>,
    #[serde(default)]
    api_keys: ApiKeys,
}

#[derive(Deserialize, Default, Debug)]
#[serde(deny_unknown_fields)]
struct ApiKeys {
    gemini: Option<String>,
    openai: Option<String>,
    tavily: Option<String>,
}

fn default_config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("warden").join("config.toml"))
}

/// Loads the config file. An explicit `--config` path that doesn't exist is an error (the user
/// asked for it by name); the default OS config path is optional — most users won't have one yet.
fn load_config(explicit_path: Option<&str>) -> anyhow::Result<FileConfig> {
    match explicit_path {
        Some(p) => load_config_from_path(&PathBuf::from(p), true),
        None => match default_config_path() {
            Some(p) => load_config_from_path(&p, false),
            None => Ok(FileConfig::default()),
        },
    }
}

fn load_config_from_path(path: &std::path::Path, required: bool) -> anyhow::Result<FileConfig> {
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
fn resolve_secret(from_env: Option<String>, from_file: Option<String>) -> Option<String> {
    from_env.or(from_file)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let config = load_config(cli.config.as_deref())?;

    let provider = cli.provider.or(config.provider).unwrap_or(Provider::Gemini);
    let model_override = cli.model.or(config.model);
    let vault_path = cli.vault_path.or(config.vault_path).unwrap_or_else(|| "vault".to_string());

    let model_provider: Arc<dyn ModelProvider> = match provider {
        Provider::Gemini => {
            let api_key = resolve_secret(std::env::var("GEMINI_API_KEY").ok(), config.api_keys.gemini).context(
                "GEMINI_API_KEY not set (env var or config file) — get a free key at https://aistudio.google.com/apikey",
            )?;
            let model = model_override.unwrap_or_else(|| "gemini-2.5-flash".to_string());
            Arc::new(GeminiProvider::new(api_key, model))
        }
        Provider::Openai => {
            let api_key = resolve_secret(std::env::var("OPENAI_API_KEY").ok(), config.api_keys.openai)
                .context("OPENAI_API_KEY not set (env var or config file) — export it before running warden")?;
            let model = model_override.unwrap_or_else(|| "gpt-4o-mini".to_string());
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

    println!("Warden — talk to it below (Ctrl+D or 'exit' to quit).\n");

    let stdin = io::stdin();
    loop {
        print!("> ");
        io::stdout().flush()?;

        let mut input = String::new();
        if stdin.read_line(&mut input)? == 0 {
            break;
        }
        let input = input.trim();
        if input.is_empty() {
            continue;
        }
        if input == "exit" || input == "quit" {
            break;
        }

        match orchestrator.handle_message(input).await {
            Ok(response) => println!("{response}\n"),
            Err(err) => eprintln!("error: {err:#}\n"),
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_toml_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "warden-cli-config-test-{name}-{}.toml",
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
        // Mirrors the default (no --config flag) OS config path: most users won't have one yet,
        // so a missing file there should silently mean "no config", not an error.
        let path = temp_toml_path("does-not-exist");
        let config = load_config_from_path(&path, false).unwrap();

        assert!(config.provider.is_none());
        assert!(config.vault_path.is_none());
    }

    #[test]
    fn missing_required_path_errors() {
        // Mirrors an explicit `--config <path>`: the user named this file, so a missing file
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
    fn resolve_secret_prefers_env_over_file() {
        assert_eq!(
            resolve_secret(Some("from-env".to_string()), Some("from-file".to_string())),
            Some("from-env".to_string())
        );
        assert_eq!(resolve_secret(None, Some("from-file".to_string())), Some("from-file".to_string()));
        assert_eq!(resolve_secret(None, None), None);
    }
}
