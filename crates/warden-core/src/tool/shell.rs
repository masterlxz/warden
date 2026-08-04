use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::memory::Vault;
use crate::tool::{Tool, ToolSpec};

const DEFAULT_TIMEOUT_MS: u64 = 30_000;
const MAX_TIMEOUT_MS: u64 = 300_000;
const MAX_OUTPUT_BYTES: usize = 20_000;

/// Runs a shell command on the local machine. Deliberately no sandboxing or allowlist — same
/// trust model the file tools already have (no path scoping either). Gated behind an opt-in
/// flag at the `bootstrap()` level precisely because this one is a bigger blast radius than
/// read/write file.
pub struct ShellTool {
    vault: Arc<Vault>,
}

impl ShellTool {
    pub fn new(vault: Arc<Vault>) -> Self {
        Self { vault }
    }
}

#[async_trait]
impl Tool for ShellTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "shell".to_string(),
            description:
                "Run a shell command on the local machine (sh -c on Linux/macOS, cmd /C on Windows) and return its \
                 exit code, stdout and stderr. Runs in the vault root directory by default."
                    .to_string(),
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

        let cwd = match args.get("cwd").and_then(Value::as_str) {
            Some(cwd) => {
                let path = std::path::Path::new(cwd);
                if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    self.vault.root().join(path)
                }
            }
            None => self.vault.root().clone(),
        };
        std::fs::create_dir_all(&cwd)?;

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
fn truncate(s: &str) -> String {
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
