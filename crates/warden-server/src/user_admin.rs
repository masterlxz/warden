//! The workspace's members (P84) from a client: the web's People screen and `warden-server users`
//! do the same things. Every change asks for the pairing key again, with the same 1 s wait and the
//! same per-hub lock as a settings save. A provisional password is generated here and shown once,
//! in the reply; only its hash is written.
//!
//! `ChangePassword` is the member's own: checked against their current password instead of the key.

use std::path::{Path, PathBuf};

use warden_bootstrap::member_crypto::{self, MemberKey};
use warden_bootstrap::recovery::RecoveryPolicy;
use warden_bootstrap::users::{
    ack_recovery_notices, add_user, create_invite, redeem_invite, unlink_truthid, TruthIdLink, change_password_with, enable_encryption, generate_temp_password, open_key, recover_member, regenerate_recovery_code, remove_space, remove_user, rename_user,
    reset_password, restore_user, save_space, set_learning_opt_out, set_recovery_policy, set_user_tools, spaces_for, sync_recovery_policy, user_conversations_dir, workspace_policy, PasswordChange, SpaceConfig,
};
use warden_bootstrap::{load_config_from_path, save_config};
use warden_server_protocol::protocol::{RemovedUserDto, SpaceDto};
use warden_server_protocol::ServerMessage;

use crate::api_keys::ApiKeyStore;
use crate::device_registry::PairingStore;
use crate::people::{unix_now, user_info};
use crate::settings::{keys_match, SettingsHost, WRONG_KEY_DELAY};

fn user_error(request_id: u64, message: String, auth_rejected: bool) -> ServerMessage {
    ServerMessage::UserError { request_id, message, auth_rejected }
}

const NO_SETTINGS: &str = "this hub has no settings file, so it has no people besides its owner";

fn list(settings: &dyn SettingsHost, request_id: u64, temp_password: Option<String>, invite_code: Option<String>) -> anyhow::Result<ServerMessage> {
    let config = load_config_from_path(&settings.config_path(), false)?;
    let policy = workspace_policy(&config);
    Ok(ServerMessage::UserList {
        request_id,
        users: config.users.iter().map(|u| user_info(u, &config.agents, policy)).collect(),
        temp_password,
        invite_code,
        recovery_policy: policy.as_str().to_string(),
        removed: config.removed_users.iter().map(|u| RemovedUserDto { id: u.id.clone(), name: u.name.clone() }).collect(),
    })
}

/// Answers `ListUsers` (the root's connection only — `people::member_refusal` stops a member first).
pub fn handle_list_users(settings: Option<&dyn SettingsHost>, request_id: u64) -> ServerMessage {
    match settings {
        Some(settings) => list(settings, request_id, None, None).unwrap_or_else(|err| user_error(request_id, format!("{err:#}"), false)),
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
    /// Fatia 4: brings back a removed member whose encrypted data was kept.
    Restore { id: String },
    /// Fatia 5: an invite to link a TruthID, shown once.
    Invite { id: String },
    /// Fatia 5: unties a member's TruthID and cancels an open invite.
    UnlinkTruthId { id: String },
    /// Fatia 4 parte B: the owner opens a member's data with the workspace's recovery key.
    Recover { id: String, recovery_key: String, code: Option<String> },
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
    dirs: Option<DataDirs<'_>>,
    change: UserChange,
) -> ServerMessage {
    let Some(settings) = settings else { return user_error(request_id, NO_SETTINGS.to_string(), false) };
    let _serialized = lock.lock().await;
    if !keys_match(pairing_key, auth_key) {
        tokio::time::sleep(WRONG_KEY_DELAY).await;
        return user_error(request_id, "wrong pairing key".to_string(), true);
    }
    let config_path = settings.config_path();
    let result = (|| -> anyhow::Result<(Option<String>, Option<String>)> {
        let mut config = load_config_from_path(&config_path, false)?;
        let mut temp = None;
        let mut invite = None;
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
            UserChange::Restore { id } => restore_user(&mut config, &id)?,
            UserChange::Invite { id } => invite = Some(create_invite(&mut config, &id, unix_now())?),
            UserChange::UnlinkTruthId { id } => unlink_truthid(&mut config, &id)?,
            UserChange::Recover { id, recovery_key, code } => {
                let recovered = recover_member(&mut config, &id, &recovery_key, code.as_deref())?;
                eprintln!("warden-server: the owner recovered '{id}' under the {} policy", recovered.kind.as_str());
                temp = Some(recovered.temp_password);
            }
        }
        save_config(&config_path, &config)?;
        if let Some(id) = removed {
            if let Some(dirs) = dirs {
                dirs.lock(&id);
            }
            pairing.revoke_user_devices(&id)?;
            if let Some(keys) = api_keys {
                keys.revoke_user_keys(&id)?;
            }
        }
        Ok((temp, invite))
    })();
    match result.and_then(|(temp, invite)| list(settings, request_id, temp, invite)) {
        Ok(reply) => reply,
        Err(err) => user_error(request_id, format!("{err:#}"), false),
    }
}

