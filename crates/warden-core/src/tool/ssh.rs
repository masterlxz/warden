use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::tool::shell::{truncate, DEFAULT_TIMEOUT_MS, MAX_TIMEOUT_MS};
use crate::tool::{ApprovalRequest, Approver, Tool, ToolSpec};

/// How long `ssh` itself waits to establish the connection, separate from the whole-command
/// timeout — a dead host should fail fast, not eat the command's entire time budget.
const CONNECT_TIMEOUT_SECS: u32 = 10;

/// Largest file `ssh_upload`/`ssh_download` will move. The whole thing streams through the `ssh`
/// child's stdin/stdout, so this isn't about memory — it's a guard against a model filling the
/// disk (download) or a remote host's disk (upload) by mistake.
const MAX_TRANSFER_BYTES: u64 = 100 * 1024 * 1024;
const DEFAULT_TRANSFER_TIMEOUT_MS: u64 = 120_000;
const MAX_TRANSFER_TIMEOUT_MS: u64 = 600_000;

/// How long an approval prompt may sit unanswered before it counts as a "no".
const APPROVAL_TIMEOUT: Duration = Duration::from_secs(120);

/// One registered SSH server (P47). Mirrors `warden_bootstrap::SshHostConfig` minus the config-only
/// `enabled` flag — the bootstrap layer only hands over hosts that are enabled, so this crate never
/// has to know about that switch. The private key is only ever a *path* (`identity_file`): Warden
/// never reads or stores key material.
#[derive(Debug, Clone, PartialEq)]
pub struct SshHost {
    /// Unique, user-chosen. The only thing the model ever passes to `ssh_exec` to pick a server.
    pub id: String,
    pub host: String,
    pub user: String,
    pub port: u16,
    pub identity_file: Option<String>,
    /// Agent ids allowed to use this host. Empty means every agent — and channels with no agent
    /// at all (Telegram/WhatsApp/mobile/MCP server), the same trust the `shell` tool has.
    pub agents: Vec<String>,
    /// When set, every `ssh_exec`/`ssh_upload`/`ssh_download` on this host waits for a human "yes"
    /// (see `Approver`). A channel that can't ask refuses instead of running unattended.
    pub require_approval: bool,
}

impl SshHost {
    /// Rejects anything that could turn into an `ssh` option or a different destination. `host`
    /// and `user` come from a config file the user edits, but the same check runs at call time
    /// too, so a hand-edited `config.toml` can't smuggle in `-oProxyCommand=...` or an
    /// `ssh://user@host:port` URI (which `ssh` accepts in the host position and which would
    /// override `-l`/`-p`).
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.id.trim().is_empty() {
            anyhow::bail!("ssh host id must not be empty");
        }
        if self.port == 0 {
            anyhow::bail!("ssh host '{}': port must be between 1 and 65535", self.id);
        }
        check_token(&self.id, "host", &self.host, |c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':' | '%'))?;
        check_token(&self.id, "user", &self.user, |c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))?;
        if let Some(path) = &self.identity_file {
            if path.trim().is_empty() || path.chars().any(char::is_control) {
                anyhow::bail!("ssh host '{}': identity file path is empty or has control characters", self.id);
            }
        }
        Ok(())
    }

    fn visible_to(&self, agent: Option<&str>) -> bool {
        self.agents.is_empty() || agent.is_some_and(|a| self.agents.iter().any(|allowed| allowed == a))
    }
}

fn check_token(id: &str, what: &str, value: &str, allowed: impl Fn(char) -> bool) -> anyhow::Result<()> {
    if value.is_empty() {
        anyhow::bail!("ssh host '{id}': {what} must not be empty");
    }
    if value.starts_with('-') {
        anyhow::bail!("ssh host '{id}': {what} must not start with '-'");
    }
    if let Some(bad) = value.chars().find(|c| !allowed(*c)) {
        anyhow::bail!("ssh host '{id}': {what} contains an unsupported character ({bad:?})");
    }
    Ok(())
}

/// The full `ssh` argument list for running `command` on `host`. Non-interactive (`BatchMode`, so
/// a password/passphrase prompt fails instead of hanging the process) and strict about host keys
/// (an unknown key is an error, never silently trusted — see the hint in `run_on_host`).
pub fn ssh_args(host: &SshHost, command: &str) -> Vec<String> {
    let mut args = vec![
        "-o".to_string(),
        "BatchMode=yes".to_string(),
        "-o".to_string(),
        "StrictHostKeyChecking=yes".to_string(),
        "-o".to_string(),
        format!("ConnectTimeout={CONNECT_TIMEOUT_SECS}"),
    ];
    if let Some(identity) = &host.identity_file {
        args.extend(["-o".to_string(), "IdentitiesOnly=yes".to_string(), "-i".to_string(), identity.clone()]);
    }
    args.extend(["-p".to_string(), host.port.to_string(), "-l".to_string(), host.user.clone(), "--".to_string()]);
    args.push(host.host.clone());
    args.push(command.to_string());
    args
}

/// Runs `command` on `host` and returns `{exit_code, stdout, stderr, timed_out}` (plus a `hint`
/// when the failure is one the user can fix). Shared by the `ssh_exec` tool and the "test
/// connection" buttons/commands in the desktop and CLI, so all of them speak to the server
/// exactly the same way.
pub async fn run_on_host(program: &str, host: &SshHost, command: &str, timeout_ms: u64) -> anyhow::Result<Value> {
    host.validate()?;
    check_command(command)?;

    let mut cmd = tokio::process::Command::new(program);
    cmd.args(ssh_args(host, command)).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    let child = cmd.spawn().map_err(|e| spawn_error(program, e))?;

    let timeout_ms = timeout_ms.min(MAX_TIMEOUT_MS);
    match tokio::time::timeout(Duration::from_millis(timeout_ms), child.wait_with_output()).await {
        Ok(result) => {
            let output = result.map_err(|e| anyhow::anyhow!("failed to run ssh: {e}"))?;
            let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
            let mut result = json!({
                "host_id": host.id,
                "exit_code": output.status.code(),
                "stdout": truncate(&String::from_utf8_lossy(&output.stdout)),
                "stderr": truncate(&stderr),
                "timed_out": false,
            });
            if let Some(hint) = failure_hint(host, &stderr) {
                result["hint"] = json!(hint);
            }
            Ok(result)
        }
        Err(_) => Ok(json!({
            "host_id": host.id,
            "exit_code": null,
            "stdout": "",
            "stderr": "",
            "timed_out": true,
        })),
    }
}

/// Verdict of `test_connection`, ready to show a user: did it work, and if not, the most
/// actionable explanation available.
#[derive(Debug, Clone, PartialEq)]
pub struct ConnectionTest {
    pub ok: bool,
    pub message: String,
}

