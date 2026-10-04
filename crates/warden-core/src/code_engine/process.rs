//! Starts and keeps the `opencode serve` of each project folder (`Launcher` for the real opencode).
//!
//! One server per working folder, started by the first task that needs it and ended after it has been unused for a
//! while. Each is started *in* the folder, on a port the system picks, listening on the loopback only, with a password
//! made for it and handed to the Warden alone (an environment variable, never a file in the person's repository). The
//! hub's own settings for the opencode (which models, through which endpoint) travel the same way, as
//! `OPENCODE_CONFIG_CONTENT`: nothing is written into the repository.
//!
//! A server that died is started again by the next task. A task running keeps its server alive however long it takes:
//! the `Endpoint` it was given holds a lease, and only a server nobody holds a lease on can be ended for being idle.

use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context};
use async_trait::async_trait;
use rand::RngCore;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

use super::opencode::{Endpoint, Launcher};

/// The opencode version the formats in this module were read from. A different minor version still works — it is
/// told to the person, not refused — because the parts used are the ones the opencode's own TUI uses.
pub const TESTED_VERSION: &str = "1.18.34";

/// How long a new server gets to say where it is listening.
const START_TIMEOUT: Duration = Duration::from_secs(30);

/// `Some(warning)` when `version` isn't the tested one's `major.minor`.
pub fn version_warning(version: &str) -> Option<String> {
    let minor = |v: &str| v.trim().split('.').take(2).map(str::to_string).collect::<Vec<_>>();
    (minor(version) != minor(TESTED_VERSION)).then(|| format!("the installed opencode is {}, and the Warden was tested with {TESTED_VERSION}; it should work, but if something breaks, that is the first suspect", version.trim()))
}

struct Running {
    /// Kept so the process lives as long as this entry does (`kill_on_drop`).
    child: Child,
    endpoint: Endpoint,
    last_used: Instant,
}

struct Inner {
    binary: String,
    /// The opencode's own settings as JSON (`OPENCODE_CONFIG_CONTENT`), if the hub has any for it.
    config: Option<String>,
    idle: Duration,
    servers: tokio::sync::Mutex<HashMap<String, Running>>,
}

pub struct OpencodeProcesses {
    inner: Arc<Inner>,
}

impl OpencodeProcesses {
    /// `binary` is the opencode's command (`opencode`, or a full path). `idle`: how long an unused server is kept.
    pub fn new(binary: impl Into<String>, config: Option<String>, idle: Duration) -> Self {
        let inner = Arc::new(Inner { binary: binary.into(), config, idle, servers: tokio::sync::Mutex::default() });
        tokio::spawn(reap(Arc::downgrade(&inner)));
        Self { inner }
    }

