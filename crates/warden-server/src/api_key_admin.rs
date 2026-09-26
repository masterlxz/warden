//! The Warden API's keys (P12) from a client: the same list/create/revoke as `warden-server
//! api-keys`, for the web's settings. Listing is open to any paired device, like the device list;
//! creating or revoking asks for the pairing key again, with the same 1 s wait and the same per-hub
//! lock as a settings save, so they all share one guessing rate.

use warden_server_protocol::protocol::ApiKeyDto;
use warden_server_protocol::ServerMessage;

use crate::api_keys::{ApiKey, ApiKeyStore};
use crate::settings::{keys_match, WRONG_KEY_DELAY};

pub fn dto(key: ApiKey) -> ApiKeyDto {
    ApiKeyDto { id: key.id, name: key.name, shown: key.shown, created_at_ms: key.created_at_ms, last_used_at_ms: key.last_used_at_ms }
}

fn api_key_error(request_id: u64, message: String, auth_rejected: bool) -> ServerMessage {
    ServerMessage::ApiKeyError { request_id, message, auth_rejected }
}

fn list(store: &ApiKeyStore) -> anyhow::Result<Vec<ApiKeyDto>> {
    Ok(store.list()?.into_iter().map(dto).collect())
}

/// Answers `ListApiKeys`. `store` is `None` on a hub without the API.
pub fn handle_list_api_keys(store: Option<&ApiKeyStore>, request_id: u64) -> ServerMessage {
    let Some(store) = store else {
        return api_key_error(request_id, "this hub doesn't offer the Warden API".to_string(), false);
    };
    match list(store) {
        Ok(keys) => ServerMessage::ApiKeyList { request_id, keys },
        Err(err) => api_key_error(request_id, format!("failed to read the API keys: {err:#}"), false),
    }
}

/// What `CreateApiKey`/`RevokeApiKey` asks for.
pub enum ApiKeyChange {
    Create { name: String },
    Revoke { id: String },
}

/// Answers `CreateApiKey` (with `ApiKeyCreated`) or `RevokeApiKey` (with the updated list).
pub async fn handle_api_key_change(
    store: Option<&ApiKeyStore>,
    lock: &tokio::sync::Mutex<()>,
    auth_key: &str,
    request_id: u64,
    pairing_key: &str,
    change: ApiKeyChange,
) -> ServerMessage {
    let Some(store) = store else {
        return api_key_error(request_id, "this hub doesn't offer the Warden API".to_string(), false);
    };
    let _serialized = lock.lock().await;
    if !keys_match(pairing_key, auth_key) {
        tokio::time::sleep(WRONG_KEY_DELAY).await;
        return api_key_error(request_id, "wrong pairing key".to_string(), true);
    }
    let result = match change {
        ApiKeyChange::Create { name } => store.create(&name).and_then(|created| Ok(ServerMessage::ApiKeyCreated { request_id, key: created.key, keys: list(store)? })),
        ApiKeyChange::Revoke { id } => match store.revoke(&id) {
            Ok(true) => list(store).map(|keys| ServerMessage::ApiKeyList { request_id, keys }),
            Ok(false) => Err(anyhow::anyhow!("no API key with id '{id}'")),
            Err(err) => Err(err),
        },
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
        let reply = handle_api_key_change(Some(&store), &lock, KEY, 1, "wrong", ApiKeyChange::Create { name: "n8n".into() }).await;
        assert!(matches!(reply, ServerMessage::ApiKeyError { auth_rejected: true, .. }), "{reply:?}");
        assert!(started.elapsed() >= WRONG_KEY_DELAY);
        assert!(store.list().unwrap().is_empty(), "nothing was created");

        let reply = handle_api_key_change(Some(&store), &lock, KEY, 2, KEY, ApiKeyChange::Create { name: "n8n".into() }).await;
        let ServerMessage::ApiKeyCreated { key, keys, .. } = reply else { panic!("{reply:?}") };
        assert!(store.authenticate(&key).unwrap().is_some());
        let ServerMessage::ApiKeyList { keys: listed, .. } = handle_list_api_keys(Some(&store), 3) else { panic!() };
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, keys[0].id);

        let reply = handle_api_key_change(Some(&store), &lock, KEY, 4, KEY, ApiKeyChange::Revoke { id: keys[0].id.clone() }).await;
        assert!(matches!(reply, ServerMessage::ApiKeyList { ref keys, .. } if keys.is_empty()), "{reply:?}");
        assert!(store.authenticate(&key).unwrap().is_none());
        let reply = handle_api_key_change(Some(&store), &lock, KEY, 5, KEY, ApiKeyChange::Revoke { id: "ghost".into() }).await;
        assert!(matches!(reply, ServerMessage::ApiKeyError { auth_rejected: false, .. }));
    }
}
