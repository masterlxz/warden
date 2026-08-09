//! Process-level smoke tests for the `warden-whatsapp` binary (Fase 3), same spirit as
//! `warden-cli/tests/cli.rs` and `warden-telegram/tests/telegram.rs`: spawn the real compiled
//! binary to catch wiring mistakes in `main.rs` that the in-crate hermetic tests in
//! `src/sidecar.rs` can't see. Every scenario here fails before the sidecar's stdout is ever
//! read, so none of them hang waiting on a real WhatsApp connection.

use std::process::Command;

fn warden_whatsapp_command() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_warden-whatsapp"));
    cmd.env_clear();
    cmd
}

fn unique_temp_path(prefix: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "{prefix}-{}",
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ))
}

#[test]
fn fails_clearly_without_a_gemini_key() {
    let output = warden_whatsapp_command().output().unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("GEMINI_API_KEY"), "stderr was: {stderr}");
}

#[test]
fn fails_clearly_when_explicit_config_path_is_missing() {
    let missing_config_path = unique_temp_path("warden-whatsapp-test-missing-config").with_extension("toml");

    let output = warden_whatsapp_command().args(["--config", missing_config_path.to_str().unwrap()]).output().unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("config file"), "stderr was: {stderr}");
}

#[test]
fn fails_clearly_when_node_is_not_on_path() {
    // Just clearing PATH isn't enough to prove this — on Linux, execvp falls back to a libc
    // default search path (e.g. "/bin:/usr/bin") when PATH is entirely unset, which can still
    // find a real `node` there. Pointing PATH at a directory that can't contain it is reliable.
    let output = warden_whatsapp_command()
        .env("GEMINI_API_KEY", "fake-key")
        .env("PATH", "/warden-test-nonexistent-path")
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Node.js"), "stderr was: {stderr}");
}
