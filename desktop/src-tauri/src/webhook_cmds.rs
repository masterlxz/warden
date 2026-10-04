//! Tauri commands backing the Webhooks screen (P105): the desktop's own `[[webhooks]]` in `config.toml` and the
//! credentials in `webhook_tokens.json`, the same list/create/edit/pause/remove, credential and revoke as
//! `warden-server webhooks` and the web. Stateless like `task_cmds.rs`: every call rereads the config and the credentials.
//!
//! No pairing key is asked: this is the owner's own machine, as on the Tasks and Settings screens. The rules (a rename
//! keeps the credential, a change of mode drops it, a removed webhook leaves none) are the hub's own
//! (`warden_server::webhook_admin::apply_webhook_change`), so the two can't drift apart.
//!
//! Calls only arrive while the embedded hub runs (`hub_running`), at the address the hub is reached at (`hub_url`).

use std::path::PathBuf;

use serde::Serialize;
use tauri::State;
use warden_bootstrap::webhooks::conversation_id;
use warden_bootstrap::{load_config_from_path, load_conversation, ChatRole};
use warden_server::webhook_admin::{apply_webhook_change, infos, WebhookChange};
use warden_server::webhook_tokens::WebhookTokenStore;
use warden_server_protocol::protocol::{WebhookDto, WebhookInfoDto};

use crate::AppState;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebhookListPayload {
    webhooks: Vec<WebhookInfoDto>,
    /// The embedded hub is running — calls only arrive then.
    hub_running: bool,
    /// Where the hub is reached, `http(s)://host:port`; absent when it isn't running or TLS has no known host name.
    #[serde(skip_serializing_if = "Option::is_none")]
    hub_url: Option<String>,
}

/// A credential just made — the only time it is shown — with the list as it is now.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebhookCreatedPayload {
    id: String,
    credential: String,
    /// `"token"` or `"hmac"`.
    kind: String,
    list: WebhookListPayload,
}

/// One message of a webhook's conversation, for the read-only history.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebhookMessage {
    role: &'static str,
    content: String,
    created_at: i64,
}

fn config_path() -> Result<PathBuf, String> {
    warden_bootstrap::default_config_path().ok_or_else(|| "could not determine the OS config directory".to_string())
}

fn tokens() -> Result<WebhookTokenStore, String> {
    warden_bootstrap::default_webhook_tokens_path().map(WebhookTokenStore::new).ok_or_else(|| "could not determine the OS config directory".to_string())
}

fn list(state: &AppState) -> Result<WebhookListPayload, String> {
    let config = load_config_from_path(&config_path()?, false).map_err(|e| format!("{e:#}"))?;
    let webhooks = infos(&config, &tokens()?).map_err(|e| format!("{e:#}"))?;
    let (hub_running, hub_url) = match state.embedded_server.lock().unwrap().as_ref() {
        Some(hub) => (true, hub.base_url()),
        None => (false, None),
    };
    Ok(WebhookListPayload { webhooks, hub_running, hub_url })
}

/// Applies `change`, then answers with the list (and the credential, if one was made).
fn change(state: &AppState, change: WebhookChange) -> Result<(Option<warden_server::webhook_admin::Created>, WebhookListPayload), String> {
    let created = apply_webhook_change(&config_path()?, &tokens()?, change).map_err(|e| format!("{e:#}"))?;
    Ok((created, list(state)?))
}

#[tauri::command]
pub fn list_webhooks(state: State<'_, AppState>) -> Result<WebhookListPayload, String> {
    list(&state)
}

/// Creates a webhook, or replaces `original_id` with it (a rename keeps the credential; a change of `auth` drops it).
#[tauri::command]
pub fn save_webhook(state: State<'_, AppState>, original_id: Option<String>, webhook: WebhookDto) -> Result<WebhookListPayload, String> {
    change(&state, WebhookChange::Save { original_id, webhook }).map(|(_, list)| list)
}

#[tauri::command]
pub fn set_webhook_enabled_cmd(state: State<'_, AppState>, id: String, enabled: bool) -> Result<WebhookListPayload, String> {
    change(&state, WebhookChange::SetEnabled { id, enabled }).map(|(_, list)| list)
}