/// Tries `host` exactly as `ssh_exec` would (same flags, same strict host-key check) with a
/// harmless `echo`. Backs the "Test connection" button in the desktop and `/ssh test` in the CLI.
pub async fn test_connection(host: &SshHost) -> anyhow::Result<ConnectionTest> {
    let result = run_on_host("ssh", host, "echo warden-ssh-ok", 20_000).await?;
    Ok(summarize_test(&result))
}

fn summarize_test(result: &Value) -> ConnectionTest {
    if result["exit_code"] == json!(0) {
        return ConnectionTest { ok: true, message: "Connected — the server accepted the key and ran a test command.".to_string() };
    }
    if result["timed_out"] == json!(true) {
        return ConnectionTest { ok: false, message: "Timed out waiting for the server.".to_string() };
    }
    let message = result["hint"]
        .as_str()
        .map(str::to_string)
        .or_else(|| Some(result["stderr"].as_str()?.trim().to_string()).filter(|s| !s.is_empty()))
        .unwrap_or_else(|| "The connection failed with no error message.".to_string());
    ConnectionTest { ok: false, message }
}

/// `ssh` exits 255 for its own failures, but the useful part is *why* — turn the two causes the
/// user can actually fix into an instruction instead of a bare stderr line.
fn failure_hint(host: &SshHost, stderr: &str) -> Option<String> {
    if stderr.contains("Host key verification failed") || stderr.contains("REMOTE HOST IDENTIFICATION HAS CHANGED") {
        return Some(format!(
            "The server's host key isn't trusted yet (or changed). Warden never trusts a key on its own: run \
             `ssh -p {} {}@{}` once in a terminal, check the fingerprint, and accept it.",
            host.port, host.user, host.host
        ));
    }
    if stderr.contains("Permission denied") {
        return Some("The server rejected the key. Check the user name and that the key is authorized there (a \
                     passphrase-protected key must be loaded in ssh-agent — Warden can't type a passphrase)."
            .to_string());
    }
    None
}

/// Only the parts of an `ssh` invocation that can go wrong the same way for every operation:
/// the command must not be read as an option, and must not be empty.
fn check_command(command: &str) -> anyhow::Result<()> {
    // Defense in depth. Checked with `ssh -G` on OpenSSH 10.5: without `--` an option after the
    // hostname is applied (`-oProxyCommand=...` would run locally), and with `--` it isn't. This
    // build is the only one tested, so don't rely on `--` alone: other OpenSSH versions, Windows'
    // port or a wrapper on PATH may parse differently.
    if command.starts_with('-') {
        anyhow::bail!("ssh command must not start with '-' (it would be read as an ssh option)");
    }
    if command.trim().is_empty() {
        anyhow::bail!("missing required 'command' argument");
    }
    Ok(())
}

fn spawn_error(program: &str, err: std::io::Error) -> anyhow::Error {
    anyhow::anyhow!("failed to run '{program}' — is the OpenSSH client installed and on PATH? ({err})")
}

/// Wraps `s` in single quotes for a POSIX shell, so a path with spaces, `;`, `$(...)` or a quote
/// in it reaches the remote command as one literal word.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// The remote side of `ssh_upload`: copy stdin into `remote`. `set -C` (noclobber) makes the write
/// fail when the file exists, so the model has to opt in to an overwrite. `sh -c` pins the syntax
/// to POSIX no matter what the remote user's login shell is (fish and csh don't read `set -C`).
fn remote_write_command(remote: &str, overwrite: bool) -> String {
    let inner = format!("{}cat > {}", if overwrite { "" } else { "set -C; " }, shell_quote(remote));
    format!("sh -c {}", shell_quote(&inner))
}

fn remote_read_command(remote: &str) -> String {
    format!("sh -c {}", shell_quote(&format!("cat -- {}", shell_quote(remote))))
}

fn check_remote_path(remote: &str) -> anyhow::Result<()> {
    if remote.trim().is_empty() {
        anyhow::bail!("missing required 'remote_path' argument");
    }
    if remote.len() > 4096 || remote.chars().any(char::is_control) {
        anyhow::bail!("remote_path is too long or has control characters");
    }
    Ok(())
}

/// Size of the local file to upload, refusing anything that isn't a regular file or is over the cap.
fn upload_preflight(local: &Path, max_bytes: u64) -> anyhow::Result<u64> {
    let meta = std::fs::metadata(local).map_err(|e| anyhow::anyhow!("cannot read '{}': {e}", local.display()))?;
    if !meta.is_file() {
        anyhow::bail!("'{}' is not a regular file", local.display());
    }
    if meta.len() > max_bytes {
        anyhow::bail!("'{}' is {} bytes, over the {max_bytes}-byte transfer limit", local.display(), meta.len());
    }
    Ok(meta.len())
}

fn download_preflight(dest: &Path, overwrite: bool) -> anyhow::Result<()> {
    if dest.is_dir() {
        anyhow::bail!("'{}' is a directory — give a file path", dest.display());
    }
    if dest.exists() && !overwrite {
        anyhow::bail!("'{}' already exists (pass overwrite=true to replace it)", dest.display());
    }
    Ok(())
}

fn part_path(dest: &Path) -> PathBuf {
    let mut os = dest.as_os_str().to_owned();
    os.push(".part");
    PathBuf::from(os)
}

/// Streams `local` to `remote` over `ssh` (the file is the child's stdin). Returns the same shape
/// as `run_on_host` plus `bytes`.
async fn upload_to_host(
    program: &str,
    host: &SshHost,
    local: &Path,
    remote: &str,
    overwrite: bool,
    size: u64,
    timeout_ms: u64,
) -> anyhow::Result<Value> {
    host.validate()?;
    let file = std::fs::File::open(local).map_err(|e| anyhow::anyhow!("cannot read '{}': {e}", local.display()))?;
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(ssh_args(host, &remote_write_command(remote, overwrite)))
        .stdin(Stdio::from(file))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let child = cmd.spawn().map_err(|e| spawn_error(program, e))?;

    let timeout_ms = timeout_ms.min(MAX_TRANSFER_TIMEOUT_MS);
    match tokio::time::timeout(Duration::from_millis(timeout_ms), child.wait_with_output()).await {
        Ok(result) => {
            let output = result.map_err(|e| anyhow::anyhow!("failed to run ssh: {e}"))?;
            let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
            let ok = output.status.success();
            let mut result = json!({
                "host_id": host.id,
                "local_path": local.display().to_string(),
                "remote_path": remote,
                "exit_code": output.status.code(),
                "bytes": if ok { size } else { 0 },
                "stderr": truncate(&stderr),
                "timed_out": false,
            });
            let lowered = stderr.to_lowercase();
            if !ok && !overwrite && (lowered.contains("exist") || lowered.contains("overwrite") || lowered.contains("clobber")) {
                result["hint"] = json!("The remote file already exists. Pass overwrite=true to replace it.");
            } else if let Some(hint) = failure_hint(host, &stderr) {
                result["hint"] = json!(hint);
            }
            Ok(result)
        }
        Err(_) => Ok(json!({
            "host_id": host.id,
            "local_path": local.display().to_string(),
            "remote_path": remote,
            "exit_code": null,
            "bytes": 0,
            "stderr": "",
            "timed_out": true,
        })),
    }
}

