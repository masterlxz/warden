mod telegram;

use std::path::PathBuf;

use anyhow::Context;
use clap::{Parser, ValueEnum};
use warden_bootstrap::{bootstrap, load_config, resolve_secret, Overrides};

use crate::telegram::{run_bot, TelegramClient};

#[derive(ValueEnum, Clone, Copy, Debug)]
enum Provider {
    Gemini,
    Openai,
}

impl From<Provider> for warden_bootstrap::Provider {
    fn from(p: Provider) -> Self {
        match p {
            Provider::Gemini => warden_bootstrap::Provider::Gemini,
            Provider::Openai => warden_bootstrap::Provider::Openai,
        }
    }
}

#[derive(Parser, Debug)]
#[command(name = "warden-telegram", version, about = "Warden — Telegram channel (long polling)")]
struct Cli {
    /// Path to the markdown vault (memory). Overrides the config file; defaults to
    /// ~/Warden/vault (a background process has no predictable cwd, same as the desktop app).
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

/// Same fallback the desktop app uses (`desktop/src-tauri/src/lib.rs::desktop_default_vault_path`)
/// — a background process launched by a terminal, systemd unit, etc. has no cwd a user would
/// recognize either, so relative-to-cwd (the CLI's default) isn't the right call here.
fn default_vault_path() -> PathBuf {
    dirs::home_dir().unwrap_or_default().join("Warden").join("vault")
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // bootstrap() loads the config file internally but only returns the Orchestrator — the bot
    // token isn't a model/tool secret it needs, so it's read here directly instead. Same
    // redundancy the desktop app already has between its settings commands and bootstrap()'s own
    // internal load.
    let config = load_config(cli.config.as_deref())?;
    let token = resolve_secret(std::env::var("TELEGRAM_BOT_TOKEN").ok(), config.api_keys.telegram_bot_token).context(
        "TELEGRAM_BOT_TOKEN not set (env var or config file) — create a bot via @BotFather on Telegram to get one",
    )?;

    let orchestrator = bootstrap(
        cli.config.as_deref(),
        Overrides { provider: cli.provider.map(Into::into), model: cli.model, vault_path: cli.vault_path },
        default_vault_path(),
    )
    .await?;

    let conversations_dir = warden_bootstrap::default_telegram_conversations_dir()
        .context("could not determine the OS config directory for conversations")?;

    let api = TelegramClient::new(token);
    println!("Warden Telegram bot is running (long polling)...");
    run_bot(&api, &orchestrator, &conversations_dir).await
}