    /// The installed opencode's version, which is also the check that it is installed at all.
    pub async fn version(&self) -> anyhow::Result<String> {
        let output = Command::new(&self.inner.binary).arg("--version").stdin(Stdio::null()).output().await.map_err(not_installed)?;
        if !output.status.success() {
            bail!("`{} --version` failed", self.inner.binary);
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// Ends every server now.
    pub async fn shutdown(&self) {
        self.inner.servers.lock().await.clear();
    }
}

#[async_trait]
impl Launcher for OpencodeProcesses {
    async fn endpoint(&self, workdir: &str) -> anyhow::Result<Endpoint> {
        let mut servers = self.inner.servers.lock().await;
        if let Some(running) = servers.get_mut(workdir) {
            if running.child.try_wait()?.is_none() {
                running.last_used = Instant::now();
                return Ok(running.endpoint.clone());
            }
            servers.remove(workdir);
        }
        let running = start(&self.inner.binary, workdir, self.inner.config.as_deref()).await?;
        let endpoint = running.endpoint.clone();
        servers.insert(workdir.to_string(), running);
        Ok(endpoint)
    }
}

fn not_installed(err: std::io::Error) -> anyhow::Error {
    if err.kind() == std::io::ErrorKind::NotFound {
        anyhow!("the opencode isn't installed on the machine running the hub (npm install -g opencode-ai)")
    } else {
        anyhow!("couldn't run the opencode: {err}")
    }
}

async fn start(binary: &str, workdir: &str, config: Option<&str>) -> anyhow::Result<Running> {
    if !Path::new(workdir).is_dir() {
        bail!("the project's working folder {workdir} doesn't exist on the machine running the hub");
    }
    let mut password = [0u8; 24];
    rand::thread_rng().fill_bytes(&mut password);
    let password: String = password.iter().map(|b| format!("{b:02x}")).collect();

    let mut command = Command::new(binary);
    command
        .args(["serve", "--hostname", "127.0.0.1", "--port", "0"])
        .current_dir(workdir)
        .env("OPENCODE_SERVER_PASSWORD", &password)
        // An update in the middle of a task, behind the Warden's back, is not what it was tested with.
        .env("OPENCODE_DISABLE_AUTOUPDATE", "true")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(config) = config {
        command.env("OPENCODE_CONFIG_CONTENT", config);
    }
    let mut child = command.spawn().map_err(not_installed)?;

    // Both streams are read for as long as the process lives: a pipe nobody empties ends up blocking it.
    let (lines, mut heard) = tokio::sync::mpsc::unbounded_channel::<String>();
    if let Some(out) = child.stdout.take() {
        forward(out, lines.clone());
    }
    if let Some(err) = child.stderr.take() {
        forward(err, lines.clone());
    }
    drop(lines);

    let mut recent: Vec<String> = Vec::new();
    let listening = tokio::time::timeout(START_TIMEOUT, async {
        while let Some(line) = heard.recv().await {
            if let Some(url) = listening_url(&line) {
                return Some(url);
            }
            recent.push(line);
            if recent.len() > 5 {
                recent.remove(0);
            }
        }
        None
    })
    .await;
    match listening {
        Ok(Some(base_url)) => Ok(Running { child, endpoint: Endpoint { base_url, password: Some(password), lease: Some(Arc::new(())) }, last_used: Instant::now() }),
        Ok(None) => Err(anyhow!("the opencode stopped before it was ready: {}", recent.join(" | "))).context("starting the opencode"),
        Err(_) => Err(anyhow!("the opencode didn't say it was ready in {} seconds", START_TIMEOUT.as_secs())).context("starting the opencode"),
    }
}

fn forward(stream: impl tokio::io::AsyncRead + Unpin + Send + 'static, lines: tokio::sync::mpsc::UnboundedSender<String>) {
    tokio::spawn(async move {
        let mut reader = BufReader::new(stream).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            let _ = lines.send(line);
        }
    });
}

/// `opencode server listening on http://127.0.0.1:4199` → the address.
fn listening_url(line: &str) -> Option<String> {
    let rest = line.split("listening on ").nth(1)?;
    let url = rest.split_whitespace().next()?;
    url.starts_with("http://").then(|| url.trim_end_matches('/').to_string())
}

/// Ends the servers nobody has held for `idle`. Stops by itself when the manager is gone.
async fn reap(inner: Weak<Inner>) {
    loop {
        tokio::time::sleep(Duration::from_secs(60)).await;
        let Some(inner) = inner.upgrade() else { return };
        reap_now(&inner).await;
    }
}

async fn reap_now(inner: &Inner) {
    let mut servers = inner.servers.lock().await;
    servers.retain(|_, running| {
        let held = running.endpoint.lease.as_ref().is_some_and(|lease| Arc::strong_count(lease) > 1);
        let dead = running.child.try_wait().map(|status| status.is_some()).unwrap_or(true);
        !dead && (held || running.last_used.elapsed() < inner.idle)
    });
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn dir(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("warden-opencode-{name}-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    /// A stand-in for the opencode: says where it listens (the port is its own pid, so two starts differ), then
    /// stays up; with `serve` it also writes what it was given to `seen` next to the script.
    fn fake_binary(dir: &Path, body: &str) -> String {
        let path = dir.join("opencode");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path.to_string_lossy().into_owned()
    }

    const LISTENS: &str = r#"
if [ "$1" = "--version" ]; then echo "1.18.34"; exit 0; fi
echo "$PWD|$OPENCODE_SERVER_PASSWORD|$OPENCODE_CONFIG_CONTENT|$@" > "$(dirname "$0")/seen"
echo "INFO loading something" >&2
echo "opencode server listening on http://127.0.0.1:$$"
exec sleep 300
"#;

    #[test]
    fn the_address_is_read_from_the_line_and_a_different_minor_version_is_told() {
        assert_eq!(listening_url("opencode server listening on http://127.0.0.1:4199").as_deref(), Some("http://127.0.0.1:4199"));
        assert_eq!(listening_url("timestamp=1 level=INFO message=loading path=/x"), None);
        assert_eq!(version_warning("1.18.99"), None);
        assert!(version_warning("1.19.0").unwrap().contains("1.19.0"));
        assert!(version_warning("2.0.1").is_some());
    }

    #[tokio::test]
    async fn a_server_starts_in_the_folder_with_its_own_password_and_the_hubs_config_and_is_reused() {
        let home = dir("start");
        let repo = dir("repo");
        let processes = OpencodeProcesses::new(fake_binary(&home, LISTENS), Some(r#"{"model":"x"}"#.into()), Duration::from_secs(600));
        let first = processes.endpoint(repo.to_str().unwrap()).await.unwrap();
        let second = processes.endpoint(repo.to_str().unwrap()).await.unwrap();
        assert_eq!(first.base_url, second.base_url, "one server per folder");
        assert!(first.base_url.starts_with("http://127.0.0.1:"));

        let seen = std::fs::read_to_string(home.join("seen")).unwrap();
        let fields: Vec<&str> = seen.trim().splitn(4, '|').collect();
        assert_eq!(std::fs::canonicalize(fields[0]).unwrap(), std::fs::canonicalize(&repo).unwrap(), "started in the project's folder");
        assert_eq!(Some(fields[1]), first.password.as_deref());
        assert_eq!(fields[1].len(), 48, "a password nobody could guess");
        assert_eq!(fields[2], r#"{"model":"x"}"#);
        assert_eq!(fields[3], "serve --hostname 127.0.0.1 --port 0", "loopback only, a port the system picks");
        assert!(!repo.read_dir().unwrap().any(|_| true), "nothing was written into the repository");

        let other = dir("other");
        assert_ne!(processes.endpoint(other.to_str().unwrap()).await.unwrap().base_url, first.base_url, "another folder, another server");
    }

    #[tokio::test]
    async fn a_server_that_died_is_started_again() {
        let home = dir("died");
        let repo = dir("died-repo");
        let processes = OpencodeProcesses::new(fake_binary(&home, LISTENS), None, Duration::from_secs(600));
        let first = processes.endpoint(repo.to_str().unwrap()).await.unwrap();
        let pid = first.base_url.rsplit(':').next().unwrap().to_string();
        std::process::Command::new("kill").args(["-9", &pid]).status().unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        let again = processes.endpoint(repo.to_str().unwrap()).await.unwrap();
        assert_ne!(again.base_url, first.base_url);
    }

    #[tokio::test]
    async fn a_missing_folder_a_missing_binary_and_a_server_that_never_comes_up_are_told_plainly() {
        let home = dir("errors");
        let processes = OpencodeProcesses::new(fake_binary(&home, LISTENS), None, Duration::from_secs(600));
        let err = processes.endpoint("/nonexistent/warden-test").await.unwrap_err().to_string();
        assert!(err.contains("doesn't exist"), "{err}");

        let nothing = OpencodeProcesses::new("/nonexistent/opencode", None, Duration::from_secs(600));
        let repo = dir("errors-repo");
        assert!(nothing.endpoint(repo.to_str().unwrap()).await.unwrap_err().to_string().contains("isn't installed"));
        assert!(nothing.version().await.unwrap_err().to_string().contains("isn't installed"));

        let quits = OpencodeProcesses::new(fake_binary(&dir("quits"), "echo 'boom: no config' >&2; exit 1"), None, Duration::from_secs(600));
        let err = format!("{:#}", quits.endpoint(repo.to_str().unwrap()).await.unwrap_err());
        assert!(err.contains("stopped before it was ready") && err.contains("boom: no config"), "{err}");
        assert_eq!(processes.version().await.unwrap(), "1.18.34");
    }

    #[tokio::test]
    async fn an_idle_server_is_ended_but_one_a_task_holds_is_not() {
        let home = dir("reap");
        let repo = dir("reap-repo");
        let processes = OpencodeProcesses::new(fake_binary(&home, LISTENS), None, Duration::ZERO);
        let held = processes.endpoint(repo.to_str().unwrap()).await.unwrap();
        reap_now(&processes.inner).await;
        assert_eq!(processes.inner.servers.lock().await.len(), 1, "a task is using it, however long it takes");
        drop(held);
        reap_now(&processes.inner).await;
        assert!(processes.inner.servers.lock().await.is_empty(), "nobody is using it any more");
    }

    /// Against the real opencode (`cargo test -p warden-core real_opencode -- --ignored`): it starts, takes the
    /// password, opens a session in the folder with the "ask first" rules, and its event stream says it is connected.
    /// No model is needed, so no task is sent.
    #[tokio::test]
    #[ignore = "needs the opencode installed"]
    async fn real_opencode_starts_and_opens_a_session() {
        let repo = dir("real");
        let processes = OpencodeProcesses::new("opencode", None, Duration::from_secs(60));
        eprintln!("opencode {}", processes.version().await.unwrap());
        let endpoint = processes.endpoint(repo.to_str().unwrap()).await.unwrap();
        let client = super::super::opencode::Client::new(endpoint.clone(), repo.to_str().unwrap());
        let session = client.create_session("Real").await.unwrap();
        assert!(session.starts_with("ses_"), "{session}");
        let _events = client.events().await.unwrap();
        let without_password = reqwest::get(format!("{}/session", endpoint.base_url)).await.unwrap().status();
        assert_eq!(without_password, 401, "the server wants its password");
        processes.shutdown().await;
    }
}