/// Where the hub keeps people's data, to open a member's (P84 fatia 4).
#[derive(Clone, Copy)]
pub struct DataDirs<'a> {
    /// Every member's folder (`warden_bootstrap::users::default_users_dir`).
    pub users_dir: &'a Path,
    /// Every person's conversations.
    pub conversations_root: &'a Path,
}

impl DataDirs<'_> {
    fn of(&self, id: &str) -> (PathBuf, PathBuf) {
        (self.users_dir.join(id), user_conversations_dir(self.conversations_root, id))
    }

    /// The hub forgets a member's key: a person that was taken out of the workspace.
    fn lock(&self, id: &str) {
        let (user_dir, conversations) = self.of(id);
        member_crypto::lock(&[&user_dir, &conversations]);
    }

    /// Puts a member's key to use: encrypts what they have on disk the first time (or after a run
    /// cut short), and from then on holds the key so their vault and conversations open.
    async fn use_key(&self, id: &str, key: MemberKey) -> anyhow::Result<()> {
        let (user_dir, conversations) = self.of(id);
        tokio::task::spawn_blocking(move || member_crypto::open_member_data(&user_dir, &conversations, &key))
            .await
            .map_err(|err| anyhow::anyhow!("the encryption of their data was interrupted: {err}"))?
    }
}

/// Signing in with the password is the one moment the hub can open a member's key, so it does it
/// here, before the connection gets a vault: it opens the key (and holds it), or — for a member
/// from before fatia 4 who's on their own password — creates one and encrypts what they have.
/// Returns the recovery code when that turned encryption on: shown to them once. `Ok(None)` when
/// there's nothing to open (no key yet, or the owner reset the password and the recovery code has
/// to open it first).
pub async fn open_member_data_at_sign_in(
    settings: Option<&dyn SettingsHost>,
    lock: &tokio::sync::Mutex<()>,
    dirs: DataDirs<'_>,
    id: &str,
    password: &str,
    can_show_code: bool,
) -> anyhow::Result<Option<String>> {
    let Some(settings) = settings else { return Ok(None) };
    let (key, new_code) = {
        let _serialized = lock.lock().await;
        let config_path = settings.config_path();
        let mut config = load_config_from_path(&config_path, false)?;
        let user = config.users.iter().find(|u| u.id == id).ok_or_else(|| anyhow::anyhow!("no user named '{id}'"))?;
        match open_key(user, password)? {
            Some(key) => {
                // The workspace's recovery policy may have changed since their data last followed it: with
                // the key open, bring it in step (a weaker policy waits for their yes, `AcceptRecoveryPolicy`).
                let before = user.key.clone();
                let outcome = sync_recovery_policy(&mut config, id, &key, false, can_show_code)?;
                if config.users.iter().find(|u| u.id == id).map(|u| &u.key) != Some(&before) {
                    save_config(&config_path, &config)?;
                }
                (key, outcome.new_code)
            }
            // A client that can't show the code they'd have to keep leaves the data as it is.
            None if !can_show_code => return Ok(None),
            None => match enable_encryption(&mut config, id, password)? {
                Some((key, code)) => {
                    save_config(&config_path, &config)?;
                    (key, Some(code))
                }
                None => return Ok(None),
            },
        }
    };
    dirs.use_key(id, key).await?;
    Ok(new_code)
}

