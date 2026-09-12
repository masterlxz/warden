use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use clap::{Parser, Subcommand, ValueEnum};
use warden_bootstrap::{bootstrap, Overrides};
use warden_server::{PairingStore, Server};

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
#[command(name = "warden-server", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Starts listening for client connections (the long-running hub process).
    Serve(ServeArgs),
    /// Manage the persistent device pairing registry (Fase 9.3) — `CallDeviceTool` routing only
    /// works between devices that have been `approve`d here; a running `serve` process picks up
    /// changes made this way without needing a restart.
    Devices {
        #[command(subcommand)]
        action: DevicesAction,
    },
}

#[derive(Subcommand, Debug)]
enum DevicesAction {
    /// List every device that has ever said `Hello`, with its pairing status.
    List,
    /// Approve a device for routing — it must have connected at least once already.
    Approve { device_id: String },
    /// Revoke a previously approved device — blocks its next routing attempt, doesn't force-close
    /// an already-open connection.
    Revoke { device_id: String },
}

/// Warden's server-side WebSocket endpoint (Fase 9/7.3): hosts a real `Orchestrator` (same
/// `bootstrap()` every other channel uses) and answers chat over `ws://`.
///
/// Runs over plain `ws://` — encryption is expected to come from the Tailscale tunnel (Fase
/// 9.1), not from this listener. Only tested over localhost so far; there is no real tailnet
/// in the dev environment this was built in.
#[derive(Parser, Debug)]
struct ServeArgs {
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

fn devices_path() -> anyhow::Result<PathBuf> {
    warden_bootstrap::default_server_devices_path().context("could not determine the OS config directory for the device registry")
}

fn run_devices_command(action: DevicesAction) -> anyhow::Result<()> {
    let store = PairingStore::new(devices_path()?);
    match action {
        DevicesAction::List => {
            let devices = store.list()?;
            if devices.is_empty() {
                println!("no devices have connected to this server yet");
                return Ok(());
            }
            for (device_id, device) in devices {
                println!("{device_id}\t{}\t{}\tfirst seen {}\tlast seen {}", device.device_name, device.status, device.first_seen_ms, device.last_seen_ms);
            }
        }
        DevicesAction::Approve { device_id } => {
            store.approve(&device_id)?;
            println!("device '{device_id}' approved — it can now be used as a CallDeviceTool caller or target");
        }
        DevicesAction::Revoke { device_id } => {
            store.revoke(&device_id)?;
            println!("device '{device_id}' revoked — its next routing attempt (as caller or target) will fail");
        }
    }
    Ok(())
}

async fn run_serve(args: ServeArgs) -> anyhow::Result<()> {
    let auth_key = std::env::var("WARDEN_SERVER_AUTH_KEY")
        .ok()
        .or(args.auth_key.clone())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no auth key configured — set WARDEN_SERVER_AUTH_KEY or pass --auth-key"
            )
        })?;

    let orchestrator = bootstrap(
        args.config.as_deref(),
        Overrides { provider: args.provider.map(Into::into), model: args.model.clone(), vault_path: args.vault_path.clone(), ..Default::default() },
        default_vault_path(),
    )
    .await?;

    let conversations_dir = warden_bootstrap::default_server_conversations_dir()
        .context("could not determine the OS config directory for conversations")?;

    let server = Server::bind(args.listen, auth_key, Arc::new(orchestrator), conversations_dir, devices_path()?).await?;
    let addr = server.local_addr()?;
    eprintln!("warden-server: listening on {addr}");
    server.serve().await
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Serve(args) => run_serve(args).await,
        Command::Devices { action } => run_devices_command(action),
    }
}