/// Streams `remote` into `dest` (the child's stdout). Written to `<dest>.part` and renamed only on
/// success, so a dropped connection or a too-big file never leaves a truncated `dest` behind.
async fn download_from_host(
    program: &str,
    host: &SshHost,
    remote: &str,
    dest: &Path,
    max_bytes: u64,
    timeout_ms: u64,
) -> anyhow::Result<Value> {
    host.validate()?;
    if let Some(parent) = dest.parent().filter(|p| !p.as_os_str().is_empty()) {
        tokio::fs::create_dir_all(parent).await.map_err(|e| anyhow::anyhow!("cannot create '{}': {e}", parent.display()))?;
    }
    let part = part_path(dest);

    let mut cmd = tokio::process::Command::new(program);
    cmd.args(ssh_args(host, &remote_read_command(remote)))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd.spawn().map_err(|e| spawn_error(program, e))?;
    let mut stdout = child.stdout.take().expect("stdout is piped");
    let mut stderr = child.stderr.take().expect("stderr is piped");
    // Read concurrently: a chatty stderr must never block the child while we're draining stdout.
    let stderr_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        let _ = (&mut stderr).take(64 * 1024).read_to_end(&mut buf).await;
        let _ = tokio::io::copy(&mut stderr, &mut tokio::io::sink()).await;
        buf
    });

    let run = async {
        let copied: anyhow::Result<u64> = async {
            let mut file = tokio::fs::File::create(&part).await?;
            let mut buf = vec![0u8; 64 * 1024];
            let mut total = 0u64;
            loop {
                let n = stdout.read(&mut buf).await?;
                if n == 0 {
                    break;
                }
                total += n as u64;
                if total > max_bytes {
                    anyhow::bail!("the remote file is over the {max_bytes}-byte transfer limit");
                }
                file.write_all(&buf[..n]).await?;
            }
            file.flush().await?;
            Ok(total)
        }
        .await;
        if copied.is_err() {
            let _ = child.start_kill();
        }
        let status = child.wait().await?;
        anyhow::Ok((copied, status))
    };

    let timeout_ms = timeout_ms.min(MAX_TRANSFER_TIMEOUT_MS);
    let result = tokio::time::timeout(Duration::from_millis(timeout_ms), run).await;
    let describe = |exit_code: Value, bytes: u64, stderr: &str, timed_out: bool| {
        let mut result = json!({
            "host_id": host.id,
            "remote_path": remote,
            "local_path": dest.display().to_string(),
            "exit_code": exit_code,
            "bytes": bytes,
            "stderr": truncate(stderr),
            "timed_out": timed_out,
        });
        if let Some(hint) = failure_hint(host, stderr) {
            result["hint"] = json!(hint);
        }
        result
    };
    match result {
        Err(_) => {
            stderr_task.abort();
            let _ = tokio::fs::remove_file(&part).await;
            Ok(describe(Value::Null, 0, "", true))
        }
        Ok(Err(err)) => {
            stderr_task.abort();
            let _ = tokio::fs::remove_file(&part).await;
            Err(err)
        }
        Ok(Ok((copied, status))) => {
            let stderr = String::from_utf8_lossy(&stderr_task.await.unwrap_or_default()).into_owned();
            match copied {
                Err(err) => {
                    let _ = tokio::fs::remove_file(&part).await;
                    Err(err)
                }
                Ok(bytes) if status.success() => {
                    tokio::fs::rename(&part, dest).await.map_err(|e| anyhow::anyhow!("cannot write '{}': {e}", dest.display()))?;
                    Ok(describe(json!(0), bytes, &stderr, false))
                }
                Ok(_) => {
                    let _ = tokio::fs::remove_file(&part).await;
                    Ok(describe(json!(status.code()), 0, &stderr, false))
                }
            }
        }
    }
}

/// Whether a human said yes, for the audit log.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Approval {
    NotRequired,
    Approved,
}

impl Approval {
    fn label(self) -> &'static str {
        match self {
            Approval::NotRequired => "not_required",
            Approval::Approved => "approved",
        }
    }
}

/// Append-only JSONL record of every SSH action the model attempted, one line per call, including
/// the ones a human refused. Holds the command/paths and the outcome (exit code, byte count,
/// error) but never stdout/stderr, which is where a secret would most likely turn up. The command
/// itself can still contain one, so the file is created `0600`. No rotation yet (P47).
pub struct AuditLog {
    path: PathBuf,
    write_lock: Mutex<()>,
}

impl AuditLog {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into(), write_lock: Mutex::new(()) }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn record(&self, entry: &Value) -> std::io::Result<()> {
        use std::io::Write as _;
        let _guard = self.write_lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(dir) = self.path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
        }
        let mut options = std::fs::OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&self.path)?;
        let mut line = serde_json::to_string(entry).map_err(std::io::Error::other)?;
        line.push('\n');
        file.write_all(line.as_bytes())
    }
}

/// What the three tools share: the host list and who's asking, plus the optional audit log and
/// approver. Cloned (cheaply) whenever a tool is re-scoped to an agent or handed an approver, which
/// is why re-scoping never loses hosts.
#[derive(Clone)]
struct SshContext {
    hosts: Arc<Vec<SshHost>>,
    agent: Option<String>,
    program: String,
    /// Relative local paths resolve against this (the vault root), the same as `shell`'s `cwd`.
    base_dir: PathBuf,
    audit: Option<Arc<AuditLog>>,
    approver: Option<Arc<dyn Approver>>,
    approval_timeout: Duration,
    max_transfer_bytes: u64,
}

impl SshContext {
    fn new(hosts: Vec<SshHost>) -> Self {
        Self {
            hosts: Arc::new(hosts),
            agent: None,
            program: "ssh".to_string(),
            base_dir: PathBuf::from("."),
            audit: None,
            approver: None,
            approval_timeout: APPROVAL_TIMEOUT,
            max_transfer_bytes: MAX_TRANSFER_BYTES,
        }
    }

    fn visible(&self) -> Vec<&SshHost> {
        self.hosts.iter().filter(|h| h.visible_to(self.agent.as_deref())).collect()
    }

    fn find(&self, host_id: &str) -> anyhow::Result<&SshHost> {
        self.hosts.iter().find(|h| h.id == host_id && h.visible_to(self.agent.as_deref())).ok_or_else(|| {
            let ids: Vec<&str> = self.visible().iter().map(|h| h.id.as_str()).collect();
            anyhow::anyhow!("unknown or unavailable ssh host '{host_id}' (available: {})", ids.join(", "))
        })
    }