/// Removes the webhook and its credential. Its conversation stays.
#[tauri::command]
pub fn delete_webhook(state: State<'_, AppState>, id: String) -> Result<WebhookListPayload, String> {
    change(&state, WebhookChange::Delete { id }).map(|(_, list)| list)
}

/// A new credential — a token, or a signing secret for an `hmac` webhook — replacing the old one. Shown once.
#[tauri::command]
pub fn create_webhook_credential(state: State<'_, AppState>, id: String) -> Result<WebhookCreatedPayload, String> {
    let (created, list) = change(&state, WebhookChange::CreateCredential { id })?;
    let created = created.ok_or_else(|| "the credential was not made".to_string())?;
    Ok(WebhookCreatedPayload { id: created.id, credential: created.credential, kind: created.kind.as_str().to_string(), list })
}

#[tauri::command]
pub fn revoke_webhook_credential(state: State<'_, AppState>, id: String) -> Result<WebhookListPayload, String> {
    change(&state, WebhookChange::RevokeCredential { id }).map(|(_, list)| list)
}

/// The webhook's conversation, oldest first — read-only on this screen. Empty when this machine's hub never got a call.
#[tauri::command]
pub fn webhook_history(id: String) -> Result<Vec<WebhookMessage>, String> {
    let dir = warden_bootstrap::default_server_tasks_dir().map(warden_bootstrap::tasks::TaskStore::new).ok_or_else(|| "could not determine the OS config directory".to_string())?;
    let conversation = load_conversation(&dir.conversations_dir(), &conversation_id(&id)).map_err(|e| format!("{e:#}"))?;
    Ok(conversation
        .map(|c| c.messages)
        .unwrap_or_default()
        .into_iter()
        .map(|m| WebhookMessage { role: if m.role == ChatRole::User { "user" } else { "assistant" }, content: m.content, created_at: m.created_at })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(auth: &str, credential: Option<&str>) -> WebhookInfoDto {
        WebhookInfoDto {
            id: "build".into(),
            agent_id: None,
            prompt: "p".into(),
            enabled: true,
            auth: auth.into(),
            credential: credential.map(str::to_string),
            shown: credential.map(|_| "whk_12345".to_string()),
            created_at_ms: credential.map(|_| 5),
            last_used_at_ms: None,
            conversation: "task-hook-build".into(),
        }
    }

    // Locks in the exact camelCase JSON shape `desktop/src/components/WebhooksView.tsx` reads: an absent value is left out
    // (the TypeScript types say `?:`, not `| null`).
    #[test]
    fn the_payloads_serialize_the_way_the_screen_reads_them() {
        let list = WebhookListPayload { webhooks: vec![info("token", None)], hub_running: false, hub_url: None };
        assert_eq!(
            serde_json::to_value(&list).unwrap(),
            serde_json::json!({ "webhooks": [{ "id": "build", "prompt": "p", "enabled": true, "auth": "token", "conversation": "task-hook-build" }], "hubRunning": false })
        );
        let created = WebhookCreatedPayload {
            id: "build".into(),
            credential: "whk_secret".into(),
            kind: "token".into(),
            list: WebhookListPayload { webhooks: vec![info("token", Some("token"))], hub_running: true, hub_url: Some("http://localhost:7420".into()) },
        };
        assert_eq!(
            serde_json::to_value(&created).unwrap(),
            serde_json::json!({
                "id": "build", "credential": "whk_secret", "kind": "token",
                "list": {
                    "webhooks": [{ "id": "build", "prompt": "p", "enabled": true, "auth": "token", "credential": "token", "shown": "whk_12345", "createdAtMs": 5, "conversation": "task-hook-build" }],
                    "hubRunning": true, "hubUrl": "http://localhost:7420"
                }
            })
        );
        assert_eq!(
            serde_json::to_value(WebhookMessage { role: "assistant", content: "hi".into(), created_at: 7 }).unwrap(),
            serde_json::json!({ "role": "assistant", "content": "hi", "createdAt": 7 })
        );
        // What the screen sends back: `webhook` as the web sends it, with `auth` optional (a token).
        let sent: WebhookDto = serde_json::from_str(r#"{"id":"gh","agentId":"ops","prompt":"p","enabled":false,"auth":"hmac"}"#).unwrap();
        assert_eq!((sent.id.as_str(), sent.agent_id.as_deref(), sent.enabled, sent.auth.as_str()), ("gh", Some("ops"), false, "hmac"));
    }
}
