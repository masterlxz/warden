//! The workspace's members (P84) from a client: the web's People screen and `warden-server users`
//! do the same things. Every change asks for the pairing key again, with the same 1 s wait and the
//! same per-hub lock as a settings save. A provisional password is generated here and shown once,
//! in the reply; only its hash is written.
//!
//! `ChangePassword` is the member's own: checked against their current password instead of the key.

use warden_bootstrap::users::{add_user, change_password, generate_temp_password, remove_space, remove_user, rename_user, reset_password, save_space, set_user_tools, spaces_for, SpaceConfig};
use warden_bootstrap::{load_config_from_path, save_config};
use warden_server_protocol::protocol::SpaceDto;
use warden_server_protocol::ServerMessage;

use crate::api_keys::ApiKeyStore;
use crate::device_registry::PairingStore;
use crate::people::user_info;
use crate::settings::{keys_match, SettingsHost, WRONG_KEY_DELAY};

fn user_error(request_id: u64, message: String, auth_rejected: bool) -> ServerMessage {
    ServerMessage::UserError { request_id, message, auth_rejected }
}

const NO_SETTINGS: &str = "this hub has no settings file, so it has no people besides its owner";

fn list(settings: &dyn SettingsHost, request_id: u64, temp_password: Option<String>) -> anyhow::Result<ServerMessage> {
    let config = load_config_from_path(&settings.config_path(), false)?;
    Ok(ServerMessage::UserList { request_id, users: config.users.iter().map(|u| user_info(u, &config.agents)).collect(), temp_password })
}

/// Answers `ListUsers` (the root's connection only — `people::member_refusal` stops a member first).
pub fn handle_list_users(settings: Option<&dyn SettingsHost>, request_id: u64) -> ServerMessage {
    match settings {
        Some(settings) => list(settings, request_id, None).unwrap_or_else(|err| user_error(request_id, format!("{err:#}"), false)),
        None => user_error(request_id, NO_SETTINGS.to_string(), false),
    }
}

/// What `SaveUser`/`ResetPassword`/`RemoveUser` asks for.
pub enum UserChange {
    Create { id: String, name: String },
    Rename { id: String, name: String },
    ResetPassword { id: String },
    Remove { id: String },
    /// Fatia 2: the tools they may use; `None` is the safe default.
    SetTools { id: String, tools: Option<Vec<String>> },
}

/// Answers a change with the updated `UserList` — carrying the provisional password after a create
/// or a reset. Removing a member also revokes every device and Warden API key of theirs (closing
/// their connections) and takes their own agents along.
#[allow(clippy::too_many_arguments)]
pub async fn handle_user_change(
    settings: Option<&dyn SettingsHost>,
    pairing: &PairingStore,
    api_keys: Option<&ApiKeyStore>,
    lock: &tokio::sync::Mutex<()>,
    auth_key: &str,
    request_id: u64,
    pairing_key: &str,
    change: UserChange,
) -> ServerMessage {
    let Some(settings) = settings else { return user_error(request_id, NO_SETTINGS.to_string(), false) };
    let _serialized = lock.lock().await;
    if !keys_match(pairing_key, auth_key) {
        tokio::time::sleep(WRONG_KEY_DELAY).await;
        return user_error(request_id, "wrong pairing key".to_string(), true);
    }
    let config_path = settings.config_path();
    let result = (|| -> anyhow::Result<Option<String>> {
        let mut config = load_config_from_path(&config_path, false)?;
        let mut temp = None;
        let mut removed = None;
        match change {
            UserChange::Create { id, name } => {
                let password = generate_temp_password();
                add_user(&mut config, &id, &name, &password)?;
                temp = Some(password);
            }
            UserChange::Rename { id, name } => rename_user(&mut config, &id, &name)?,
            UserChange::ResetPassword { id } => {
                let password = generate_temp_password();
                reset_password(&mut config, &id, &password)?;
                temp = Some(password);
            }
            UserChange::Remove { id } => {
                remove_user(&mut config, &id)?;
                removed = Some(id);
            }
            UserChange::SetTools { id, tools } => set_user_tools(&mut config, &id, tools)?,
        }
        save_config(&config_path, &config)?;
        if let Some(id) = removed {
            pairing.revoke_user_devices(&id)?;
            if let Some(keys) = api_keys {
                keys.revoke_user_keys(&id)?;
            }
        }
        Ok(temp)
    })();
    match result.and_then(|temp| list(settings, request_id, temp)) {
        Ok(reply) => reply,
        Err(err) => user_error(request_id, format!("{err:#}"), false),
    }
}

