use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::tool::shell::{truncate, DEFAULT_TIMEOUT_MS, MAX_TIMEOUT_MS};
use crate::tool::{Tool, ToolSpec};

/// How long `ssh` itself waits to establish the connection, separate from the whole-command
/// timeout — a dead host should fail fast, not eat the command's entire time budget.
const CONNECT_TIMEOUT_SECS: u32 = 10;

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

    let mut cmd = tokio::process::Command::new(program);
    cmd.args(ssh_args(host, command)).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    let child = cmd
        .spawn()
        .map_err(|e| anyhow::anyhow!("failed to run '{program}' — is the OpenSSH client installed and on PATH? ({e})"))?;

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

/// Runs a command on a registered SSH server. The model picks a server by `host_id` only — never a
/// hostname or user — and only among the ones the current agent may use. No sandbox and no command
/// allowlist, deliberately: inside a host the user enabled, this is the same trust as `shell`.
pub struct SshExecTool {
    hosts: Arc<Vec<SshHost>>,
    agent: Option<String>,
    program: String,
}

impl SshExecTool {
    pub fn new(hosts: Vec<SshHost>) -> Self {
        Self { hosts: Arc::new(hosts), agent: None, program: "ssh".to_string() }
    }

    pub fn for_agent(mut self, agent: Option<String>) -> Self {
        self.agent = agent;
        self
    }

    /// Points at a different `ssh` executable — only for tests, which use a stand-in script.
    pub fn with_program(mut self, program: impl Into<String>) -> Self {
        self.program = program.into();
        self
    }

    fn visible(&self) -> Vec<&SshHost> {
        self.hosts.iter().filter(|h| h.visible_to(self.agent.as_deref())).collect()
    }
}

#[async_trait]
impl Tool for SshExecTool {
    fn spec(&self) -> ToolSpec {
        let visible = self.visible();
        let listing = visible.iter().map(|h| format!("- {}: {}@{}", h.id, h.user, h.host)).collect::<Vec<_>>().join("\n");
        let ids: Vec<&str> = visible.iter().map(|h| h.id.as_str()).collect();
        ToolSpec {
            name: "ssh_exec".to_string(),
            description: format!(
                "Run a shell command on a remote server over SSH and return its exit code, stdout and stderr. \
                 Available servers (pick one by id):\n{listing}"
            ),
            parameters: json!({
                "type": "object",
                "properties": {
                    "host_id": {
                        "type": "string",
                        "enum": ids,
                        "description": "Which registered server to run the command on."
                    },
                    "command": {
                        "type": "string",
                        "description": "The command line, run by the remote user's login shell."
                    },
                    "timeout_ms": {
                        "type": "number",
                        "description": "Max time to wait before killing the connection, in milliseconds. Defaults \
                                         to 30000, capped at 300000."
                    }
                },
                "required": ["host_id", "command"]
            }),
        }
    }

    fn scoped_to_agent(&self, agent: Option<&str>) -> Option<Arc<dyn Tool>> {
        Some(Arc::new(Self { hosts: self.hosts.clone(), agent: agent.map(str::to_string), program: self.program.clone() }))
    }

    fn is_available(&self) -> bool {
        !self.visible().is_empty()
    }

    async fn call(&self, args: Value) -> anyhow::Result<Value> {
        let host_id = args
            .get("host_id")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("missing required 'host_id' argument"))?;
        let command = args
            .get("command")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("missing required 'command' argument"))?;
        let timeout_ms = args.get("timeout_ms").and_then(Value::as_u64).unwrap_or(DEFAULT_TIMEOUT_MS);

        let visible = self.visible();
        let host = visible.iter().find(|h| h.id == host_id).ok_or_else(|| {
            let ids: Vec<&str> = visible.iter().map(|h| h.id.as_str()).collect();
            anyhow::anyhow!("unknown or unavailable ssh host '{host_id}' (available: {})", ids.join(", "))
        })?;

        run_on_host(&self.program, host, command, timeout_ms).await
    }
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
        let tool = SshExecTool::new(vec![host("open", &[]), host("scoped", &["ops"])]);

        let spec = tool.spec();
        assert!(spec.description.contains("open") && !spec.description.contains("scoped"));
        assert_eq!(spec.parameters["properties"]["host_id"]["enum"], json!(["open"]));

        let scoped = tool.scoped_to_agent(Some("ops")).unwrap();
        assert_eq!(scoped.spec().parameters["properties"]["host_id"]["enum"], json!(["open", "scoped"]));
    }

    #[test]
    fn is_unavailable_when_no_host_is_visible() {
        let tool = SshExecTool::new(vec![host("scoped", &["ops"])]);
        assert!(!tool.is_available());
        assert!(tool.scoped_to_agent(Some("ops")).unwrap().is_available());
        // Re-scoping keeps the full list: going back to a different agent doesn't lose hosts.
        let back = tool.scoped_to_agent(Some("ops")).unwrap().scoped_to_agent(Some("other")).unwrap();
        assert!(!back.is_available());
        assert!(back.scoped_to_agent(Some("ops")).unwrap().is_available());
    }

    #[tokio::test]
    async fn call_requires_arguments_and_a_visible_host() {
        let tool = SshExecTool::new(vec![host("scoped", &["ops"])]);

        assert!(tool.call(json!({ "command": "ls" })).await.unwrap_err().to_string().contains("host_id"));
        assert!(tool.call(json!({ "host_id": "scoped" })).await.unwrap_err().to_string().contains("command"));
        let err = tool.call(json!({ "host_id": "scoped", "command": "ls" })).await.unwrap_err();
        assert!(err.to_string().contains("unknown or unavailable"));
        let err = tool.call(json!({ "host_id": "nope", "command": "ls" })).await.unwrap_err();
        assert!(err.to_string().contains("unknown or unavailable"));
    }

    #[tokio::test]
    async fn refuses_a_command_that_would_be_read_as_an_ssh_option() {
        let tool = SshExecTool::new(vec![host("web", &[])]).with_program("false");
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
        path.to_string_lossy().into_owned()
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn passes_the_built_arguments_to_the_program_and_captures_its_output() {
        let program = fake_ssh(r#"for a in "$@"; do echo "$a"; done; echo oops >&2; exit 3"#);
        let tool = SshExecTool::new(vec![host("web", &[])]).with_program(program);

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
        let tool = SshExecTool::new(vec![host("web", &[])]).with_program(fake_ssh("sleep 5"));
        let result = tool.call(json!({ "host_id": "web", "command": "ls", "timeout_ms": 50 })).await.unwrap();
        assert_eq!(result["timed_out"], json!(true));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn adds_a_hint_when_the_host_key_is_not_trusted() {
        let program = fake_ssh("echo 'Host key verification failed.' >&2; exit 255");
        let tool = SshExecTool::new(vec![host("web", &[])]).with_program(program);
        let result = tool.call(json!({ "host_id": "web", "command": "ls" })).await.unwrap();
        assert_eq!(result["exit_code"], json!(255));
        assert!(result["hint"].as_str().unwrap().contains("fingerprint"));
    }

    #[tokio::test]
    async fn a_missing_ssh_binary_is_a_clear_error() {
        let tool = SshExecTool::new(vec![host("web", &[])]).with_program("/nonexistent/ssh");
        let err = tool.call(json!({ "host_id": "web", "command": "ls" })).await.unwrap_err();
        assert!(err.to_string().contains("OpenSSH"));
    }
}
