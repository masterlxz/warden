//! The Warden API's keys (P12) from a client: the same list/create/revoke as `warden-server
//! api-keys`, for the web's settings. Listing is open to any paired device, like the device list;
//! creating or revoking asks for the pairing key again, with the same 1 s wait and the same per-hub
//! lock as a settings save, so they all share one guessing rate.
//!
//! P84 fatia 2: a member has keys of their own. They see and change only theirs, confirming with
//! their own password where the owner gives the pairing key, and a key they make can only speak as
//! an agent they see.

use warden_server_protocol::protocol::ApiKeyDto;
use warden_server_protocol::ServerMessage;

use crate::api_keys::{ApiKey, ApiKeyStore};
use crate::settings::{keys_match, SettingsHost, WRONG_KEY_DELAY};

pub fn dto(key: ApiKey) -> ApiKeyDto {
    ApiKeyDto {
        id: key.id,
        name: key.name,
        shown: key.shown,
        created_at_ms: key.created_at_ms,
        last_used_at_ms: key.last_used_at_ms,
        agent_id: key.agent_id,
        user: key.user,
    }
}

/// A key can only be bound to an agent the hub's config has — one `member` sees, for a member's key
/// (`None`: the owner's agents). Blank is a general key.
pub fn check_agent_exists(config_path: &std::path::Path, agent_id: Option<&str>, member: Option<&str>) -> anyhow::Result<()> {
    let Some(agent_id) = agent_id.map(str::trim).filter(|id| !id.is_empty()) else {
        return Ok(());
    };
    let config = warden_bootstrap::load_config_from_path(config_path, false)?;
    anyhow::ensure!(
        config.agents.iter().any(|a| a.id == agent_id && warden_bootstrap::users::agent_visible_to(a, member)),
        "there is no agent '{agent_id}'"
    );
    Ok(())
}

fn api_key_error(request_id: u64, message: String, auth_rejected: bool) -> ServerMessage {
    ServerMessage::ApiKeyError { request_id, message, auth_rejected }
}

/// Every key for the owner (`member` = `None`), a member's own for them.
fn list(store: &ApiKeyStore, member: Option<&str>) -> anyhow::Result<Vec<ApiKeyDto>> {
    Ok(store.list()?.into_iter().filter(|k| member.is_none() || k.user.as_deref() == member).map(dto).collect())
}

/// Answers `ListApiKeys`. `store` is `None` on a hub without the API; `member` is who asks (`None`:
/// the owner).
pub fn handle_list_api_keys(store: Option<&ApiKeyStore>, member: Option<&str>, request_id: u64) -> ServerMessage {
    let Some(store) = store else {
        return api_key_error(request_id, "this hub doesn't offer the Warden API".to_string(), false);
    };
    match list(store, member) {
        Ok(keys) => ServerMessage::ApiKeyList { request_id, keys },
        Err(err) => api_key_error(request_id, format!("failed to read the API keys: {err:#}"), false),
    }
}

/// What `CreateApiKey`/`RevokeApiKey` asks for.
pub enum ApiKeyChange {
    Create { name: String, agent_id: Option<String> },
    Revoke { id: String },
}

/// Whether `proof` is the member's own password (P84) — the member's stand-in for the pairing key.
fn member_password_ok(settings: Option<&dyn SettingsHost>, member: &str, proof: &str) -> bool {
    let Some(host) = settings else { return false };
    warden_bootstrap::load_config_from_path(&host.config_path(), false)
        .ok()
        .and_then(|config| config.users.into_iter().find(|u| u.id == member))
        .is_some_and(|user| warden_bootstrap::users::verify_password(&user.password_hash, proof))
}

