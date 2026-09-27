use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use clap::{Parser, Subcommand, ValueEnum};
use warden_bootstrap::auto_sync::{SyncBackend, SyncRunner, AUTO_SYNC_INTERVAL, PAIRING_PORTS, PAIRING_TIMEOUT};
use warden_bootstrap::tasks::{check_tasks, next_run, run_task, task_status, TaskStore, Zone};
use warden_bootstrap::{bootstrap, load_config_from_path, save_config, Overrides, TaskConfig};
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
    /// The Warden API's keys (P12): what an OpenAI-compatible client presents as its bearer token
    /// to `http(s)://<hub>/v1`. A running `serve` sees a change on the next request.
    ApiKeys {
        #[command(subcommand)]
        action: ApiKeysAction,
    },
    /// Prints a fresh random pairing key (64 hex chars) for `serve --auth-key` /
    /// WARDEN_SERVER_AUTH_KEY.
    GenKey,
    /// Vault sync (P61) — what `serve` does every 5 minutes on its own, and how a hub with no
    /// screen gets its vault key. Safe while `serve` runs: a round started here waits for the
    /// hub's to finish (one sync at a time per vault, across processes).
    Sync(SyncArgs),
    /// Scheduled tasks (P92): prompts an agent runs on its own, kept as `[[tasks]]` in the config
    /// file. Only a hub started with `serve --run-tasks` runs them; each run lands in the task's
    /// conversation, which every device lists. A running `serve` sees changes made here within
    /// half a minute.
    Tasks(TasksArgs),
    /// Makes this machine a node (P93): it connects to a hub and lends its shell and/or a folder of
    /// files to the hub's agents. The hub still decides who may use it (`warden-server nodes`).
    Node(NodeArgs),
    /// On the hub: what agents may do with each node (P93) — the `[[nodes]]` entries.
    Nodes {
        #[command(subcommand)]
        action: NodesAction,
        /// Path to the config file (TOML), as in `serve`.
        #[arg(long, global = true)]
        config: Option<String>,
    },
    /// The people of this workspace besides you (P84), kept as `[[users]]` in the config file. Each
    /// signs in with their username and password (the web, the phone) and has their own vault and
    /// conversations on this hub. You stay the owner: whoever holds the pairing key.
    Users {
        #[command(subcommand)]
        action: UsersAction,
        /// Path to the config file (TOML), as in `serve`.
        #[arg(long, global = true)]
        config: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
enum UsersAction {
    /// Every member, and whether they still have to pick their own password.
    List,
    /// Adds a member with a provisional password (printed once) they change when they first sign in.
    Add {
        /// Their username: lowercase letters, digits, - or _.
        id: String,
        /// How they're shown.
        #[arg(long)]
        name: String,
    },
    /// Gives a member a new provisional password (printed once), for a forgotten one.
    ResetPassword { id: String },
    /// Removes a member and revokes their devices. Their vault and conversations stay on disk.
    Remove { id: String },
}

#[derive(clap::Args, Debug)]
struct NodeArgs {
    /// The hub to join, e.g. wss://hub.tailnet.ts.net:7420 (or ws:// on a trusted network).
    #[arg(long)]
    hub: String,
    /// The hub's pairing key — only the first time; the node keeps the token the hub gives it.
    /// Falls back to WARDEN_SERVER_AUTH_KEY.
    #[arg(long)]
    auth_key: Option<String>,
    /// How this node shows up on the hub. Defaults to the machine's host name.
    #[arg(long)]
    name: Option<String>,
    /// One line for the agents: what this machine is and what it's good for.
    #[arg(long, default_value = "")]
    description: String,
    /// A label agents can pick nodes by, like gpu or home. Repeatable.
    #[arg(long = "tag")]
    tags: Vec<String>,
    /// Lend this machine's shell: agents allowed on the hub can run any command as this user.
    #[arg(long)]
    shell: bool,
    /// Share this folder: agents allowed on the hub can read and write text files inside it (never
    /// outside). Also where --shell commands start.
    #[arg(long)]
    files: Option<PathBuf>,
    /// Lend one of this machine's MCP servers, by its name in `[[mcp_servers]]` of this machine's
    /// config file. Repeatable; a server not named here isn't lent.
    #[arg(long = "mcp")]
    mcp: Vec<String>,
    /// Lend one of this machine's model providers (its local Ollama, say), by its id in
    /// `[[providers]]` of this machine's config file. Repeatable. On the hub, a `[[providers]]` entry
    /// with `kind = "node"`, this node's id and that provider's id uses it.
    #[arg(long = "model")]
    models: Vec<String>,
    /// This machine's config file (TOML), where --mcp looks the servers up. Defaults to the OS config dir.
    #[arg(long)]
    config: Option<String>,
}

#[derive(Subcommand, Debug)]
enum NodesAction {
    /// Every node in `[[nodes]]` and whether it's on, for which agents, and whether it asks first.
    List,
    /// Lets agents use this node (its device id, as `devices list` shows it). It must also be approved
    /// there (`devices approve`).
    Allow {
        id: String,
        /// Only these agents (repeatable). Without it, every agent.
        #[arg(long = "agent")]
        agents: Vec<String>,
        /// Ask a person before every command or file operation on it.
        #[arg(long)]
        approval: bool,
    },
    /// Stops agents from using this node (keeps its entry).
    Deny { id: String },
}

#[derive(clap::Args, Debug)]
struct TasksArgs {
    #[command(subcommand)]
    action: TasksCommand,

    /// Path to the config file (TOML), as in `serve`.
    #[arg(long, global = true)]
    config: Option<String>,
}

#[derive(Subcommand, Debug)]
enum TasksCommand {
    /// Every task: its schedule, agent, whether it's on, the last run and the next one.
    List,
    /// Adds a task. Give exactly one of --every, --cron or --once.
    #[command(group(clap::ArgGroup::new("schedule").required(true).args(["every", "cron", "once"])))]
    Add {
        /// 1-59 letters, digits, '-' or '_'.
        id: String,
        /// What the agent is asked on every run.
        #[arg(long)]
        prompt: String,
        /// The agent that runs it (none: no persona).
        #[arg(long)]
        agent: Option<String>,
        /// An interval counted from the last run: 30m, 2h, 1d.
        #[arg(long)]
        every: Option<String>,
        /// Five-field cron, e.g. "0 8 * * 1-5" for 8:00 on weekdays.
        #[arg(long)]
        cron: Option<String>,
        /// Once, at this local date and time: 2026-10-01T09:00.
        #[arg(long)]
        once: Option<String>,
        /// IANA time zone for --cron and --once (e.g. America/Sao_Paulo). Default: this machine's.
        #[arg(long)]
        timezone: Option<String>,
    },
    /// Stops a task from running until `resume`.
    Pause { id: String },
    /// Switches a paused task back on; it counts from now, without making up for what it skipped.
    Resume { id: String },
    /// Removes a task. Its conversation stays, for whoever wants to read it.
    Remove { id: String },
    /// Runs a task now, in this process, and prints the answer. Devices connected to a running
    /// `serve` see it when they next reload their conversation list.
    Run {
        id: String,
        /// Path to the markdown vault, as in `serve`.
        #[arg(long)]
        vault_path: Option<String>,
    },
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
    /// Shows a pairing code another device joins this hub's sync group with, and waits for it.
    /// Safe while `serve` runs: it only reads the vault key.
    Host,
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

#[derive(Subcommand, Debug)]
enum ApiKeysAction {
    /// Every key: id, name, its first characters, when it was created and last used.
    List,
    /// A new key named NAME — printed once, never stored.
    Create {
        name: String,
        /// Binds the key to this agent: it then only speaks as it (`warden` and `warden/<this
        /// agent>` both mean it, any other agent is refused). Without it the key is general.
        #[arg(long)]
        agent: Option<String>,
        /// The config file the agent is checked against, as in `serve`.
        #[arg(long)]
        config: Option<String>,
    },
    /// Removes a key; clients using it get 401 from the next request on.
    Revoke { id: String },
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

    /// Run the scheduled tasks (`warden-server tasks`, P92) on this hub. Off by default: the config
    /// file syncs, so turn it on in exactly one hub — the one that's always up.
    #[arg(long)]
    run_tasks: bool,
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
        SyncCommand::Host => {
            let host = runner.pairing_host().await?;
            let port = host.local_addr()?.port();
            println!("pairing code: {}", host.code());
            println!(
                "type it on the other device (desktop Sync screen, or `/sync pair <code> <this hub's IP>` in the terminal); \
                 listening on port {port} for {} min",
                PAIRING_TIMEOUT.as_secs() / 60
            );
            println!("from outside this hub's LAN, give the other device this hub's IP (a Tailscale one works) and allow ports {}-{}", PAIRING_PORTS[0], PAIRING_PORTS[PAIRING_PORTS.len() - 1]);
            host.wait_for_join().await?;
            println!("paired — the other device now has the vault key");
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

fn tasks_store() -> anyhow::Result<TaskStore> {
    let dir = warden_bootstrap::default_server_tasks_dir().context("could not determine the OS config directory for scheduled tasks")?;
    Ok(TaskStore::new(dir))
}

fn now_millis() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or_default()
}

async fn run_node_command(args: NodeArgs) -> anyhow::Result<()> {
    anyhow::ensure!(
        args.shell || args.files.is_some() || !args.mcp.is_empty() || !args.models.is_empty(),
        "a node has to lend something — pass --shell, --files <folder>, --mcp <server>, --model <provider>, or a mix"
    );
    if let Some(dir) = &args.files {
        anyhow::ensure!(dir.is_dir(), "--files {} is not a folder", dir.display());
    }
    let name = args.name.clone().unwrap_or_else(|| resolve_server_name(None));
    let identity_path = warden_server::node_client::default_node_identity_path().context("could not determine the OS config directory")?;
    let identity = warden_server::node_client::NodeIdentity::load_or_create(&identity_path, &name)?;
    let config_path = args.config.as_deref().map(PathBuf::from).or_else(warden_bootstrap::default_config_path).context("could not determine the OS config directory")?;
    let mcp_tools = warden_server::node_client::lend_mcp_servers(&args.mcp, &config_path, args.config.is_some()).await?;
    let models = warden_server::node_client::lend_models(&args.models, &config_path, args.config.is_some())?;
    let local = std::sync::Arc::new(warden_server::node_client::LocalNode::new(args.shell, args.files.clone()).with_mcp_tools(mcp_tools).with_models(models));
    let session = warden_server::node_client::NodeSession {
        hub_url: args.hub.clone(),
        name,
        auth_key: std::env::var("WARDEN_SERVER_AUTH_KEY").ok().or(args.auth_key.clone()).unwrap_or_default(),
        offer: local.offer(args.description.clone(), args.tags.clone()),
        identity_path: Some(identity_path),
    };
    let mut lent = Vec::new();
    if args.shell {
        lent.push("its shell".to_string());
    }
    if let Some(dir) = &args.files {
        lent.push(format!("the folder {}", dir.display()));
    }
    if !args.mcp.is_empty() {
        lent.push(format!("{} MCP tool(s) from {}", session_tool_count(&local), args.mcp.join(", ")));
    }
    if !args.models.is_empty() {
        lent.push(format!("the model(s) {}", args.models.join(", ")));
    }
    let lends = lent.join(", ");
    eprintln!("warden-server node: '{}' ({}) lends {lends}", session.name, identity.device_id);
    warden_server::node_client::run_node(session, identity, local, None).await
}

fn session_tool_count(local: &warden_server::node_client::LocalNode) -> usize {
    local.offer(String::new(), Vec::new()).mcp_tools.len()
}

fn run_users_command(action: UsersAction, config: Option<String>) -> anyhow::Result<()> {
    use warden_bootstrap::users::{add_user, generate_temp_password, remove_user, reset_password};
    let config_path = config.as_deref().map(PathBuf::from).or_else(warden_bootstrap::default_config_path).context("could not determine the OS config directory")?;
    let mut file = load_config_from_path(&config_path, config.is_some())?;
    match action {
        UsersAction::List => {
            if file.users.is_empty() {
                println!("no one else yet — add someone with `warden-server users add <username> --name \"Their Name\"`");
            }
            for user in &file.users {
                let password = if user.must_change_password { "provisional password" } else { "own password" };
                println!("{}\t{}\t{password}", user.id, user.name);
            }
        }
        UsersAction::Add { id, name } => {
            let password = generate_temp_password();
            add_user(&mut file, &id, &name, &password)?;
            save_config(&config_path, &file)?;
            let id = &file.users.last().expect("just added").id;
            println!("'{id}' added. Provisional password (shown only now): {password}");
            println!("They sign in on the web or the phone with the username '{id}' and this password, then pick their own.");
        }
        UsersAction::ResetPassword { id } => {
            let password = generate_temp_password();
            reset_password(&mut file, &id, &password)?;
            save_config(&config_path, &file)?;
            println!("New provisional password for '{id}' (shown only now): {password}");
        }
        UsersAction::Remove { id } => {
            remove_user(&mut file, &id)?;
            save_config(&config_path, &file)?;
            let revoked = PairingStore::new(devices_path()?).revoke_user_devices(&id)?;
            println!("'{id}' removed and {revoked} device(s) of theirs revoked — their vault and conversations stay on disk");
        }
    }
    Ok(())
}

fn run_nodes_command(action: NodesAction, config: Option<String>) -> anyhow::Result<()> {
    let config_path = config.as_ref().map(PathBuf::from).or_else(warden_bootstrap::default_config_path).context("could not determine the OS config directory")?;
    match action {
        NodesAction::List => {
            let config = load_config_from_path(&config_path, config.is_some())?;
            if config.nodes.is_empty() {
                println!("no nodes allowed yet — run `warden-server node` on a machine, approve it (`devices approve <id>`), then `nodes allow <id>`");
            }
            for node in &config.nodes {
                let who = if node.agents.is_empty() { "every agent".to_string() } else { node.agents.join(", ") };
                println!("{}\t{}\t{who}\t{}", node.id, if node.enabled { "on" } else { "off" }, if node.require_approval { "asks first" } else { "no approval" });
            }
        }
        NodesAction::Allow { id, agents, approval } => {
            warden_server::nodes::set_node_access(&config_path, warden_bootstrap::NodeAccessConfig { id: id.clone(), enabled: true, agents, require_approval: approval })?;
            let store = PairingStore::new(devices_path()?);
            match store.status(&id)? {
                Some(warden_server::PairingStatus::Approved) => println!("node '{id}' allowed"),
                _ => println!("node '{id}' allowed — it also has to be approved in the device list: `warden-server devices approve {id}`"),
            }
        }
        NodesAction::Deny { id } => {
            let mut config = load_config_from_path(&config_path, config.is_some())?;
            let node = config.nodes.iter_mut().find(|n| n.id == id).ok_or_else(|| anyhow::anyhow!("no node '{id}' in the config"))?;
            node.enabled = false;
            save_config(&config_path, &config)?;
            println!("node '{id}' denied — agents can't use it any more");
        }
    }
    Ok(())
}

async fn run_tasks_command(args: TasksArgs) -> anyhow::Result<()> {
    let config_path = args.config.as_ref().map(PathBuf::from).or_else(warden_bootstrap::default_config_path).context("could not determine the OS config directory")?;
    let mut config = load_config_from_path(&config_path, args.config.is_some())?;
    let store = tasks_store()?;
    let position = |config: &warden_bootstrap::FileConfig, id: &str| {
        config.tasks.iter().position(|t| t.id == id).ok_or_else(|| anyhow::anyhow!("no task named '{id}' — `warden-server tasks list` shows them"))
    };
    match args.action {
        TasksCommand::List => {
            if config.tasks.is_empty() {
                println!("no scheduled tasks yet — add one with `warden-server tasks add <id> --prompt ... --every 1d`");
                return Ok(());
            }
            let states = store.states()?;
            let now = now_millis();
            for task in &config.tasks {
                let zone = Zone::parse(task.timezone.as_deref()).unwrap_or(Zone::Local);
                let status = task_status(task, states.get(&task.id), now);
                let on = if task.enabled { "on" } else { "paused" };
                let agent = task.agent.as_deref().unwrap_or("(no agent)");
                let last = match (status.last_run_at_ms, status.running, status.last_error.as_deref()) {
                    (None, _, _) => "never ran".to_string(),
                    (Some(at), true, _) => format!("running since {}", zone.format(at)),
                    (Some(at), false, None) => format!("last ran {}", zone.format(at)),
                    (Some(at), false, Some(err)) => format!("last ran {} and failed: {err}", zone.format(at)),
                };
                let next = match (&status.schedule_error, status.next_run_at_ms) {
                    (Some(err), _) => format!("invalid: {err}"),
                    (None, Some(at)) => format!("next {}", zone.format(at)),
                    (None, None) if !task.enabled => "not scheduled".to_string(),
                    (None, None) => "done".to_string(),
                };
                println!("{}\t{on}\t{}\t{agent}\t{last}\t{next}", task.id, task.schedule_label());
            }
        }
        TasksCommand::Add { id, prompt, agent, every, cron, once, timezone } => {
            anyhow::ensure!(!config.tasks.iter().any(|t| t.id == id), "there's already a task named '{id}'");
            let task = TaskConfig { id: id.clone(), agent, prompt, every, cron, once, timezone, enabled: true };
            config.tasks.push(task.clone());
            check_tasks(&config.tasks, &config.agents)?;
            save_config(&config_path, &config)?;
            let zone = Zone::parse(task.timezone.as_deref()).unwrap_or(Zone::Local);
            match next_run(&task, None, now_millis()) {
                Some(at) => println!("task '{id}' added — first run {} (on the hub started with `serve --run-tasks`)", zone.format(at)),
                None => println!("task '{id}' added"),
            }
        }
        TasksCommand::Pause { id } => {
            let i = position(&config, &id)?;
            config.tasks[i].enabled = false;
            save_config(&config_path, &config)?;
            println!("task '{id}' paused");
        }
        TasksCommand::Resume { id } => {
            let i = position(&config, &id)?;
            config.tasks[i].enabled = true;
            save_config(&config_path, &config)?;
            println!("task '{id}' is on again — it counts from now");
        }
        TasksCommand::Remove { id } => {
            let i = position(&config, &id)?;
            config.tasks.remove(i);
            save_config(&config_path, &config)?;
            println!("task '{id}' removed — its conversation stays on the hub");
        }
        TasksCommand::Run { id, vault_path } => {
            let task = config.tasks[position(&config, &id)?].clone();
            task.schedule()?;
            let overrides = Overrides { vault_path, ..Default::default() };
            let orchestrator = bootstrap(args.config.as_deref(), overrides, default_vault_path()).await?;
            let now = now_millis();
            store.mark_started(&task, now)?;
            let result = run_task(&orchestrator, &config, Some(&config_path), &task, &store.conversations_dir(), now).await;
            store.record_finish(&id, now_millis(), result.as_ref().err().map(|e| format!("{e:#}")))?;
            println!("{}", result?.content);
        }
    }
    Ok(())
}

fn api_keys_path() -> anyhow::Result<PathBuf> {
    warden_bootstrap::default_api_keys_path().context("could not determine the OS config directory for the API keys")
}

fn run_api_keys_command(action: ApiKeysAction) -> anyhow::Result<()> {
    let store = warden_server::api_keys::ApiKeyStore::new(api_keys_path()?);
    match action {
        ApiKeysAction::List => {
            let keys = store.list()?;
            if keys.is_empty() {
                println!("no API keys yet — create one with `warden-server api-keys create <name>`");
            }
            for key in keys {
                let used = key.last_used_at_ms.map_or("never used".to_string(), |ms| format!("last used {ms}"));
                let scope = key.agent_id.as_deref().map_or("any agent".to_string(), |agent| format!("only agent {agent}"));
                println!("{}\t{}\t{}…\t{scope}\tcreated {}\t{used}", key.id, key.name, key.shown, key.created_at_ms);
            }
        }
        ApiKeysAction::Create { name, agent, config } => {
            if agent.as_deref().is_some_and(|a| !a.trim().is_empty()) {
                let config_path = config.map(PathBuf::from).or_else(warden_bootstrap::default_config_path).context("could not determine the OS config directory")?;
                warden_server::api_key_admin::check_agent_exists(&config_path, agent.as_deref())?;
            }
            let created = store.create(&name, agent.as_deref())?;
            println!("{}", created.key);
            eprintln!("key '{}' created (id {}) — copy it now, it isn't shown again", created.info.name, created.info.id);
            eprintln!("use it as the bearer token with base URL http(s)://<this hub>:<port>/v1");
            match &created.info.agent_id {
                Some(agent) => eprintln!("it only speaks as agent '{agent}' (model \"warden\" or \"warden/{agent}\"; any other agent is refused)"),
                None => eprintln!("a general key: model \"warden\", or \"warden/<agent>\" to speak as a configured agent"),
            }
        }
        ApiKeysAction::Revoke { id } => {
            anyhow::ensure!(store.revoke(&id)?, "no API key with id '{id}'");
            println!("key '{id}' revoked");
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
        .with_sync(runner, Some(AUTO_SYNC_INTERVAL))
        .with_api(api_keys_path()?)
        // P92 — every device lists the tasks' conversations; only `--run-tasks` runs them.
        .with_tasks(tasks_store()?, args.run_tasks);
    let task_count = args
        .config
        .as_ref()
        .map(PathBuf::from)
        .or_else(warden_bootstrap::default_config_path)
        .and_then(|path| load_config_from_path(&path, false).ok())
        .map_or(0, |config| config.tasks.len());
    match (args.run_tasks, task_count) {
        (true, n) => eprintln!("warden-server: running scheduled tasks on this hub ({n} configured)"),
        (false, 0) => {}
        (false, n) => eprintln!("warden-server: {n} scheduled task(s) configured, but this hub doesn't run them (start it with --run-tasks)"),
    }
    if let Some(settings) = settings {
        server = server.with_settings(Arc::new(settings));
        // P84 — the members in `[[users]]` sign in with their password, each with their own vault.
        if let Some(users_dir) = warden_bootstrap::users::default_users_dir() {
            server = server.with_users_dir(users_dir);
        }
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
    eprintln!("warden-server: Warden API (OpenAI-compatible) at {}/v1 — keys: `warden-server api-keys create <name>`", page_url.trim_end_matches('/'));
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
        Command::ApiKeys { action } => run_api_keys_command(action),
        Command::Sync(args) => run_sync_command(args).await,
        Command::Tasks(args) => run_tasks_command(args).await,
        Command::Node(args) => run_node_command(args).await,
        Command::Nodes { action, config } => run_nodes_command(action, config),
        Command::Users { action, config } => run_users_command(action, config),
        Command::GenKey => {
            println!("{}", warden_bootstrap::generate_auth_key());
            Ok(())
        }
    }
}