    fn listing(&self) -> String {
        self.visible().iter().map(|h| format!("- {}: {}@{}", h.id, h.user, h.host)).collect::<Vec<_>>().join("\n")
    }

    fn host_id_schema(&self) -> Value {
        let ids: Vec<&str> = self.visible().iter().map(|h| h.id.as_str()).collect();
        json!({ "type": "string", "enum": ids, "description": "Which registered server to use." })
    }

    fn resolve(&self, path: &str) -> PathBuf {
        let path = Path::new(path);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.base_dir.join(path)
        }
    }

    /// Runs the approval step for `host`. `Err` means the action must not run — either nobody could
    /// be asked or the answer was no — and the refusal is already in the audit log.
    async fn authorize(&self, host: &SshHost, action: &str, detail: &str, target: &Value) -> anyhow::Result<Approval> {
        if !host.require_approval {
            return Ok(Approval::NotRequired);
        }
        let Some(approver) = &self.approver else {
            self.record(host, action, target, "unavailable", json!({ "error": "refused: no way to ask for approval" }));
            anyhow::bail!(
                "host '{}' requires approval for every action, and this channel can't ask for it \
                 (use the desktop app or the interactive CLI)",
                host.id
            );
        };
        let request = ApprovalRequest { host_id: host.id.clone(), action: action.to_string(), detail: detail.to_string() };
        let approved = tokio::time::timeout(self.approval_timeout, approver.approve(request)).await.unwrap_or(false);
        if !approved {
            self.record(host, action, target, "denied", json!({ "error": "refused: not approved" }));
            anyhow::bail!("the user did not approve this action on host '{}'", host.id);
        }
        Ok(Approval::Approved)
    }

    fn record(&self, host: &SshHost, action: &str, target: &Value, approval: &str, outcome: Value) {
        let Some(audit) = &self.audit else { return };
        let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
        let mut entry = json!({
            "ts": ts,
            "host_id": host.id,
            "agent": self.agent,
            "action": action,
            "approval": approval,
            "outcome": outcome,
        });
        if let (Some(entry), Some(target)) = (entry.as_object_mut(), target.as_object()) {
            entry.extend(target.clone());
        }
        if let Err(err) = audit.record(&entry) {
            eprintln!("note: could not write the ssh audit log ({}): {err}", audit.path().display());
        }
    }

    /// Records how a call ended, whatever the shape of the result.
    fn finish(&self, host: &SshHost, action: &str, target: &Value, approval: Approval, result: &anyhow::Result<Value>) {
        let outcome = match result {
            Ok(value) => {
                // Only the fields this kind of call has: an exec has no `bytes`.
                let picked = ["exit_code", "timed_out", "bytes"].iter().filter(|k| !value[**k].is_null() || **k == "exit_code");
                Value::Object(picked.map(|k| (k.to_string(), value[*k].clone())).collect())
            }
            Err(err) => json!({ "error": format!("{err:#}") }),
        };
        self.record(host, action, target, approval.label(), outcome);
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum SshOp {
    Exec,
    Upload,
    Download,
}

/// `ssh_exec`, `ssh_upload` and `ssh_download` — one type, three faces, so they share the host
/// list, agent scoping, approval and audit log. The model picks a server by `host_id` only — never
/// a hostname or user — and only among the ones the current agent may use. No sandbox and no
/// command allowlist, deliberately: inside a host the user enabled, this is the same trust as
/// `shell`; the per-host `require_approval` is the brake for whoever wants one.
pub struct SshTool {
    ctx: SshContext,
    op: SshOp,
}

impl SshTool {
    pub fn exec(hosts: Vec<SshHost>) -> Self {
        Self { ctx: SshContext::new(hosts), op: SshOp::Exec }
    }

    pub fn upload(hosts: Vec<SshHost>) -> Self {
        Self { ctx: SshContext::new(hosts), op: SshOp::Upload }
    }

    pub fn download(hosts: Vec<SshHost>) -> Self {
        Self { ctx: SshContext::new(hosts), op: SshOp::Download }
    }

    pub fn for_agent(mut self, agent: Option<String>) -> Self {
        self.ctx.agent = agent;
        self
    }

    /// Points at a different `ssh` executable — only for tests, which use a stand-in script.
    pub fn with_program(mut self, program: impl Into<String>) -> Self {
        self.ctx.program = program.into();
        self
    }

    /// Where a relative local path in `ssh_upload`/`ssh_download` starts (the vault root).
    pub fn with_base_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.ctx.base_dir = dir.into();
        self
    }

    pub fn with_audit_log(mut self, audit: Arc<AuditLog>) -> Self {
        self.ctx.audit = Some(audit);
        self
    }

    pub fn with_max_transfer_bytes(mut self, max: u64) -> Self {
        self.ctx.max_transfer_bytes = max;
        self
    }

    pub fn with_approval_timeout(mut self, timeout: Duration) -> Self {
        self.ctx.approval_timeout = timeout;
        self
    }

    async fn call_exec(&self, args: &Value) -> anyhow::Result<Value> {
        let host_id = str_arg(args, "host_id")?;
        let command = str_arg(args, "command")?;
        let timeout_ms = args.get("timeout_ms").and_then(Value::as_u64).unwrap_or(DEFAULT_TIMEOUT_MS);
        let host = self.ctx.find(host_id)?;
        check_command(command)?;

        let target = json!({ "command": command });
        let approval = self.ctx.authorize(host, "exec", command, &target).await?;
        let result = run_on_host(&self.ctx.program, host, command, timeout_ms).await;
        self.ctx.finish(host, "exec", &target, approval, &result);
        result
    }

    async fn call_upload(&self, args: &Value) -> anyhow::Result<Value> {
        let host_id = str_arg(args, "host_id")?;
        let local_path = str_arg(args, "local_path")?;
        let remote_path = str_arg(args, "remote_path")?;
        let overwrite = args.get("overwrite").and_then(Value::as_bool).unwrap_or(false);
        let timeout_ms = args.get("timeout_ms").and_then(Value::as_u64).unwrap_or(DEFAULT_TRANSFER_TIMEOUT_MS);
        let host = self.ctx.find(host_id)?;
        check_remote_path(remote_path)?;
        let local = self.ctx.resolve(local_path);
        let size = upload_preflight(&local, self.ctx.max_transfer_bytes)?;

        let target = json!({ "local_path": local.display().to_string(), "remote_path": remote_path, "overwrite": overwrite });
        let detail = format!("{} → {remote_path}", local.display());
        let approval = self.ctx.authorize(host, "upload", &detail, &target).await?;
        let result = upload_to_host(&self.ctx.program, host, &local, remote_path, overwrite, size, timeout_ms).await;
        self.ctx.finish(host, "upload", &target, approval, &result);
        result
    }

    async fn call_download(&self, args: &Value) -> anyhow::Result<Value> {
        let host_id = str_arg(args, "host_id")?;
        let remote_path = str_arg(args, "remote_path")?;
        let local_path = str_arg(args, "local_path")?;
        let overwrite = args.get("overwrite").and_then(Value::as_bool).unwrap_or(false);
        let timeout_ms = args.get("timeout_ms").and_then(Value::as_u64).unwrap_or(DEFAULT_TRANSFER_TIMEOUT_MS);
        let host = self.ctx.find(host_id)?;
        check_remote_path(remote_path)?;
        let local = self.ctx.resolve(local_path);
        download_preflight(&local, overwrite)?;

        let target = json!({ "local_path": local.display().to_string(), "remote_path": remote_path, "overwrite": overwrite });
        let detail = format!("{remote_path} → {}", local.display());
        let approval = self.ctx.authorize(host, "download", &detail, &target).await?;
        let result = download_from_host(&self.ctx.program, host, remote_path, &local, self.ctx.max_transfer_bytes, timeout_ms).await;
        self.ctx.finish(host, "download", &target, approval, &result);
        result
    }
}

