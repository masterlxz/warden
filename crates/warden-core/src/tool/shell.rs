use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::memory::Vault;
use crate::tool::{ApprovalRequest, Approver, Tool, ToolSpec};

pub(crate) const DEFAULT_TIMEOUT_MS: u64 = 30_000;
pub(crate) const MAX_TIMEOUT_MS: u64 = 300_000;
const MAX_OUTPUT_BYTES: usize = 20_000;
/// How long a project's shell waits for the person's yes to one command; no answer counts as no (like the SSH hosts').
const APPROVAL_TIMEOUT: Duration = Duration::from_secs(120);

/// Runs a shell command on the local machine. Deliberately no sandboxing or allowlist — same
/// trust model the file tools already have (no path scoping either). Gated behind an opt-in
/// flag at the `bootstrap()` level precisely because this one is a bigger blast radius than
/// read/write file.
///
/// A code project's conversations (P103 b) get the other mode, `in_folder`: it starts in the project's working folder
/// and asks the person before **every** command. It is still no sandbox — a command can `cd ..` — so the approval is
/// the protection.
pub struct ShellTool {
    mode: Mode,
}

enum Mode {
    /// The shell of everyone else: starts in the vault, runs what it is told.
    Vault(Arc<Vault>),
    Project(ProjectShell),
}

struct ProjectShell {
    /// The project's name, for the person to see whose command it is.
    project: String,
    folder: PathBuf,
    approver: Option<Arc<dyn Approver>>,
}

impl ShellTool {
    pub fn new(vault: Arc<Vault>) -> Self {
        Self { mode: Mode::Vault(vault) }
    }

    /// The shell of a code project: starts in `folder` (which must already exist — it is never created) and puts each
    /// command to `with_approver`'s approver first. Without one it refuses, as a channel that can't ask must.
    pub fn in_folder(project: impl Into<String>, folder: PathBuf) -> Self {
        Self { mode: Mode::Project(ProjectShell { project: project.into(), folder, approver: None }) }
    }
}

#[async_trait]
impl Tool for ShellTool {
    /// Its default working directory is the vault's folder, so it follows the vault too. A project's shell keeps its
    /// working folder: the vault it was built next to is not what it is about.
    fn with_vault(&self, vault: &Arc<Vault>) -> Option<Arc<dyn Tool>> {
        match &self.mode {
            Mode::Vault(_) => Some(Arc::new(Self::new(vault.clone()))),
            Mode::Project(_) => None,
        }
    }

    /// Only a project's shell asks; the ordinary one runs as before.
    fn with_approver(&self, approver: Arc<dyn Approver>) -> Option<Arc<dyn Tool>> {
        match &self.mode {
            Mode::Vault(_) => None,
            Mode::Project(shell) => Some(Arc::new(Self {
                mode: Mode::Project(ProjectShell { project: shell.project.clone(), folder: shell.folder.clone(), approver: Some(approver) }),
            })),
        }
    }

    fn spec(&self) -> ToolSpec {
        let description = match &self.mode {
            Mode::Vault(_) => "Run a shell command on the local machine (sh -c on Linux/macOS, cmd /C on Windows) and return its \
                 exit code, stdout and stderr. Runs in the vault root directory by default."
                .to_string(),
            Mode::Project(shell) => format!(
                "Run a shell command on the local machine (sh -c on Linux/macOS, cmd /C on Windows) and return its exit code, stdout \
                 and stderr. Runs in the project's working folder, {}, by default (a relative cwd starts there). It is not confined \
                 to that folder, and the person is asked to approve every command before it runs, so a refused command did not run.",
                shell.folder.display()
            ),
        };
        ToolSpec {
            name: "shell".to_string(),
            description,
            parameters: json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The command line to run."
                    },
                    "cwd": {
                        "type": "string",
                        "description": "Working directory for the command. Relative to the vault root, or \
                                         absolute. Defaults to the vault root."
                    },
                    "timeout_ms": {
                        "type": "number",
                        "description": "Max time to wait before killing the process, in milliseconds. Defaults \
                                         to 30000, capped at 300000."
                    }
                },
                "required": ["command"]
            }),
        }
    }

    async fn call(&self, args: Value) -> anyhow::Result<Value> {
        let command = args
            .get("command")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("missing required 'command' argument"))?;

        let base = match &self.mode {
            Mode::Vault(vault) => vault.root().clone(),
            Mode::Project(shell) => shell.folder.clone(),
        };
        let cwd = match args.get("cwd").and_then(Value::as_str) {
            Some(cwd) => {
                let path = std::path::Path::new(cwd);
                if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    base.join(path)
                }
            }
            None => base,
        };
        match &self.mode {
            Mode::Vault(_) => std::fs::create_dir_all(&cwd)?,
            Mode::Project(shell) => {
                // The folder is the repository the person named: one that isn't there is a mistake to say, not to paper over.
                if !cwd.is_dir() {
                    anyhow::bail!("the folder {} doesn't exist on this machine (the project's working folder is {})", cwd.display(), shell.folder.display());
                }
                let Some(approver) = &shell.approver else {
                    anyhow::bail!("this project's shell asks the person before every command, and this channel can't ask (use the desktop app or the web)");
                };
                let request = ApprovalRequest { target: shell.project.clone(), action: "shell".to_string(), detail: format!("in {}: {command}", cwd.display()) };
                if !tokio::time::timeout(APPROVAL_TIMEOUT, approver.approve(request)).await.unwrap_or(false) {
                    anyhow::bail!("the person did not approve this command, so it did not run");
                }
            }
        }

        let timeout_ms = args.get("timeout_ms").and_then(Value::as_u64).unwrap_or(DEFAULT_TIMEOUT_MS).min(MAX_TIMEOUT_MS);

        let mut cmd = platform_shell_command(command);
        cmd.current_dir(&cwd).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
        let child = cmd.spawn().map_err(|e| anyhow::anyhow!("failed to spawn command: {e}"))?;

        match tokio::time::timeout(Duration::from_millis(timeout_ms), child.wait_with_output()).await {
            Ok(result) => {
                let output = result.map_err(|e| anyhow::anyhow!("failed to run command: {e}"))?;
                Ok(json!({
                    "exit_code": output.status.code(),
                    "stdout": truncate(&String::from_utf8_lossy(&output.stdout)),
                    "stderr": truncate(&String::from_utf8_lossy(&output.stderr)),
                    "timed_out": false,
                }))
            }
            Err(_) => Ok(json!({
                "exit_code": null,
                "stdout": "",
                "stderr": "",
                "timed_out": true,
            })),
        }
    }
}

