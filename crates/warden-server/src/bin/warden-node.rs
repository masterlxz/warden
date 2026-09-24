use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use warden_bootstrap::{default_client_device_tokens_path, load_config, resolve_vault_path, Overrides};
use warden_core::memory::Vault;
use warden_server::{vault_node, DeviceTokenStore};

/// The target side of P61's `RemoteNodeProvider` (`crates/warden-server-protocol`) — connects to a
/// `warden-server` hub as a client and serves `vault_read`/`vault_write`/`vault_list`/
/// `vault_delete` against this machine's own vault, so another Warden install can select
/// `storage_provider = "remote_node"` and point `[remote_node]` at this node's `device_id`.
///
/// Encrypted when `--server-url` is `wss://` (a hub started with `--tailscale-cert` or
/// `--tls-cert`), verified against the public web roots; plain `ws://` otherwise.
#[derive(Parser, Debug)]
#[command(name = "warden-node", version, about)]
struct Cli {
    /// The `warden-server` hub to connect through, e.g. `wss://hub.tail1234.ts.net:7420` (TLS hub)
    /// or `ws://192.168.x.x:7420`.
    #[arg(long)]
    server_url: String,

    /// This node's own id — what a `RemoteNodeProvider` elsewhere names as its `target_device_id`.
    #[arg(long)]
    device_id: String,

    #[arg(long)]
    device_name: String,

    /// The hub's pairing key, for the first `Hello`. Falls back to WARDEN_SERVER_AUTH_KEY if not
    /// passed (env wins if both are set) — same precedence `warden-server`'s own CLI uses. Only
    /// needed until this node holds a device token for the hub (P36, kept in
    /// `device_tokens.json` in the OS config dir).
    #[arg(long)]
    auth_key: Option<String>,

    /// Path to the markdown vault this node serves. Overrides the config file; defaults to
    /// ~/Warden/vault (a background process has no predictable cwd, same as every other channel).
    #[arg(long)]
    vault_path: Option<String>,

    /// Path to the config file (TOML). Defaults to the OS config dir.
    #[arg(long)]
    config: Option<String>,
}

fn default_vault_path() -> PathBuf {
    dirs::home_dir().unwrap_or_default().join("Warden").join("vault")
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let tokens = DeviceTokenStore::new(
        default_client_device_tokens_path().ok_or_else(|| anyhow::anyhow!("could not determine the OS config directory"))?,
    );
    let auth_key = std::env::var("WARDEN_SERVER_AUTH_KEY").ok().or(cli.auth_key.clone());
    if auth_key.is_none() && tokens.get(&cli.server_url, &cli.device_id)?.is_none() {
        anyhow::bail!("not paired with this hub yet and no auth key configured — set WARDEN_SERVER_AUTH_KEY or pass --auth-key");
    }

    let config = load_config(cli.config.as_deref())?;
    let vault_path = resolve_vault_path(&Overrides { vault_path: cli.vault_path.clone(), ..Default::default() }, &config, default_vault_path());
    let vault = Arc::new(Vault::new(vault_path.clone()));

    let conn = vault_node::connect(&cli.server_url, &cli.device_id, &cli.device_name, auth_key.as_deref().unwrap_or_default(), &tokens).await?;
    eprintln!("warden-node: connected to {} as '{}', serving vault at {}", cli.server_url, cli.device_id, vault_path.display());
    vault_node::serve(conn, vault).await
}
