//! Where the desktop keeps what it reads from the OS config dir for its hubs (P102): the saved hubs (`hubs.json`) and, beside
//! the `config.toml`, what this computer remembers of each (`remote_hub.json`). One place, so a test can point both at a
//! folder of its own instead of the person's real one.

use std::path::PathBuf;

/// A folder a test sets so the commands read and write there. Only tests ever set it.
#[cfg(test)]
pub static TEST_DIR: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

#[cfg(test)]
fn test_dir() -> Option<PathBuf> {
    TEST_DIR.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

#[cfg(not(test))]
fn test_dir() -> Option<PathBuf> {
    None
}

const NO_CONFIG_DIR: &str = "could not determine the OS config directory";

/// The `config.toml`, whose neighbours (`remote_hub.json`...) are what this app keeps beside it.
pub fn config_file() -> Result<PathBuf, String> {
    match test_dir() {
        Some(dir) => Ok(dir.join("config.toml")),
        None => warden_bootstrap::default_config_path().ok_or_else(|| NO_CONFIG_DIR.to_string()),
    }
}

/// The saved hubs, `hubs.json`.
pub fn saved_hubs_file() -> Result<PathBuf, String> {
    match test_dir() {
        Some(dir) => Ok(dir.join("hubs.json")),
        None => warden_bootstrap::saved_hubs::default_saved_hubs_path().ok_or_else(|| NO_CONFIG_DIR.to_string()),
    }
}