/// Answers a member's `ChangePassword`. A wrong current password waits like a wrong key does. It's
/// also where a new member's data key is born (from their own password, if `can_show_code`: the
/// client can show the recovery code that comes with it), and where an owner's reset is undone with
/// the recovery code.
#[allow(clippy::too_many_arguments)]
pub async fn handle_change_password(
    settings: Option<&dyn SettingsHost>,
    lock: &tokio::sync::Mutex<()>,
    dirs: Option<DataDirs<'_>>,
    user: &str,
    request_id: u64,
    old: &str,
    new: &str,
    recovery_code: Option<&str>,
    can_show_code: bool,
) -> ServerMessage {
    let Some(settings) = settings else { return user_error(request_id, NO_SETTINGS.to_string(), false) };
    let _serialized = lock.lock().await;
    let config_path = settings.config_path();
    let result = (|| -> anyhow::Result<PasswordChange> {
        let mut config = load_config_from_path(&config_path, false)?;
        let change = change_password_with(&mut config, user, old, new, recovery_code, can_show_code)?;
        save_config(&config_path, &config)?;
        Ok(change)
    })();
    match result {
        Ok(change) => {
            if let (Some(key), Some(dirs)) = (change.key, dirs) {
                // The password is changed either way; if this fails, the next sign-in finishes it.
                if let Err(err) = dirs.use_key(user, key).await {
                    return user_error(request_id, format!("your password changed, but your data couldn't be encrypted yet: {err:#} — sign in again to finish"), false);
                }
            }
            ServerMessage::PasswordChanged { request_id, recovery_code: change.new_recovery_code }
        }
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

/// Answers a member's `RegenerateRecoveryCode`: a new code, shown once, and the old one stops
/// working. A wrong password waits like it does anywhere else.
pub async fn handle_regenerate_recovery_code(settings: Option<&dyn SettingsHost>, lock: &tokio::sync::Mutex<()>, user: &str, request_id: u64, password: &str) -> ServerMessage {
    let Some(settings) = settings else { return user_error(request_id, NO_SETTINGS.to_string(), false) };
    let _serialized = lock.lock().await;
    let config_path = settings.config_path();
    let result = (|| -> anyhow::Result<String> {
        let mut config = load_config_from_path(&config_path, false)?;
        let code = regenerate_recovery_code(&mut config, user, password)?;
        save_config(&config_path, &config)?;
        Ok(code)
    })();
    match result {
        Ok(code) => ServerMessage::RecoveryCode { request_id, code },
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

/// Answers `RedeemInvite` (P84 fatia 5): the member, already signed in, links the TruthID `username`
/// with the owner's invite `code`, which has to be theirs. The registry on Base is asked first, with
/// no lock held, since that's a network call; then the invite is checked and used under the lock.
/// Linking proves nothing about who controls that TruthID — signing in with it will (a later slice).
pub async fn handle_redeem_invite(settings: Option<&dyn SettingsHost>, lock: &tokio::sync::Mutex<()>, user: &str, request_id: u64, code: &str, username: &str) -> ServerMessage {
    let Some(settings) = settings else { return user_error(request_id, NO_SETTINGS.to_string(), false) };
    let config_path = settings.config_path();
    let result = async {
        let username = username.trim().trim_start_matches('@').to_string();
        anyhow::ensure!(!username.is_empty(), "say your TruthID username");
        let config = load_config_from_path(&config_path, false)?;
        // Checked before the network call, so a stranger's guess costs the owner nothing.
        let invited = warden_bootstrap::users::check_invite(&config.users, code, unix_now())?;
        anyhow::ensure!(invited.id == user, "that invite is for someone else");
        let rpc_url = config.truthid_rpc_url.clone().unwrap_or_else(|| config.truthid_network.default_rpc_url().to_string());
        let identity = warden_truthid::identity::resolve_identity(&rpc_url, config.truthid_network, &username)
            .await?
            .ok_or_else(|| anyhow::anyhow!("there's no TruthID named '{username}'"))?;
        let _serialized = lock.lock().await;
        let mut config = load_config_from_path(&config_path, false)?;
        let link = TruthIdLink { username: identity.username.clone(), identity_id: identity.id, linked_at: unix_now() };
        redeem_invite(&mut config, code, link, unix_now())?;
        save_config(&config_path, &config)?;
        Ok(identity.username)
    }
    .await;
    match result {
        Ok(username) => ServerMessage::TruthIdLinked { request_id, username },
        Err(err) => user_error(request_id, format!("{err:#}"), false),
    }
}

/// Answers the member's yes to the workspace's recovery policy (`AcceptRecoveryPolicy`), after a change
/// to a weaker one waited for it. Their password opens their key, so it's checked like any other: a
/// wrong one waits. Entering or leaving `consent` makes a new recovery code, shown once — from a client
/// that can show it (`can_show_code`).
pub async fn handle_accept_recovery_policy(settings: Option<&dyn SettingsHost>, lock: &tokio::sync::Mutex<()>, user: &str, request_id: u64, password: &str, can_show_code: bool) -> ServerMessage {
    let Some(settings) = settings else { return user_error(request_id, NO_SETTINGS.to_string(), false) };
    let _serialized = lock.lock().await;
    let config_path = settings.config_path();
    let result = (|| -> anyhow::Result<Option<String>> {
        let mut config = load_config_from_path(&config_path, false)?;
        let found = config.users.iter().find(|u| u.id == user).ok_or_else(|| anyhow::anyhow!("no user named '{user}'"))?;
        let key = open_key(found, password)?.ok_or_else(|| anyhow::anyhow!("your data isn't encrypted with a key of yours that opens with your password right now"))?;
        let outcome = sync_recovery_policy(&mut config, user, &key, true, can_show_code)?;
        anyhow::ensure!(!outcome.pending, "this change makes a new recovery code, which this app can't show — sign in on the web to accept it");
        save_config(&config_path, &config)?;
        Ok(outcome.new_code)
    })();
    match result {
        Ok(recovery_code) => ServerMessage::RecoveryPolicyAccepted { request_id, recovery_code },
        Err(err) => {
            let message = format!("{err:#}");
            let wrong = message.contains("password doesn't open");
            if wrong {
                tokio::time::sleep(WRONG_KEY_DELAY).await;
            }
            user_error(request_id, message, wrong)
        }
    }
}

/// Answers `AckRecoveryNotices`: the member has seen the recoveries the owner made.
pub async fn handle_ack_recovery_notices(settings: Option<&dyn SettingsHost>, lock: &tokio::sync::Mutex<()>, user: &str, request_id: u64) -> ServerMessage {
    let Some(settings) = settings else { return user_error(request_id, NO_SETTINGS.to_string(), false) };
    let _serialized = lock.lock().await;
    let config_path = settings.config_path();
    let result = (|| -> anyhow::Result<()> {
        let mut config = load_config_from_path(&config_path, false)?;
        ack_recovery_notices(&mut config, user)?;
        save_config(&config_path, &config)
    })();
    match result {
        Ok(()) => ServerMessage::RecoveryNoticesAcked { request_id },
        Err(err) => user_error(request_id, format!("{err:#}"), false),
    }
}

/// Answers `SetLearning`: a member's own choice about the assistant learning from their conversations.
pub async fn handle_set_learning(settings: Option<&dyn SettingsHost>, lock: &tokio::sync::Mutex<()>, user: &str, request_id: u64, enabled: bool) -> ServerMessage {
    let Some(settings) = settings else { return user_error(request_id, NO_SETTINGS.to_string(), false) };
    let _serialized = lock.lock().await;
    let config_path = settings.config_path();
    let result = (|| -> anyhow::Result<()> {
        let mut config = load_config_from_path(&config_path, false)?;
        set_learning_opt_out(&mut config, user, !enabled)?;
        save_config(&config_path, &config)
    })();
    match result {
        Ok(()) => ServerMessage::LearningSet { request_id },
        Err(err) => user_error(request_id, format!("{err:#}"), false),
    }
}

/// Answers `SetRecoveryPolicy` (the owner's): the pairing key is checked like every people change. A
/// policy that needs the owner's recovery key makes one if there is none, and hands back its private
/// half once. Members' data follows at their next sign-in.
pub async fn handle_set_recovery_policy(
    settings: Option<&dyn SettingsHost>,
    lock: &tokio::sync::Mutex<()>,
    auth_key: &str,
    request_id: u64,
    pairing_key: &str,
    policy: &str,
    new_key: bool,
) -> ServerMessage {
    let Some(settings) = settings else { return user_error(request_id, NO_SETTINGS.to_string(), false) };
    let _serialized = lock.lock().await;
    if !keys_match(pairing_key, auth_key) {
        tokio::time::sleep(WRONG_KEY_DELAY).await;
        return user_error(request_id, "wrong pairing key".to_string(), true);
    }
    let config_path = settings.config_path();
    let result = (|| -> anyhow::Result<(RecoveryPolicy, Option<String>)> {
        let policy = RecoveryPolicy::parse(policy)?;
        let mut config = load_config_from_path(&config_path, false)?;
        let secret = set_recovery_policy(&mut config, policy, new_key);
        save_config(&config_path, &config)?;
        Ok((policy, secret))
    })();
    match result {
        Ok((policy, secret)) => ServerMessage::RecoveryPolicy { request_id, policy: policy.as_str().to_string(), secret },
        Err(err) => user_error(request_id, format!("{err:#}"), false),
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
