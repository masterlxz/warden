use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use clap::{Parser, ValueEnum};
use warden_bootstrap::{bootstrap, Overrides};
use warden_server::Server;

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

/// Warden's server-side WebSocket endpoint (Fase 9/7.3): hosts a real `Orchestrator` (same
/// `bootstrap()` every other channel uses) and answers chat over `ws://`.
///
/// Runs over plain `ws://` — encryption is expected to come from the Tailscale tunnel (Fase
/// 9.1), not from this listener. Only tested over localhost so far; there is no real tailnet
/// in the dev environment this was built in.
#[derive(Parser, Debug)]
#[command(name = "warden-server", version, about)]
struct Cli {
    /// Address to listen on.
    #[arg(long, default_value = "0.0.0.0:7420")]
    listen: SocketAddr,

    /// Shared secret clients must present in their Hello message. Falls back to
    /// WARDEN_SERVER_AUTH_KEY if not passed (env wins if both are set).
    #[arg(long)]
    auth_key: Option<String>,

    /// Path to the markdown vault (memory). Overrides the config file; defaults to
    /// ~/Warden/vault (a background process has no predictable cwd, same as warden-telegram).
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

/// Same fallback warden-telegram/the desktop app use — a background process launched by a
/// terminal, systemd unit, etc. has no cwd a user would recognize either.
fn default_vault_path() -> PathBuf {
    dirs::home_dir().unwrap_or_default().join("Warden").join("vault")
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let auth_key = std::env::var("WARDEN_SERVER_AUTH_KEY")
        .ok()
        .or(cli.auth_key.clone())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no auth key configured — set WARDEN_SERVER_AUTH_KEY or pass --auth-key"
            )
        })?;

    let orchestrator = bootstrap(
        cli.config.as_deref(),
        Overrides { provider: cli.provider.map(Into::into), model: cli.model.clone(), vault_path: cli.vault_path.clone(), ..Default::default() },
        default_vault_path(),
    )
    .await?;

    let conversations_dir = warden_bootstrap::default_server_conversations_dir()
        .context("could not determine the OS config directory for conversations")?;

    let server = Server::bind(cli.listen, auth_key, Arc::new(orchestrator), conversations_dir).await?;
    let addr = server.local_addr()?;
    eprintln!("warden-server: listening on {addr}");
    server.serve().await
}
