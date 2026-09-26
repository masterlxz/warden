use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use clap::{Parser, Subcommand, ValueEnum};
use warden_bootstrap::auto_sync::{SyncBackend, SyncRunner, AUTO_SYNC_INTERVAL};
use warden_bootstrap::{bootstrap, Overrides};
use warden_server::chat_input::WhisperTranscriber;
use warden_server::{resolve_server_name, EmbeddedWebUi, HubTls, PairingStore, Server, WebAssets};

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
    /// Prints a fresh random pairing key (64 hex chars) for `serve --auth-key` /
    /// WARDEN_SERVER_AUTH_KEY.
    GenKey,
    /// Vault sync (P61) — what `serve` does every 5 minutes on its own, and how a hub with no
    /// screen gets its vault key. While `serve` runs, the web's Sync screen does the same without
    /// two processes syncing at once.
    Sync(SyncArgs),
}

#[derive(clap::Args, Debug)]
struct SyncArgs {
    #[command(subcommand)]
    action: SyncCommand,

    /// Path to the config file (TOML), as in `serve`.
    #[arg(long, global = true)]
    config: Option<String>,

    /// Path to the markdown vault, as in `serve`.
    #[arg(long, global = true)]
    vault_path: Option<String>,
}

#[derive(Subcommand, Debug)]
enum SyncCommand {
    /// Where the vault syncs to, what's waiting to go, and when it last synced.
    Status,
    /// Runs one round now: git pulls then pushes, Arweave only pulls.
    Now,
    /// Makes this hub the first device of a sync group (a fresh vault key).
    Init,
    /// Receives the vault key from a device showing a pairing code (the desktop's Sync screen, or
    /// `/sync pair` in the CLI).
    Pair {
        code: String,
        /// That device's IPv4 address (a Tailscale one works) — needed whenever it isn't on this
        /// hub's LAN, which the default sweep covers.
        #[arg(long)]
        host: Option<std::net::Ipv4Addr>,
    },
}

#[derive(Subcommand, Debug)]
enum DevicesAction {
    /// List every device that has ever said `Hello`, with its pairing status.
    List,
    /// Approve a device for routing — it must have connected at least once already.
    Approve { device_id: String },
    /// Revoke a device — its token stops working (so does re-pairing under the same id), and a
    /// running `serve` closes its open connection within a few seconds. To also keep it from
    /// pairing again under a *new* id, rotate the pairing key (`--auth-key`) and restart.
    Revoke { device_id: String },
}

/// Warden's server-side WebSocket endpoint (Fase 9/7.3): hosts a real `Orchestrator` (same
/// `bootstrap()` every other channel uses) and answers chat over `ws://`, or only over `wss://`
/// once a certificate is configured (P36 — `--tailscale-cert`, or `--tls-cert`/`--tls-key`).
#[derive(Parser, Debug)]
struct ServeArgs {
    /// Address to listen on.
    #[arg(long, default_value = "0.0.0.0:7420")]
    listen: SocketAddr,

    /// Serve only wss://, with a certificate for this machine's Tailscale MagicDNS name, fetched
    /// via `tailscale cert` at startup and renewed daily (no restart needed). Needs HTTPS
    /// certificates enabled for the tailnet; clients then connect to wss://<name>.<tailnet>.ts.net.
    #[arg(long, conflicts_with_all = ["tls_cert", "tls_key"])]
    tailscale_cert: bool,

    /// Serve only wss://, with this PEM certificate chain (leaf first). Re-read whenever the file
    /// changes, so renewing it needs no restart. Requires --tls-key.
    #[arg(long, requires = "tls_key")]
    tls_cert: Option<PathBuf>,

    /// PEM private key for --tls-cert.
    #[arg(long, requires = "tls_cert")]
    tls_key: Option<PathBuf>,

    /// Host name --tls-cert is valid for, advertised to LAN discovery so clients know which
    /// wss:// URL to use. Optional; --tailscale-cert fills it in by itself.
    #[arg(long, requires = "tls_cert")]
    tls_host: Option<String>,

    /// Don't serve the web interface (P78). By default, opening this hub's address in a browser
    /// (http://, or https:// with TLS) shows the Warden web UI, which pairs as one more device.
    #[arg(long)]
    no_web_ui: bool,

    /// Pairing key — what a new client presents in its first Hello to get its own device token
    /// (P36). Devices already holding a token keep working if this changes, so rotating it only
    /// affects new pairings. Falls back to WARDEN_SERVER_AUTH_KEY if not passed (env wins if both
    /// are set). Must be at least 32 characters; `warden-server gen-key` prints one.
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

