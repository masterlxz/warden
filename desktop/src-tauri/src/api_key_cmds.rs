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
}

impl From<ApiKey> for ApiKeyInfo {
    fn from(key: ApiKey) -> Self {
        Self { id: key.id, name: key.name, shown: key.shown, created_at_ms: key.created_at_ms, last_used_at_ms: key.last_used_at_ms }
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

#[tauri::command]
pub fn create_api_key(name: String) -> Result<CreatedApiKeyInfo, String> {
    let created = store()?.create(&name).map_err(|e| format!("{e:#}"))?;
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