/// Answers a member's `ChangePassword`. A wrong current password waits like a wrong key does.
pub async fn handle_change_password(settings: Option<&dyn SettingsHost>, lock: &tokio::sync::Mutex<()>, user: &str, request_id: u64, old: &str, new: &str) -> ServerMessage {
    let Some(settings) = settings else { return user_error(request_id, NO_SETTINGS.to_string(), false) };
    let _serialized = lock.lock().await;
    let config_path = settings.config_path();
    let result = (|| -> anyhow::Result<()> {
        let mut config = load_config_from_path(&config_path, false)?;
        change_password(&mut config, user, old, new)?;
        save_config(&config_path, &config)
    })();
    match result {
        Ok(()) => ServerMessage::PasswordChanged { request_id },
        Err(err) => {
            let message = format!("{err:#}");
            let wrong = message.contains("current password is wrong");
            if wrong {
                tokio::time::sleep(WRONG_KEY_DELAY).await;
            }
            user_error(request_id, message, wrong)
        }
    }
}

fn space_dto(space: &SpaceConfig) -> SpaceDto {
    SpaceDto { id: space.id.clone(), folder: space.folder.clone(), readers: space.readers.clone(), writers: space.writers.clone() }
}

fn space_list(settings: &dyn SettingsHost, member: Option<&str>, request_id: u64) -> anyhow::Result<ServerMessage> {
    let config = load_config_from_path(&settings.config_path(), false)?;
    let spaces = match member {
        None => config.spaces.iter().map(space_dto).collect(),
        // A member sees the spaces they're in, not who else is.
        Some(member) => spaces_for(&config.spaces, member)
            .into_iter()
            .map(|(space, writable)| SpaceDto {
                id: space.id.clone(),
                folder: format!("{}/{}", warden_core::memory::MOUNTS_DIR, space.id),
                readers: if writable { Vec::new() } else { vec![member.to_string()] },
                writers: if writable { vec![member.to_string()] } else { Vec::new() },
            })
            .collect(),
    };
    Ok(ServerMessage::SpaceList { request_id, spaces })
}

/// Answers `ListSpaces` — every space for the owner (`member` = `None`), the member's own for them.
pub fn handle_list_spaces(settings: Option<&dyn SettingsHost>, member: Option<&str>, request_id: u64) -> ServerMessage {
    match settings {
        Some(settings) => space_list(settings, member, request_id).unwrap_or_else(|err| user_error(request_id, format!("{err:#}"), false)),
        None => user_error(request_id, NO_SETTINGS.to_string(), false),
    }
}

/// What `SaveSpace`/`DeleteSpace` asks for.
pub enum SpaceChange {
    Save { original_id: Option<String>, space: SpaceDto },
    Delete { id: String },
}

/// Answers a space change with the updated `SpaceList`, after checking the pairing key like every
/// people change.
pub async fn handle_space_change(
    settings: Option<&dyn SettingsHost>,
    lock: &tokio::sync::Mutex<()>,
    auth_key: &str,
    request_id: u64,
    pairing_key: &str,
    change: SpaceChange,
) -> ServerMessage {
    let Some(settings) = settings else { return user_error(request_id, NO_SETTINGS.to_string(), false) };
    let _serialized = lock.lock().await;
    if !keys_match(pairing_key, auth_key) {
        tokio::time::sleep(WRONG_KEY_DELAY).await;
        return user_error(request_id, "wrong pairing key".to_string(), true);
    }
    let config_path = settings.config_path();
    let result = (|| -> anyhow::Result<()> {
        let mut config = load_config_from_path(&config_path, false)?;
        match change {
            SpaceChange::Save { original_id, space } => {
                let space = SpaceConfig { id: space.id, folder: space.folder, readers: space.readers, writers: space.writers };
                save_space(&mut config, original_id.as_deref(), space)?;
            }
            SpaceChange::Delete { id } => remove_space(&mut config, &id)?,
        }
        save_config(&config_path, &config)
    })();
    match result.and_then(|()| space_list(settings, None, request_id)) {
        Ok(reply) => reply,
        Err(err) => user_error(request_id, format!("{err:#}"), false),
    }
}
