//! The workspace's members (P84) from a client: the web's People screen and `warden-server users`
//! do the same things. Every change asks for the pairing key again, with the same 1 s wait and the
//! same per-hub lock as a settings save. A provisional password is generated here and shown once,
//! in the reply; only its hash is written.
//!
//! `ChangePassword` is the member's own: checked against their current password instead of the key.

use warden_bootstrap::users::{add_user, change_password, generate_temp_password, remove_user, rename_user, reset_password};
use warden_bootstrap::{load_config_from_path, save_config};
use warden_server_protocol::ServerMessage;

use crate::device_registry::PairingStore;
use crate::people::user_info;
use crate::settings::{keys_match, SettingsHost, WRONG_KEY_DELAY};

fn user_error(request_id: u64, message: String, auth_rejected: bool) -> ServerMessage {
    ServerMessage::UserError { request_id, message, auth_rejected }
}

const NO_SETTINGS: &str = "this hub has no settings file, so it has no people besides its owner";

fn list(settings: &dyn SettingsHost, request_id: u64, temp_password: Option<String>) -> anyhow::Result<ServerMessage> {
    let config = load_config_from_path(&settings.config_path(), false)?;
    Ok(ServerMessage::UserList { request_id, users: config.users.iter().map(user_info).collect(), temp_password })
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
}

/// Answers a change with the updated `UserList` — carrying the provisional password after a create
/// or a reset. Removing a member also revokes every device of theirs, which closes its connection.
pub async fn handle_user_change(
    settings: Option<&dyn SettingsHost>,
    pairing: &PairingStore,
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
        }
        save_config(&config_path, &config)?;
        if let Some(id) = removed {
            pairing.revoke_user_devices(&id)?;
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
