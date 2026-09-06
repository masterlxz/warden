use std::net::SocketAddr;

use clap::Parser;
use warden_server::Server;

/// Warden's server-side WebSocket endpoint (Fase 9).
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
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let auth_key = std::env::var("WARDEN_SERVER_AUTH_KEY")
        .ok()
        .or(cli.auth_key)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no auth key configured — set WARDEN_SERVER_AUTH_KEY or pass --auth-key"
            )
        })?;

    let server = Server::bind(cli.listen, auth_key).await?;
    let addr = server.local_addr()?;
    eprintln!("warden-server: listening on {addr}");
    server.serve().await
}
