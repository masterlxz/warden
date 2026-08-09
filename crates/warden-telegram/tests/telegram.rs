//! Process-level smoke tests for the `warden-telegram` binary (Fase 2), same spirit as
//! `warden-cli/tests/cli.rs`: spawn the real compiled binary to catch wiring mistakes in
//! `main.rs` (arg parsing, config precedence, fail-fast messages) that the in-crate hermetic
//! tests in `src/telegram.rs` can't see. Every scenario here fails before `run_bot` ever starts
//! polling, so none of them make a real network call or hang waiting on Telegram.

use std::process::Command;

fn warden_telegram_command() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_warden-telegram"));
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
fn fails_clearly_without_a_telegram_token() {
    let output = warden_telegram_command().env("GEMINI_API_KEY", "fake-key").output().unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("TELEGRAM_BOT_TOKEN"), "stderr was: {stderr}");
}

#[test]
fn fails_clearly_without_a_gemini_key_once_the_token_is_present() {
    let output = warden_telegram_command().env("TELEGRAM_BOT_TOKEN", "fake-token").output().unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("GEMINI_API_KEY"), "stderr was: {stderr}");
}

#[test]
fn reads_telegram_token_from_config_file() {
    let config_path = unique_temp_path("warden-telegram-test-config").with_extension("toml");
    std::fs::write(&config_path, "[api_keys]\ntelegram_bot_token = \"fake-token-from-config\"\n").unwrap();

    let output = warden_telegram_command().args(["--config", config_path.to_str().unwrap()]).output().unwrap();

    // No GEMINI_API_KEY set — if the token had NOT been picked up from the config file, this
    // would fail on TELEGRAM_BOT_TOKEN instead, so failing specifically on GEMINI_API_KEY proves
    // the config file's token was read.
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("GEMINI_API_KEY"), "stderr was: {stderr}");

    std::fs::remove_file(&config_path).ok();
}

#[test]
fn fails_clearly_when_explicit_config_path_is_missing() {
    let missing_config_path = unique_temp_path("warden-telegram-test-missing-config").with_extension("toml");

    let output = warden_telegram_command().args(["--config", missing_config_path.to_str().unwrap()]).output().unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("config file"), "stderr was: {stderr}");
}