/// Answers `CreateApiKey` (with `ApiKeyCreated`) or `RevokeApiKey` (with the updated list).
/// `settings` is where the agents are, for a key bound to one; without it only general keys can be
/// made. `member` is who asks (`None`: the owner): `proof` is then their password, not the pairing
/// key, and only their own keys are theirs to see and revoke.
#[allow(clippy::too_many_arguments)]
pub async fn handle_api_key_change(
    store: Option<&ApiKeyStore>,
    settings: Option<&dyn SettingsHost>,
    lock: &tokio::sync::Mutex<()>,
    auth_key: &str,
    member: Option<&str>,
    request_id: u64,
    proof: &str,
    change: ApiKeyChange,
) -> ServerMessage {
    let Some(store) = store else {
        return api_key_error(request_id, "this hub doesn't offer the Warden API".to_string(), false);
    };
    let _serialized = lock.lock().await;
    let proven = match member {
        None => keys_match(proof, auth_key),
        Some(member) => member_password_ok(settings, member, proof),
    };
    if !proven {
        tokio::time::sleep(WRONG_KEY_DELAY).await;
        let what = if member.is_some() { "wrong password" } else { "wrong pairing key" };
        return api_key_error(request_id, what.to_string(), true);
    }
    let result = match change {
        ApiKeyChange::Create { name, agent_id } => {
            let checked = match (agent_id.as_deref().map(str::trim).filter(|id| !id.is_empty()), settings) {
                (None, _) => Ok(()),
                (Some(_), None) => Err(anyhow::anyhow!("this hub has no settings file, so it has no agents to bind a key to")),
                (Some(_), Some(host)) => check_agent_exists(&host.config_path(), agent_id.as_deref(), member),
            };
            checked
                .and_then(|()| store.create_for(&name, agent_id.as_deref(), member))
                .and_then(|created| Ok(ServerMessage::ApiKeyCreated { request_id, key: created.key, keys: list(store, member)? }))
        }
        ApiKeyChange::Revoke { id } => {
            let theirs = store.list().map(|keys| keys.iter().any(|k| k.id == id && (member.is_none() || k.user.as_deref() == member)));
            match theirs {
                Ok(false) => Err(anyhow::anyhow!("no API key with id '{id}'")),
                Err(err) => Err(err),
                Ok(true) => match store.revoke(&id) {
                    Ok(true) => list(store, member).map(|keys| ServerMessage::ApiKeyList { request_id, keys }),
                    Ok(false) => Err(anyhow::anyhow!("no API key with id '{id}'")),
                    Err(err) => Err(err),
                },
            }
        }
    };
    result.unwrap_or_else(|err| api_key_error(request_id, format!("{err:#}"), false))
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "pairing-key-0123456789-0123456789";

    fn store(name: &str) -> ApiKeyStore {
        ApiKeyStore::new(std::env::temp_dir().join(format!("warden-hub-api-keys-{name}-{}", std::process::id())).join("api_keys.json"))
    }

    #[tokio::test]
    async fn creating_needs_the_pairing_key_and_shows_the_key_once() {
        let store = store("create");
        let lock = tokio::sync::Mutex::new(());
        let started = std::time::Instant::now();
        let reply = handle_api_key_change(Some(&store), None, &lock, KEY, None, 1, "wrong", ApiKeyChange::Create { name: "n8n".into(), agent_id: None }).await;
        assert!(matches!(reply, ServerMessage::ApiKeyError { auth_rejected: true, .. }), "{reply:?}");
        assert!(started.elapsed() >= WRONG_KEY_DELAY);
        assert!(store.list().unwrap().is_empty(), "nothing was created");

        let reply = handle_api_key_change(Some(&store), None, &lock, KEY, None, 2, KEY, ApiKeyChange::Create { name: "n8n".into(), agent_id: None }).await;
        let ServerMessage::ApiKeyCreated { key, keys, .. } = reply else { panic!("{reply:?}") };
        assert!(store.authenticate(&key).unwrap().is_some());
        let ServerMessage::ApiKeyList { keys: listed, .. } = handle_list_api_keys(Some(&store), None, 3) else { panic!() };
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, keys[0].id);

        let reply = handle_api_key_change(Some(&store), None, &lock, KEY, None, 4, KEY, ApiKeyChange::Revoke { id: keys[0].id.clone() }).await;
        assert!(matches!(reply, ServerMessage::ApiKeyList { ref keys, .. } if keys.is_empty()), "{reply:?}");
        assert!(store.authenticate(&key).unwrap().is_none());
        let reply = handle_api_key_change(Some(&store), None, &lock, KEY, None, 5, KEY, ApiKeyChange::Revoke { id: "ghost".into() }).await;
        assert!(matches!(reply, ServerMessage::ApiKeyError { auth_rejected: false, .. }));
    }

    struct Host(std::path::PathBuf);

    #[async_trait::async_trait]
    impl SettingsHost for Host {
        fn config_path(&self) -> std::path::PathBuf {
            self.0.clone()
        }

        async fn build(&self) -> anyhow::Result<warden_core::orchestrator::Orchestrator> {
            anyhow::bail!("not used")
        }
    }

    #[tokio::test]
    async fn a_key_is_only_bound_to_an_agent_that_exists() {
        let store = store("bound");
        let config = std::env::temp_dir().join(format!("warden-hub-api-keys-config-{}.toml", std::process::id()));
        std::fs::write(&config, "[[agents]]\nid = \"poet\"\npersona = \"p\"\n").unwrap();
        let host = Host(config);
        let lock = tokio::sync::Mutex::new(());
        let create = |agent: &str| ApiKeyChange::Create { name: format!("for-{agent}"), agent_id: Some(agent.to_string()) };

        let reply = handle_api_key_change(Some(&store), Some(&host), &lock, KEY, None, 1, KEY, create("ghost")).await;
        assert!(matches!(reply, ServerMessage::ApiKeyError { auth_rejected: false, ref message, .. } if message.contains("no agent 'ghost'")), "{reply:?}");
        let reply = handle_api_key_change(Some(&store), None, &lock, KEY, None, 2, KEY, create("poet")).await;
        assert!(matches!(reply, ServerMessage::ApiKeyError { .. }), "no settings, no agents: {reply:?}");
        assert!(store.list().unwrap().is_empty(), "nothing was created");

        let reply = handle_api_key_change(Some(&store), Some(&host), &lock, KEY, None, 3, KEY, create("poet")).await;
        let ServerMessage::ApiKeyCreated { keys, .. } = reply else { panic!("{reply:?}") };
        assert_eq!(keys[0].agent_id.as_deref(), Some("poet"));
    }
}