fn str_arg<'a>(args: &'a Value, name: &str) -> anyhow::Result<&'a str> {
    args.get(name).and_then(Value::as_str).ok_or_else(|| anyhow::anyhow!("missing required '{name}' argument"))
}

#[async_trait]
impl Tool for SshTool {
    fn spec(&self) -> ToolSpec {
        let listing = self.ctx.listing();
        let host_id = self.ctx.host_id_schema();
        let timeout = |default: &str, max: &str| {
            json!({
                "type": "number",
                "description": format!("Max time to wait before killing the connection, in milliseconds. Defaults to {default}, capped at {max}.")
            })
        };
        match self.op {
            SshOp::Exec => ToolSpec {
                name: "ssh_exec".to_string(),
                description: format!(
                    "Run a shell command on a remote server over SSH and return its exit code, stdout and stderr. \
                     Available servers (pick one by id):\n{listing}"
                ),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "host_id": host_id,
                        "command": { "type": "string", "description": "The command line, run by the remote user's login shell." },
                        "timeout_ms": timeout("30000", "300000")
                    },
                    "required": ["host_id", "command"]
                }),
            },
            SshOp::Upload => ToolSpec {
                name: "ssh_upload".to_string(),
                description: format!(
                    "Copy one local file to a remote server over SSH (up to 100 MB). Refuses to replace an existing \
                     remote file unless overwrite is true. Available servers (pick one by id):\n{listing}"
                ),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "host_id": host_id,
                        "local_path": { "type": "string", "description": "File to send. Relative to the vault root, or absolute." },
                        "remote_path": { "type": "string", "description": "Destination file path on the server (relative paths start in the remote home)." },
                        "overwrite": { "type": "boolean", "description": "Replace the remote file if it exists. Defaults to false." },
                        "timeout_ms": timeout("120000", "600000")
                    },
                    "required": ["host_id", "local_path", "remote_path"]
                }),
            },
            SshOp::Download => ToolSpec {
                name: "ssh_download".to_string(),
                description: format!(
                    "Copy one file from a remote server to this machine over SSH (up to 100 MB). Refuses to replace an \
                     existing local file unless overwrite is true. Available servers (pick one by id):\n{listing}"
                ),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "host_id": host_id,
                        "remote_path": { "type": "string", "description": "File to fetch from the server (relative paths start in the remote home)." },
                        "local_path": { "type": "string", "description": "Where to save it here. Relative to the vault root, or absolute." },
                        "overwrite": { "type": "boolean", "description": "Replace the local file if it exists. Defaults to false." },
                        "timeout_ms": timeout("120000", "600000")
                    },
                    "required": ["host_id", "remote_path", "local_path"]
                }),
            },
        }
    }

    fn scoped_to_agent(&self, agent: Option<&str>) -> Option<Arc<dyn Tool>> {
        let mut ctx = self.ctx.clone();
        ctx.agent = agent.map(str::to_string);
        Some(Arc::new(Self { ctx, op: self.op }))
    }

    fn with_approver(&self, approver: Arc<dyn Approver>) -> Option<Arc<dyn Tool>> {
        let mut ctx = self.ctx.clone();
        ctx.approver = Some(approver);
        Some(Arc::new(Self { ctx, op: self.op }))
    }

    fn is_available(&self) -> bool {
        !self.ctx.visible().is_empty()
    }

    async fn call(&self, args: Value) -> anyhow::Result<Value> {
        match self.op {
            SshOp::Exec => self.call_exec(&args).await,
            SshOp::Upload => self.call_upload(&args).await,
            SshOp::Download => self.call_download(&args).await,
        }
    }
}

