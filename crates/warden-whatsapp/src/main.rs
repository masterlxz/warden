mod sidecar;

use std::path::PathBuf;

use anyhow::Context;
use clap::{Parser, ValueEnum};
use warden_bootstrap::{bootstrap, Overrides};

use crate::sidecar::{run_bot, ChildSidecar};

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
#[command(name = "warden-whatsapp", version, about = "Warden — WhatsApp channel (Baileys sidecar)")]
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

    /// Path to the Baileys sidecar script. Defaults to `sidecar/whatsapp/index.mjs` relative to
    /// this crate's own source — a dev-mode default; packaging this for a real distribution build
    /// is future work (see PENDING.md), same as the rest of the CLI-style binaries today.
    #[arg(long)]
    sidecar_script: Option<String>,
}

/// Same fallback the desktop app uses (`desktop/src-tauri/src/lib.rs::desktop_default_vault_path`)
/// — a background process launched by a terminal, systemd unit, etc. has no cwd a user would
/// recognize either, so relative-to-cwd (the CLI's default) isn't the right call here.
fn default_vault_path() -> PathBuf {
    dirs::home_dir().unwrap_or_default().join("Warden").join("vault")
}

/// Where the sidecar's `useMultiFileAuthState` persists WhatsApp session credentials — opaque
/// app data (not vault-worthy), same OS config dir as everything else in `warden-bootstrap`.
/// Kept local to this crate rather than in `warden-bootstrap`: it's a sidecar implementation
/// detail, not a "conversation" concept like `default_whatsapp_conversations_dir`.
fn default_whatsapp_auth_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("warden").join("whatsapp-auth"))
}

fn default_sidecar_script_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../sidecar/whatsapp/index.mjs")
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let orchestrator = bootstrap(
        cli.config.as_deref(),
        Overrides { provider: cli.provider.map(Into::into), model: cli.model, vault_path: cli.vault_path },
        default_vault_path(),
    )
    .await?;

    let conversations_dir = warden_bootstrap::default_whatsapp_conversations_dir()
        .context("could not determine the OS config directory for conversations")?;
    let auth_dir = default_whatsapp_auth_dir().context("could not determine the OS config directory for the WhatsApp session")?;
    let script_path = cli.sidecar_script.map(PathBuf::from).unwrap_or_else(default_sidecar_script_path);

    let mut sidecar = ChildSidecar::spawn(&script_path, &auth_dir).await?;
    println!("Warden WhatsApp bot is starting — scan the QR code below with your phone if this is the first run.");
    run_bot(&mut sidecar, &orchestrator, &conversations_dir).await
}