    /// Display name this hub answers with in HelloAck and to a LAN discovery sweep (Fase 9.1,
    /// redefined — see `discover_hubs`). Falls back to WARDEN_SERVER_NAME, then the OS hostname.
    #[arg(long)]
    server_name: Option<String>,
}

/// Same fallback warden-telegram/the desktop app use — a background process launched by a
/// terminal, systemd unit, etc. has no cwd a user would recognize either.
fn default_vault_path() -> PathBuf {
    dirs::home_dir().unwrap_or_default().join("Warden").join("vault")
}


/// The runner `serve` and `sync` share: this hub's config file and the vault it resolves to.
fn sync_runner(config: Option<&str>, vault_path: Option<String>) -> anyhow::Result<SyncRunner> {
    let config_path = config.map(PathBuf::from).or_else(warden_bootstrap::default_config_path).context("could not determine the OS config directory")?;
    let file = warden_bootstrap::load_config_from_path(&config_path, config.is_some())?;
    let overrides = Overrides { vault_path, ..Default::default() };
    let vault_path = warden_bootstrap::resolve_vault_path(&overrides, &file, default_vault_path());
    Ok(SyncRunner::with_default_paths(vault_path, config_path))
}

async fn run_sync_command(args: SyncArgs) -> anyhow::Result<()> {
    let runner = sync_runner(args.config.as_deref(), args.vault_path)?;
    match args.action {
        SyncCommand::Status => {}
        SyncCommand::Now => {
            let report = runner.run_once().await;
            if let Some(p) = &report.pulled {
                println!("pulled: {} written, {} removed{}", p.files_written, p.files_deleted, if p.config_updated { ", config.toml updated" } else { "" });
            }
            if let Some(p) = &report.pushed {
                println!("pushed: commit {} ({} file(s))", &p.commit_sha[..12.min(p.commit_sha.len())], p.files_changed);
            }
            if let Some(err) = report.error {
                anyhow::bail!(err);
            }
            if report.pulled.is_none() && report.pushed.is_none() {
                println!("nothing to sync");
            }
        }
        SyncCommand::Init => {
            runner.init_fresh().await?;
            println!("vault key created — other devices pair with this one to get it");
        }
        SyncCommand::Pair { code, host } => {
            runner.pair_join(&code, host).await?;
            println!("paired — this hub now has the vault key");
        }
    }
    let state = runner.state()?;
    let backend = match state.backend {
        SyncBackend::NotSetUp => "not set up (no vault key yet: `warden-server sync init` or `sync pair <code>`)",
        SyncBackend::Git => "git",
        SyncBackend::Arweave => "Arweave (pull only here; pushing needs the TruthID phone)",
    };
    println!("backend: {backend}");
    if let Some(remote) = &state.git_remote {
        println!("git remote: {remote}");
    }
    println!(
        "pending: {} file(s){}",
        state.pending_vault_changes,
        if state.pending_config_changed { " + config.toml" } else { "" }
    );
    match state.last_synced_at_ms {
        Some(ms) => {
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(ms);
            println!("last synced: {} min ago", (now - ms).max(0) / 60_000);
        }
        None => println!("last synced: never"),
    }
    if state.backend == SyncBackend::Arweave {
        println!("no [git_sync] in the config: set one to sync both ways from this hub");
    }
    Ok(())
}

/// Settings over the network (P78) for this process: the same config file and the same flags it
/// started with, so a reload after a save builds exactly what a restart would.
struct ServeSettings {
    config_path: PathBuf,
    explicit_config: Option<String>,
    overrides: Overrides,
}

#[async_trait::async_trait]
impl warden_server::SettingsHost for ServeSettings {
    fn config_path(&self) -> PathBuf {
        self.config_path.clone()
    }

    async fn build(&self) -> anyhow::Result<warden_core::orchestrator::Orchestrator> {
        bootstrap(self.explicit_config.as_deref(), self.overrides.clone(), default_vault_path()).await
    }