fn platform_shell_command(command: &str) -> tokio::process::Command {
    #[cfg(target_os = "windows")]
    let mut cmd = tokio::process::Command::new("cmd");
    #[cfg(target_os = "windows")]
    cmd.arg("/C").arg(command);

    #[cfg(not(target_os = "windows"))]
    let mut cmd = tokio::process::Command::new("sh");
    #[cfg(not(target_os = "windows"))]
    cmd.arg("-c").arg(command);

    cmd
}

/// Cuts `s` down to `MAX_OUTPUT_BYTES` so a runaway command can't blow up the model's context,
/// backing off to the nearest char boundary so a multi-byte UTF-8 sequence never gets split.
pub(crate) fn truncate(s: &str) -> String {
    if s.len() <= MAX_OUTPUT_BYTES {
        return s.to_string();
    }
    let mut end = MAX_OUTPUT_BYTES;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n...[truncated, {} bytes total]", &s[..end], s.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_vault() -> Arc<Vault> {
        Arc::new(Vault::new(std::env::temp_dir().join(format!(
            "warden-shell-tool-test-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ))))
    }

    #[tokio::test]
    async fn runs_a_command_and_captures_stdout() {
        let tool = ShellTool::new(temp_vault());
        let result = tool.call(json!({ "command": "echo hi" })).await.unwrap();

        assert_eq!(result["exit_code"], json!(0));
        assert_eq!(result["stdout"].as_str().unwrap().trim(), "hi");
        assert_eq!(result["timed_out"], json!(false));
    }

    #[tokio::test]
    async fn requires_command_argument() {
        let tool = ShellTool::new(temp_vault());
        let err = tool.call(json!({})).await.unwrap_err();
        assert!(err.to_string().contains("command"));
    }

    #[tokio::test]
    async fn captures_non_zero_exit_code() {
        let tool = ShellTool::new(temp_vault());
        let result = tool.call(json!({ "command": "exit 3" })).await.unwrap();

        assert_eq!(result["exit_code"], json!(3));
    }

    #[tokio::test]
    async fn defaults_cwd_to_vault_root() {
        let vault = temp_vault();
        let tool = ShellTool::new(vault.clone());

        tool.call(json!({ "command": "echo hi > marker.txt" })).await.unwrap();

        assert!(vault.root().join("marker.txt").exists());
    }

    #[tokio::test]
    async fn cwd_is_resolved_relative_to_the_vault_root() {
        let vault = temp_vault();
        let tool = ShellTool::new(vault.clone());

        tool.call(json!({ "command": "echo hi > marker.txt", "cwd": "subdir" })).await.unwrap();

        assert!(vault.root().join("subdir").join("marker.txt").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn kills_the_process_when_it_exceeds_the_timeout() {
        let tool = ShellTool::new(temp_vault());
        let result = tool.call(json!({ "command": "sleep 5", "timeout_ms": 50 })).await.unwrap();

        assert_eq!(result["timed_out"], json!(true));
    }

    #[test]
    fn truncate_leaves_short_strings_untouched() {
        assert_eq!(truncate("hi"), "hi");
    }

    #[test]
    fn truncate_cuts_long_strings_at_a_char_boundary() {
        let long = "a".repeat(MAX_OUTPUT_BYTES + 100);
        let result = truncate(&long);

        assert!(result.starts_with(&"a".repeat(MAX_OUTPUT_BYTES)));
        assert!(result.contains("truncated"));
    }
}
