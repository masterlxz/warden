//! Tauri commands backing Settings' "Warden API" section (P12): the keys the embedded hub's
//! OpenAI-compatible routes accept. Stateless like `workspace_cmds.rs`: `ApiKeyStore` rereads
//! `api_keys.json` on every call, the same file `warden-server api-keys` and the web use.

use serde::Serialize;
use warden_server::api_keys::{ApiKey, ApiKeyStore};

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ApiKeyInfo {
    id: String,
    name: String,
    shown: String,
    created_at_ms: i64,
    last_used_at_ms: Option<i64>,
    /// The only agent this key speaks as; `None` for a general key.
    agent_id: Option<String>,
    /// P84: the member the key belongs to; `None` for the owner's.
    user: Option<String>,
}

impl From<ApiKey> for ApiKeyInfo {
    fn from(key: ApiKey) -> Self {
        Self {
            id: key.id,
            name: key.name,
            shown: key.shown,
            created_at_ms: key.created_at_ms,
            last_used_at_ms: key.last_used_at_ms,
            agent_id: key.agent_id,
            user: key.user,
        }
    }
}

/// A key just created: `key` is shown once and kept nowhere.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatedApiKeyInfo {
    key: String,
    info: ApiKeyInfo,
}

fn store() -> Result<ApiKeyStore, String> {
    warden_bootstrap::default_api_keys_path().map(ApiKeyStore::new).ok_or_else(|| "could not determine the OS config directory".to_string())
}

#[tauri::command]
pub fn list_api_keys() -> Result<Vec<ApiKeyInfo>, String> {
    Ok(store()?.list().map_err(|e| format!("{e:#}"))?.into_iter().map(Into::into).collect())
}

/// `agent_id` binds the key to that agent (it must be in the config); `None` or blank is general.
#[tauri::command]
pub fn create_api_key(name: String, agent_id: Option<String>) -> Result<CreatedApiKeyInfo, String> {
    if agent_id.as_deref().is_some_and(|a| !a.trim().is_empty()) {
        let config_path = warden_bootstrap::default_config_path().ok_or_else(|| "could not determine the OS config directory".to_string())?;
        warden_server::api_key_admin::check_agent_exists(&config_path, agent_id.as_deref(), None).map_err(|e| format!("{e:#}"))?;
    }
    let created = store()?.create(&name, agent_id.as_deref()).map_err(|e| format!("{e:#}"))?;
    Ok(CreatedApiKeyInfo { key: created.key, info: created.info.into() })
}

#[tauri::command]
pub fn revoke_api_key(id: String) -> Result<(), String> {
    if store()?.revoke(&id).map_err(|e| format!("{e:#}"))? {
        Ok(())
    } else {
        Err(format!("no API key with id '{id}'"))
    }
}
