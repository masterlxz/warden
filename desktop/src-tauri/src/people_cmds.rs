//! Tauri commands backing the Workspace screen's "People" section (P84): the members of the
//! workspace, `[[users]]` in this machine's `config.toml`. The file syncs, so a member added here also
//! exists on the hub in the VPS once it gets there — the same way `node_cmds.rs` edits `[[nodes]]`.
//! A provisional password is generated here and handed back once; only its hash is written.
//!
//! No pairing key is asked: this is the owner's own machine, like the rest of its settings.

use serde::Serialize;
use warden_bootstrap::users::{add_user, create_invite, unlink_truthid, generate_temp_password, remove_space, remove_user, rename_user, reset_password, save_space, set_user_tools, set_user_workdirs, NodeFolder, SpaceConfig};
use warden_bootstrap::{load_config_from_path, save_config};
use warden_server::people::user_info;
use warden_server::PairingStore;
use warden_server_protocol::protocol::{NodeFolderDto, SpaceDto, UserInfoDto};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PeoplePayload {
    users: Vec<UserInfoDto>,
    /// The provisional password of the member just created or reset — shown once.
    temp_password: Option<String>,
    /// P84 fatia 5: the invite to link a TruthID just made — shown once.
    invite_code: Option<String>,
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
    let policy = warden_bootstrap::users::workspace_policy(&config);
    Ok(PeoplePayload { users: config.users.iter().map(|u| user_info(u, &config.agents, policy)).collect(), temp_password, invite_code: None })
}

#[tauri::command]
pub fn list_people() -> Result<PeoplePayload, String> {
    let config = load_config_from_path(&config_path()?, false).map_err(|e| format!("{e:#}"))?;
    let policy = warden_bootstrap::users::workspace_policy(&config);
    Ok(PeoplePayload { users: config.users.iter().map(|u| user_info(u, &config.agents, policy)).collect(), temp_password: None, invite_code: None })
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

/// P84 fatia 2: which tools a member may use; `None` is the safe default.
#[tauri::command]
pub fn set_person_tools(id: String, tools: Option<Vec<String>>) -> Result<PeoplePayload, String> {
    change(|config| set_user_tools(config, &id, tools).map(|()| None))
}

/// P102: the folders a member may pick as a working folder, on this machine and on nodes. Empty lists take them all away.
#[tauri::command]
pub fn set_person_workdirs(id: String, workdirs: Vec<String>, node_workdirs: Vec<NodeFolderDto>) -> Result<PeoplePayload, String> {
    let node_workdirs = node_workdirs.into_iter().map(|f| NodeFolder { node: f.node, path: f.path }).collect();
    change(|config| set_user_workdirs(config, &id, workdirs, node_workdirs).map(|()| None))
}

/// P84 fatia 5: an invite for a member to link their TruthID, handed back once (7 days, single use).
#[tauri::command]
pub fn invite_person(id: String) -> Result<PeoplePayload, String> {
    let mut code = None;
    let mut payload = change(|config| {
        code = Some(create_invite(config, &id, warden_server::people::unix_now())?);
        Ok(None)
    })?;
    payload.invite_code = code;
    Ok(payload)
}

/// Unties a member's TruthID and cancels an open invite.
#[tauri::command]
pub fn unlink_person_truthid(id: String) -> Result<PeoplePayload, String> {
    change(|config| unlink_truthid(config, &id).map(|()| None))
}

#[tauri::command]
pub fn reset_person_password(id: String) -> Result<PeoplePayload, String> {
    change(|config| {
        let password = generate_temp_password();
        reset_password(config, &id, &password)?;
        Ok(Some(password))
    })
}

/// Removes the member (and their own agents) and revokes their devices and Warden API keys on this
/// machine's hub. A hub elsewhere (the VPS) turns them away once the file syncs there: a device or
/// key whose member is gone can't get in.
#[tauri::command]
pub fn remove_person(id: String) -> Result<PeoplePayload, String> {
    let payload = change(|config| remove_user(config, &id).map(|()| None))?;
    if let Some(devices) = warden_bootstrap::default_server_devices_path() {
        PairingStore::new(devices).revoke_user_devices(&id).map_err(|e| format!("{e:#}"))?;
    }
    if let Some(keys) = warden_bootstrap::default_api_keys_path() {
        warden_server::api_keys::ApiKeyStore::new(keys).revoke_user_keys(&id).map_err(|e| format!("{e:#}"))?;
    }
    Ok(payload)
}

/// P84 fatia 3: the folders of your vault shared with members, `[[spaces]]` in the same file.
#[tauri::command]
pub fn list_shared_spaces() -> Result<Vec<SpaceDto>, String> {
    let config = load_config_from_path(&config_path()?, false).map_err(|e| format!("{e:#}"))?;
    Ok(config.spaces.iter().map(space_dto).collect())
}

/// Shares a folder (`original_id` absent) or changes who is in a space.
#[tauri::command]
pub fn save_shared_space(original_id: Option<String>, space: SpaceDto) -> Result<Vec<SpaceDto>, String> {
    change_spaces(|config| save_space(config, original_id.as_deref(), SpaceConfig { id: space.id, folder: space.folder, readers: space.readers, writers: space.writers }))
}

/// Stops sharing a folder; it stays in your vault.
#[tauri::command]
pub fn remove_shared_space(id: String) -> Result<Vec<SpaceDto>, String> {
    change_spaces(|config| remove_space(config, &id))
}

fn space_dto(space: &SpaceConfig) -> SpaceDto {
    SpaceDto { id: space.id.clone(), folder: space.folder.clone(), readers: space.readers.clone(), writers: space.writers.clone() }
}

fn change_spaces(apply: impl FnOnce(&mut warden_bootstrap::FileConfig) -> anyhow::Result<()>) -> Result<Vec<SpaceDto>, String> {
    let path = config_path()?;
    let mut config = load_config_from_path(&path, false).map_err(|e| format!("{e:#}"))?;
    apply(&mut config).map_err(|e| format!("{e:#}"))?;
    save_config(&path, &config).map_err(|e| format!("{e:#}"))?;
    Ok(config.spaces.iter().map(space_dto).collect())
}
