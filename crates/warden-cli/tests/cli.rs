//! Process-level smoke tests for the `warden` binary (Phase 1.9). These spawn the real
//! compiled binary rather than calling into warden-core's API directly, so they catch
//! wiring mistakes in `main.rs` itself — arg parsing, env var handling, exit codes — that
//! warden-core's own integration tests (`crates/warden-core/tests/pipeline.rs`) can't see.
//! Every scenario here fails before any network call is made, so no real API key is needed.

use std::io::Write;
use std::process::{Command, Stdio};

fn warden_command() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_warden"));
    cmd.env_clear();
    cmd
}

fn temp_vault_path(name: &str) -> String {
    std::env::temp_dir()
        .join(format!(
            "warden-cli-test-{name}-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ))
        .to_string_lossy()
        .to_string()
}

#[test]
fn fails_clearly_without_a_gemini_key() {
    let output = warden_command().output().unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("GEMINI_API_KEY"), "stderr was: {stderr}");
}

#[test]
fn fails_clearly_without_an_openai_key_when_openai_selected() {
    let output = warden_command().args(["--provider", "openai"]).output().unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("OPENAI_API_KEY"), "stderr was: {stderr}");
}

#[test]
fn starts_up_and_exits_cleanly_with_a_key_present() {
    let vault_path = temp_vault_path("startup");

    let mut child = warden_command()
        .env("GEMINI_API_KEY", "fake-key-for-startup-test")
        .args(["--vault-path", &vault_path])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    child.stdin.take().unwrap().write_all(b"exit\n").unwrap();
    let output = child.wait_with_output().unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Warden"), "stdout was: {stdout}");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("TAVILY_API_KEY"), "stderr was: {stderr}");

    assert!(std::path::Path::new(&vault_path).is_dir(), "vault root should be created on startup");
}
