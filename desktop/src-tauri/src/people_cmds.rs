//! Tauri commands backing the Workspace screen's "People" section (P84): the members of the
//! workspace, `[[users]]` in this machine's `config.toml`. The file syncs, so a member added here also
//! exists on the hub in the VPS once it gets there — the same way `node_cmds.rs` edits `[[nodes]]`.
//! A provisional password is generated here and handed back once; only its hash is written.
//!
//! No pairing key is asked: this is the owner's own machine, like the rest of its settings.

use serde::Serialize;
use warden_bootstrap::users::{add_user, generate_temp_password, remove_user, rename_user, reset_password};
use warden_bootstrap::{load_config_from_path, save_config};
use warden_server::people::user_info;
use warden_server::PairingStore;
use warden_server_protocol::protocol::UserInfoDto;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PeoplePayload {
    users: Vec<UserInfoDto>,
    /// The provisional password of the member just created or reset — shown once.
    temp_password: Option<String>,
}

fn config_path() -> Result<std::path::PathBuf, String> {
    warden_bootstrap::default_config_path().ok_or_else(|| "could not determine the OS config directory".to_string())
}

/// Loads the config, applies `change`, saves it and answers with the members.
fn change(apply: impl FnOnce(&mut warden_bootstrap::FileConfig) -> anyhow::Result<Option<String>>) -> Result<PeoplePayload, String> {
    let path = config_path()?;
    let mut config = load_config_from_path(&path, false).map_err(|e| format!("{e:#}"))?;
    let temp_password = apply(&mut config).map_err(|e| format!("{e:#}"))?;
    save_config(&path, &config).map_err(|e| format!("{e:#}"))?;
    Ok(PeoplePayload { users: config.users.iter().map(user_info).collect(), temp_password })
}

#[tauri::command]
pub fn list_people() -> Result<PeoplePayload, String> {
    let config = load_config_from_path(&config_path()?, false).map_err(|e| format!("{e:#}"))?;
    Ok(PeoplePayload { users: config.users.iter().map(user_info).collect(), temp_password: None })
}

#[tauri::command]
pub fn add_person(id: String, name: String) -> Result<PeoplePayload, String> {
    change(|config| {
        let password = generate_temp_password();
        add_user(config, &id, &name, &password)?;
        Ok(Some(password))
    })
}

#[tauri::command]
pub fn rename_person(id: String, name: String) -> Result<PeoplePayload, String> {
    change(|config| rename_user(config, &id, &name).map(|()| None))
}

#[tauri::command]
pub fn reset_person_password(id: String) -> Result<PeoplePayload, String> {
    change(|config| {
        let password = generate_temp_password();
        reset_password(config, &id, &password)?;
        Ok(Some(password))
    })
}

/// Removes the member and revokes their devices on this machine's hub. A hub elsewhere (the VPS)
/// turns them away once the file syncs there: a device whose member is gone can't sign in.
#[tauri::command]
pub fn remove_person(id: String) -> Result<PeoplePayload, String> {
    let payload = change(|config| remove_user(config, &id).map(|()| None))?;
    if let Some(devices) = warden_bootstrap::default_server_devices_path() {
        PairingStore::new(devices).revoke_user_devices(&id).map_err(|e| format!("{e:#}"))?;
    }
    Ok(payload)
}