    fn notes(&self) -> Vec<String> {
        let mut notes = Vec::new();
        if let Some(model) = &self.overrides.model {
            notes.push(format!("This hub was started with --model {model}, which wins over the active provider's model."));
        }
        if let Some(provider) = self.overrides.provider {
            notes.push(format!("This hub was started with --provider {provider:?}, which only applies while no providers are saved."));
        }
        notes
    }
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
            println!("device '{device_id}' revoked — its token no longer works and a running server closes its connection shortly");
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
    // P83 — the pairing key also guards saving settings from the web, so a short one is refused.
    if !warden_bootstrap::is_strong_auth_key(&auth_key) {
        anyhow::bail!(
            "the auth key must be at least {} characters — generate one with `warden-server gen-key` and use it in \
             WARDEN_SERVER_AUTH_KEY or --auth-key (paired devices keep working; only new pairings need the new key)",
            warden_bootstrap::MIN_AUTH_KEY_LEN
        );
    }

    let overrides = Overrides { provider: args.provider.map(Into::into), model: args.model.clone(), vault_path: args.vault_path.clone(), ..Default::default() };
    let orchestrator = bootstrap(args.config.as_deref(), overrides.clone(), default_vault_path()).await?;
    let settings = args.config.as_ref().map(PathBuf::from).or_else(warden_bootstrap::default_config_path).map(|config_path| ServeSettings {
        config_path,
        explicit_config: args.config.clone(),
        overrides,
    });

    // P61 — the vault syncs on its own every few minutes, and the web's Sync screen drives it too.
    let runner = Arc::new(sync_runner(args.config.as_deref(), args.vault_path.clone())?);

    let conversations_dir = warden_bootstrap::default_server_conversations_dir()
        .context("could not determine the OS config directory for conversations")?;

    let tls = resolve_tls(&args).await?;

    let server_name = resolve_server_name(args.server_name.clone());
    let mut server = Server::bind(args.listen, auth_key, server_name.clone(), Arc::new(orchestrator), conversations_dir, devices_path()?)
        .await?
        // P78 — voice input from the web UI, with the Whisper key from the same config file.
        .with_transcriber(Arc::new(WhisperTranscriber::new(args.config.as_ref().map(PathBuf::from))))
        .with_sync(runner, Some(AUTO_SYNC_INTERVAL));
    if let Some(settings) = settings {
        server = server.with_settings(Arc::new(settings));
    }
    let addr = server.local_addr()?;
    let page_url = match tls.as_ref().and_then(|tls| tls.secure_url(addr.port())) {
        Some(url) => url.replacen("wss://", "https://", 1),
        None if tls.is_some() => format!("https://<this hub's TLS name>:{}", addr.port()),
        None => format!("http://{addr}"),
    };
    match tls {
        Some(tls) => {
            match tls.secure_url(addr.port()) {
                Some(url) => eprintln!("warden-server: listening on {addr} as '{server_name}' — TLS only, clients connect to {url}"),
                None => eprintln!("warden-server: listening on {addr} as '{server_name}' — TLS only"),
            }
            server = server.with_tls(tls);
        }
        None => eprintln!("warden-server: listening on {addr} as '{server_name}' — plain ws://, not encrypted (see --tailscale-cert)"),
    }
    if !args.no_web_ui {
        if EmbeddedWebUi.get("index.html").is_some() {
            eprintln!("warden-server: web interface at {page_url}");
        } else {
            eprintln!("warden-server: this build has no web interface (run `npm run build` in web/ and rebuild) — pages answer 503");
        }
        server = server.with_web_ui(Arc::new(EmbeddedWebUi));
    }
    server.serve().await
}

/// `--tailscale-cert` fetches the cert first (and spawns its daily renewal); `--tls-cert`/
/// `--tls-key` just load what's there. `None` = plain `ws://`, as before P36's second slice.
async fn resolve_tls(args: &ServeArgs) -> anyhow::Result<Option<HubTls>> {
    if args.tailscale_cert {
        let dir = warden_bootstrap::default_tls_dir().context("could not determine the OS config directory for the TLS certificate")?;
        let (hub_tls, cert) = HubTls::from_tailscale(&dir).await?;
        tokio::spawn(cert.renewal());
        return Ok(Some(hub_tls));
    }
    match (&args.tls_cert, &args.tls_key) {
        (Some(cert), Some(key)) => Ok(Some(HubTls::from_pem_files(cert, key, args.tls_host.clone())?)),
        _ => Ok(None),
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Serve(args) => run_serve(args).await,
        Command::Devices { action } => run_devices_command(action),
        Command::Sync(args) => run_sync_command(args).await,
        Command::GenKey => {
            println!("{}", warden_bootstrap::generate_auth_key());
            Ok(())
        }
    }
}