/// The three SSH tools for `hosts` (already filtered to the enabled, valid ones), sharing one audit
/// log. Local relative paths resolve against `base_dir`.
pub fn ssh_tools(hosts: Vec<SshHost>, base_dir: PathBuf, audit: Option<Arc<AuditLog>>) -> Vec<Arc<dyn Tool>> {
    [SshTool::exec(hosts.clone()), SshTool::upload(hosts.clone()), SshTool::download(hosts)]
        .into_iter()
        .map(|tool| {
            let tool = tool.with_base_dir(base_dir.clone());
            let tool = match &audit {
                Some(audit) => tool.with_audit_log(audit.clone()),
                None => tool,
            };
            Arc::new(tool) as Arc<dyn Tool>
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(id: &str, agents: &[&str]) -> SshHost {
        SshHost {
            id: id.into(),
            host: "example.com".into(),
            user: "deploy".into(),
            port: 22,
            identity_file: None,
            agents: agents.iter().map(|a| a.to_string()).collect(),
            require_approval: false,
        }
    }

    #[test]
    fn builds_the_expected_argument_list() {
        let mut h = host("web", &[]);
        h.port = 2222;
        h.identity_file = Some("/home/me/.ssh/id_ed25519".into());
        assert_eq!(
            ssh_args(&h, "uptime"),
            [
                "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=yes", "-o", "ConnectTimeout=10", "-o",
                "IdentitiesOnly=yes", "-i", "/home/me/.ssh/id_ed25519", "-p", "2222", "-l", "deploy", "--",
                "example.com", "uptime"
            ]
        );
        // No key configured: no -i / IdentitiesOnly, so ssh falls back to the agent and ~/.ssh/config.
        assert!(!ssh_args(&host("web", &[]), "uptime").contains(&"-i".to_string()));
    }

    #[test]
    fn validate_rejects_option_and_uri_smuggling() {
        let mut h = host("web", &[]);
        assert!(h.validate().is_ok());

        h.host = "-oProxyCommand=evil".into();
        assert!(h.validate().is_err());
        h.host = "ssh://root@evil:22".into();
        assert!(h.validate().is_err());
        h.host = "a b".into();
        assert!(h.validate().is_err());
        h.host = "user@host".into();
        assert!(h.validate().is_err());
        h.host = "fe80::1%eth0".into();
        assert!(h.validate().is_ok());

        let mut h = host("web", &[]);
        h.user = "-l".into();
        assert!(h.validate().is_err());
        h.user = "".into();
        assert!(h.validate().is_err());

        let mut h = host("web", &[]);
        h.port = 0;
        assert!(h.validate().is_err());
        assert!(host(" ", &[]).validate().is_err());
    }

    #[test]
    fn visibility_follows_the_agent_list() {
        let open = host("open", &[]);
        let scoped = host("scoped", &["ops"]);

        assert!(open.visible_to(None) && open.visible_to(Some("anyone")));
        assert!(!scoped.visible_to(None));
        assert!(!scoped.visible_to(Some("other")));
        assert!(scoped.visible_to(Some("ops")));
    }

    #[test]
    fn spec_lists_only_the_hosts_the_agent_can_use() {
        let tool = SshTool::exec(vec![host("open", &[]), host("scoped", &["ops"])]);

        let spec = tool.spec();
        assert!(spec.description.contains("open") && !spec.description.contains("scoped"));
        assert_eq!(spec.parameters["properties"]["host_id"]["enum"], json!(["open"]));

        let scoped = tool.scoped_to_agent(Some("ops")).unwrap();
        assert_eq!(scoped.spec().parameters["properties"]["host_id"]["enum"], json!(["open", "scoped"]));
    }

    #[test]
    fn is_unavailable_when_no_host_is_visible() {
        let tool = SshTool::exec(vec![host("scoped", &["ops"])]);
        assert!(!tool.is_available());
        assert!(tool.scoped_to_agent(Some("ops")).unwrap().is_available());
        // Re-scoping keeps the full list: going back to a different agent doesn't lose hosts.
        let back = tool.scoped_to_agent(Some("ops")).unwrap().scoped_to_agent(Some("other")).unwrap();
        assert!(!back.is_available());
        assert!(back.scoped_to_agent(Some("ops")).unwrap().is_available());
    }

    #[tokio::test]
    async fn call_requires_arguments_and_a_visible_host() {
        let tool = SshTool::exec(vec![host("scoped", &["ops"])]);

        assert!(tool.call(json!({ "command": "ls" })).await.unwrap_err().to_string().contains("host_id"));
        assert!(tool.call(json!({ "host_id": "scoped" })).await.unwrap_err().to_string().contains("command"));
        let err = tool.call(json!({ "host_id": "scoped", "command": "ls" })).await.unwrap_err();
        assert!(err.to_string().contains("unknown or unavailable"));
        let err = tool.call(json!({ "host_id": "nope", "command": "ls" })).await.unwrap_err();
        assert!(err.to_string().contains("unknown or unavailable"));
    }

    #[tokio::test]
    async fn refuses_a_command_that_would_be_read_as_an_ssh_option() {
        let tool = SshTool::exec(vec![host("web", &[])]).with_program("false");
        let err = tool.call(json!({ "host_id": "web", "command": "-oProxyCommand=touch /tmp/pwned" })).await.unwrap_err();
        assert!(err.to_string().contains("must not start with '-'"));
        assert!(tool.call(json!({ "host_id": "web", "command": "   " })).await.is_err());
    }

    #[test]
    fn summarize_test_prefers_the_hint_then_stderr() {
        assert!(summarize_test(&json!({ "exit_code": 0 })).ok);
        assert!(summarize_test(&json!({ "exit_code": null, "timed_out": true })).message.contains("Timed out"));
        let hinted = summarize_test(&json!({ "exit_code": 255, "stderr": "raw", "hint": "trust the key" }));
        assert_eq!((hinted.ok, hinted.message.as_str()), (false, "trust the key"));
        assert_eq!(summarize_test(&json!({ "exit_code": 255, "stderr": " Connection refused \n" })).message, "Connection refused");
        assert!(summarize_test(&json!({ "exit_code": 255, "stderr": "" })).message.contains("no error message"));
    }

    #[test]
    fn failure_hint_explains_the_two_fixable_causes() {
        let h = host("web", &[]);
        assert!(failure_hint(&h, "Host key verification failed.").unwrap().contains("ssh -p 22 deploy@example.com"));
        assert!(failure_hint(&h, "deploy@example.com: Permission denied (publickey).").unwrap().contains("ssh-agent"));
        assert!(failure_hint(&h, "Connection refused").is_none());
    }

    /// Writes an executable stand-in for `ssh` into a fresh temp dir and returns its path.
    #[cfg(unix)]
    fn fake_ssh(body: &str) -> String {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!(
            "warden-ssh-tool-test-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ssh");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        // A test thread forking at this very moment inherits our write handle until its own exec,
        // and exec-ing a file that is still open for writing fails with ETXTBSY ("Text file
        // busy"). Give those forks time to exec before the script gets used.
        std::thread::sleep(Duration::from_millis(60));
        path.to_string_lossy().into_owned()
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn passes_the_built_arguments_to_the_program_and_captures_its_output() {
        let program = fake_ssh(r#"for a in "$@"; do echo "$a"; done; echo oops >&2; exit 3"#);
        let tool = SshTool::exec(vec![host("web", &[])]).with_program(program);

        let result = tool.call(json!({ "host_id": "web", "command": "uptime -p" })).await.unwrap();

        assert_eq!(result["exit_code"], json!(3));
        assert_eq!(result["stderr"].as_str().unwrap().trim(), "oops");
        assert_eq!(result["host_id"], json!("web"));
        assert_eq!(result["timed_out"], json!(false));
        let lines: Vec<&str> = result["stdout"].as_str().unwrap().lines().collect();
        assert_eq!(&lines[lines.len() - 3..], ["--", "example.com", "uptime -p"]);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn kills_the_connection_when_it_exceeds_the_timeout() {
        let tool = SshTool::exec(vec![host("web", &[])]).with_program(fake_ssh("sleep 5"));
        let result = tool.call(json!({ "host_id": "web", "command": "ls", "timeout_ms": 50 })).await.unwrap();
        assert_eq!(result["timed_out"], json!(true));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn adds_a_hint_when_the_host_key_is_not_trusted() {
        let program = fake_ssh("echo 'Host key verification failed.' >&2; exit 255");
        let tool = SshTool::exec(vec![host("web", &[])]).with_program(program);
        let result = tool.call(json!({ "host_id": "web", "command": "ls" })).await.unwrap();
        assert_eq!(result["exit_code"], json!(255));
        assert!(result["hint"].as_str().unwrap().contains("fingerprint"));
    }

    #[tokio::test]
    async fn a_missing_ssh_binary_is_a_clear_error() {
        let tool = SshTool::exec(vec![host("web", &[])]).with_program("/nonexistent/ssh");
        let err = tool.call(json!({ "host_id": "web", "command": "ls" })).await.unwrap_err();
        assert!(err.to_string().contains("OpenSSH"));
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "warden-ssh-{tag}-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn shell_quote_keeps_hostile_paths_as_one_word() {
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
        assert_eq!(shell_quote("$(rm -rf ~); x"), "'$(rm -rf ~); x'");
        assert_eq!(shell_quote("-rf"), "'-rf'");
    }

    #[test]
    fn remote_commands_pin_posix_sh_and_noclobber() {
        assert_eq!(remote_write_command("/tmp/a b", false), r#"sh -c 'set -C; cat > '\''/tmp/a b'\'''"#);
        assert_eq!(remote_write_command("/tmp/x", true), r#"sh -c 'cat > '\''/tmp/x'\'''"#);
        assert_eq!(remote_read_command("-weird"), r#"sh -c 'cat -- '\''-weird'\'''"#);
    }

    #[test]
    fn remote_path_must_be_a_single_printable_line() {
        assert!(check_remote_path("/var/log/syslog").is_ok());
        assert!(check_remote_path("   ").is_err());
        assert!(check_remote_path("a\nb").is_err());
        assert!(check_remote_path("a\0b").is_err());
    }

    #[test]
    fn upload_and_download_specs_list_only_visible_hosts() {
        let hosts = vec![host("open", &[]), host("scoped", &["ops"])];
        for tool in [SshTool::upload(hosts.clone()), SshTool::download(hosts.clone())] {
            let spec = tool.spec();
            assert!(spec.name == "ssh_upload" || spec.name == "ssh_download");
            assert_eq!(spec.parameters["properties"]["host_id"]["enum"], json!(["open"]));
            let ops = tool.scoped_to_agent(Some("ops")).unwrap();
            assert_eq!(ops.spec().parameters["properties"]["host_id"]["enum"], json!(["open", "scoped"]));
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn upload_streams_the_local_file_and_reports_the_size() {
        let dir = temp_dir("up");
        let out = dir.join("received");
        std::fs::write(dir.join("payload.txt"), "hello world").unwrap();
        // The stand-in "ssh" writes what it gets on stdin to a file and echoes its last argument.
        let program = fake_ssh(&format!("cat > '{}'; for a in \"$@\"; do last=\"$a\"; done; echo \"$last\" >&2", out.display()));
        let tool = SshTool::upload(vec![host("web", &[])]).with_program(program).with_base_dir(&dir);

        let result = tool
            .call(json!({ "host_id": "web", "local_path": "payload.txt", "remote_path": "/srv/it's here.txt" }))
            .await
            .unwrap();

        assert_eq!(result["exit_code"], json!(0));
        assert_eq!(result["bytes"], json!(11));
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "hello world");
        let remote = result["stderr"].as_str().unwrap().trim();
        assert_eq!(remote, r#"sh -c 'set -C; cat > '\''/srv/it'\''\'\'''\''s here.txt'\'''"#);
    }

    #[tokio::test]
    async fn upload_refuses_missing_directory_and_oversized_files_before_connecting() {
        let dir = temp_dir("up-refuse");
        std::fs::write(dir.join("big.bin"), vec![0u8; 100]).unwrap();
        let tool = SshTool::upload(vec![host("web", &[])]).with_program("/nonexistent/ssh").with_base_dir(&dir).with_max_transfer_bytes(50);
        let call = |local: &str| json!({ "host_id": "web", "local_path": local, "remote_path": "/x" });

        assert!(tool.call(call("nope.txt")).await.unwrap_err().to_string().contains("cannot read"));
        assert!(tool.call(call(".")).await.unwrap_err().to_string().contains("not a regular file"));
        assert!(tool.call(call("big.bin")).await.unwrap_err().to_string().contains("transfer limit"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn upload_hints_at_overwrite_when_the_remote_file_exists() {
        let dir = temp_dir("up-exists");
        std::fs::write(dir.join("a.txt"), "x").unwrap();
        let program = fake_ssh("cat >/dev/null; echo 'sh: 1: cannot create /x: File exists' >&2; exit 1");
        let tool = SshTool::upload(vec![host("web", &[])]).with_program(program).with_base_dir(&dir);
        let result = tool.call(json!({ "host_id": "web", "local_path": "a.txt", "remote_path": "/x" })).await.unwrap();
        assert_eq!(result["bytes"], json!(0));
        assert!(result["hint"].as_str().unwrap().contains("overwrite=true"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn download_writes_atomically_and_creates_parent_directories() {
        let dir = temp_dir("down");
        let tool = SshTool::download(vec![host("web", &[])]).with_program(fake_ssh("printf 'remote data'")).with_base_dir(&dir);

        let result = tool.call(json!({ "host_id": "web", "remote_path": "/etc/motd", "local_path": "sub/dir/motd" })).await.unwrap();

        assert_eq!(result["exit_code"], json!(0));
        assert_eq!(result["bytes"], json!(11));
        assert_eq!(std::fs::read_to_string(dir.join("sub/dir/motd")).unwrap(), "remote data");
        assert!(!dir.join("sub/dir/motd.part").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn download_refuses_to_replace_a_local_file_without_overwrite() {
        let dir = temp_dir("down-exists");
        std::fs::write(dir.join("keep"), "mine").unwrap();
        let tool = SshTool::download(vec![host("web", &[])]).with_program(fake_ssh("printf 'theirs'")).with_base_dir(&dir);
        let call = |overwrite: bool| json!({ "host_id": "web", "remote_path": "/x", "local_path": "keep", "overwrite": overwrite });

        assert!(tool.call(call(false)).await.unwrap_err().to_string().contains("overwrite=true"));
        assert_eq!(std::fs::read_to_string(dir.join("keep")).unwrap(), "mine");
        tool.call(call(true)).await.unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("keep")).unwrap(), "theirs");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_failed_download_leaves_no_file_behind() {
        let dir = temp_dir("down-fail");
        let program = fake_ssh("echo 'cat: /x: No such file or directory' >&2; exit 1");
        let tool = SshTool::download(vec![host("web", &[])]).with_program(program).with_base_dir(&dir);

        let result = tool.call(json!({ "host_id": "web", "remote_path": "/x", "local_path": "out" })).await.unwrap();

        assert_eq!(result["exit_code"], json!(1));
        assert!(result["stderr"].as_str().unwrap().contains("No such file"));
        assert!(!dir.join("out").exists() && !dir.join("out.part").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_download_over_the_limit_is_cut_off_and_cleaned_up() {
        let dir = temp_dir("down-big");
        let tool = SshTool::download(vec![host("web", &[])])
            .with_program(fake_ssh("head -c 100000 /dev/zero"))
            .with_base_dir(&dir)
            .with_max_transfer_bytes(1000);

        let err = tool.call(json!({ "host_id": "web", "remote_path": "/x", "local_path": "out" })).await.unwrap_err();

        assert!(err.to_string().contains("transfer limit"));
        assert!(!dir.join("out").exists() && !dir.join("out.part").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_download_that_times_out_leaves_no_file_behind() {
        let dir = temp_dir("down-timeout");
        let tool = SshTool::download(vec![host("web", &[])]).with_program(fake_ssh("printf 'partial'; sleep 5")).with_base_dir(&dir);
        let result = tool.call(json!({ "host_id": "web", "remote_path": "/x", "local_path": "out", "timeout_ms": 200 })).await.unwrap();
        assert_eq!(result["timed_out"], json!(true));
        assert!(!dir.join("out").exists() && !dir.join("out.part").exists());
    }

    /// Answers every request with a fixed reply, remembering what it was asked.
    struct FixedApprover {
        answer: bool,
        asked: Mutex<Vec<ApprovalRequest>>,
    }

    #[async_trait]
    impl Approver for FixedApprover {
        async fn approve(&self, request: ApprovalRequest) -> bool {
            self.asked.lock().unwrap().push(request);
            self.answer
        }
    }

    struct NeverAnswers;

    #[async_trait]
    impl Approver for NeverAnswers {
        async fn approve(&self, _request: ApprovalRequest) -> bool {
            std::future::pending().await
        }
    }

    fn approval_host() -> SshHost {
        SshHost { require_approval: true, ..host("prod", &[]) }
    }

    fn audit_lines(path: &Path) -> Vec<Value> {
        std::fs::read_to_string(path).unwrap_or_default().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
    }

    #[tokio::test]
    async fn a_host_that_requires_approval_refuses_when_nobody_can_be_asked() {
        let log = temp_dir("audit-none").join("audit.jsonl");
        let tool = SshTool::exec(vec![approval_host()]).with_program("false").with_audit_log(Arc::new(AuditLog::new(&log)));

        let err = tool.call(json!({ "host_id": "prod", "command": "reboot" })).await.unwrap_err();

        assert!(err.to_string().contains("requires approval"));
        let lines = audit_lines(&log);
        assert_eq!((lines[0]["approval"].as_str(), lines[0]["command"].as_str()), (Some("unavailable"), Some("reboot")));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn an_approved_command_runs_and_the_approver_sees_exactly_what_will_run() {
        let log = temp_dir("audit-yes").join("audit.jsonl");
        let approver = Arc::new(FixedApprover { answer: true, asked: Mutex::new(Vec::new()) });
        let tool = SshTool::exec(vec![approval_host()])
            .with_program(fake_ssh("exit 0"))
            .with_audit_log(Arc::new(AuditLog::new(&log)))
            .with_approver(approver.clone())
            .unwrap();

        let result = tool.call(json!({ "host_id": "prod", "command": "uptime" })).await.unwrap();

        assert_eq!(result["exit_code"], json!(0));
        assert_eq!(*approver.asked.lock().unwrap(), [ApprovalRequest { host_id: "prod".into(), action: "exec".into(), detail: "uptime".into() }]);
        assert_eq!(audit_lines(&log)[0]["approval"], json!("approved"));
    }

    #[tokio::test]
    async fn a_refused_command_never_reaches_ssh() {
        let log = temp_dir("audit-no").join("audit.jsonl");
        let approver = Arc::new(FixedApprover { answer: false, asked: Mutex::new(Vec::new()) });
        // "/nonexistent/ssh" would fail with an OpenSSH error if the command were ever attempted.
        let tool = SshTool::exec(vec![approval_host()])
            .with_program("/nonexistent/ssh")
            .with_audit_log(Arc::new(AuditLog::new(&log)))
            .with_approver(approver)
            .unwrap();

        let err = tool.call(json!({ "host_id": "prod", "command": "rm -rf /" })).await.unwrap_err();

        assert!(err.to_string().contains("did not approve"));
        assert_eq!(audit_lines(&log)[0]["approval"], json!("denied"));
    }

    #[tokio::test]
    async fn an_unanswered_prompt_counts_as_a_refusal() {
        let tool = SshTool::exec(vec![approval_host()])
            .with_program("/nonexistent/ssh")
            .with_approval_timeout(Duration::from_millis(50))
            .with_approver(Arc::new(NeverAnswers))
            .unwrap();
        let err = tool.call(json!({ "host_id": "prod", "command": "ls" })).await.unwrap_err();
        assert!(err.to_string().contains("did not approve"));
    }

    #[tokio::test]
    async fn a_command_that_is_invalid_is_rejected_before_asking_anyone() {
        let approver = Arc::new(FixedApprover { answer: true, asked: Mutex::new(Vec::new()) });
        let tool = SshTool::exec(vec![approval_host()]).with_program("false").with_approver(approver.clone()).unwrap();
        assert!(tool.call(json!({ "host_id": "prod", "command": "-oProxyCommand=x" })).await.is_err());
        assert!(approver.asked.lock().unwrap().is_empty());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn hosts_without_the_flag_never_ask_and_the_audit_log_records_every_call() {
        use std::os::unix::fs::PermissionsExt;
        let log = temp_dir("audit-plain").join("nested").join("audit.jsonl");
        let approver = Arc::new(FixedApprover { answer: false, asked: Mutex::new(Vec::new()) });
        let tool = SshTool::exec(vec![host("web", &[])])
            .with_program(fake_ssh("echo SECRET-OUTPUT; exit 7"))
            .with_audit_log(Arc::new(AuditLog::new(&log)))
            .for_agent(Some("ops".into()))
            .with_approver(approver.clone())
            .unwrap();

        tool.call(json!({ "host_id": "web", "command": "ls" })).await.unwrap();
        tool.call(json!({ "host_id": "web", "command": "df" })).await.unwrap();

        assert!(approver.asked.lock().unwrap().is_empty());
        let lines = audit_lines(&log);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1]["command"], json!("df"));
        assert_eq!((lines[0]["host_id"].as_str(), lines[0]["action"].as_str(), lines[0]["approval"].as_str()), (Some("web"), Some("exec"), Some("not_required")));
        assert_eq!(lines[0]["outcome"]["exit_code"], json!(7));
        assert!(lines[0]["ts"].as_u64().unwrap() > 0);
        // Output can hold secrets, so it never goes in the log; the file itself is owner-only.
        assert!(!std::fs::read_to_string(&log).unwrap().contains("SECRET-OUTPUT"));
        assert_eq!(std::fs::metadata(&log).unwrap().permissions().mode() & 0o777, 0o600);
    }

    #[tokio::test]
    async fn re_scoping_keeps_the_approver_and_the_approver_keeps_the_scope() {
        let approver = Arc::new(FixedApprover { answer: true, asked: Mutex::new(Vec::new()) });
        let scoped = SshTool::exec(vec![host("open", &[]), host("scoped", &["ops"])]).with_approver(approver).unwrap();
        assert!(!scoped.is_available() || scoped.spec().parameters["properties"]["host_id"]["enum"] == json!(["open"]));
        let ops = scoped.scoped_to_agent(Some("ops")).unwrap();
        assert_eq!(ops.spec().parameters["properties"]["host_id"]["enum"], json!(["open", "scoped"]));
    }
}
