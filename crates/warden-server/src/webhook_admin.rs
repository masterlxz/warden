//! Incoming webhooks (P105) from a client: the same list/create/edit/pause/remove, and the credential made or taken away,
//! as `warden-server webhooks`, for the web's Webhooks screen. Listing is open to any paired device of the owner, like the
//! task list; every change asks for the pairing key again, with the same 1 s wait and the same per-hub lock as a settings
//! save — a webhook lets a stranger's call run an agent with its tools and its spend. A member has no webhooks
//! (`people::member_refusal`).
//!
//! Changes write `[[webhooks]]` in the config file and the credentials file, and nothing else: the route re-reads the
//! config on every call, so the orchestrator isn't rebuilt. A credential is the only thing shown once: `WebhookCreated`
//! carries it, and nothing the hub sends later does.

use warden_bootstrap::webhooks::{conversation_id, remove_webhook, save_webhook, set_webhook_enabled, WebhookAuth, WebhookConfig};
use warden_bootstrap::{load_config_from_path, save_config, FileConfig};
use warden_server_protocol::protocol::{WebhookDto, WebhookInfoDto};
use warden_server_protocol::ServerMessage;

use crate::settings::{keys_match, SettingsHost, WRONG_KEY_DELAY};
use crate::webhook_tokens::WebhookTokenStore;
use crate::webhooks::WebhookContext;

fn webhook_error(request_id: u64, message: String, auth_rejected: bool) -> ServerMessage {
    ServerMessage::WebhookError { request_id, message, auth_rejected }
}

/// The config's webhooks with what the hub knows about each one's credential.
pub(crate) fn infos(config: &FileConfig, store: &WebhookTokenStore) -> anyhow::Result<Vec<WebhookInfoDto>> {
    let credentials = store.list()?;
    Ok(config
        .webhooks
        .iter()
        .map(|hook| {
            let credential = credentials.iter().find(|c| c.webhook == hook.id);
            WebhookInfoDto {
                id: hook.id.clone(),
                agent_id: hook.agent.clone(),
                prompt: hook.prompt.clone(),
                enabled: hook.enabled,
                auth: hook.auth.as_str().to_string(),
                credential: credential.map(|c| c.kind.as_str().to_string()),
                shown: credential.map(|c| c.shown.clone()),
                created_at_ms: credential.map(|c| c.created_at_ms),
                last_used_at_ms: credential.and_then(|c| c.last_used_at_ms),
                conversation: conversation_id(&hook.id),
            }
        })
        .collect())
}

fn config_of(dto: WebhookDto) -> anyhow::Result<WebhookConfig> {
    Ok(WebhookConfig { id: dto.id, agent: dto.agent_id, prompt: dto.prompt, enabled: dto.enabled, auth: WebhookAuth::parse(&dto.auth)? })
}

fn parts<'a>(webhooks: Option<&'a WebhookContext>, settings: Option<&'a dyn SettingsHost>) -> Result<(&'a WebhookContext, &'a dyn SettingsHost), String> {
    let webhooks = webhooks.ok_or_else(|| "this hub doesn't offer webhooks".to_string())?;
    let settings = settings.ok_or_else(|| "this hub has no settings file, so it has no webhooks".to_string())?;
    Ok((webhooks, settings))
}

fn store_of(ctx: &WebhookContext) -> WebhookTokenStore {
    WebhookTokenStore::new(ctx.tokens_path.as_ref().clone())
}

fn list(ctx: &WebhookContext, settings: &dyn SettingsHost, request_id: u64) -> anyhow::Result<ServerMessage> {
    let config = load_config_from_path(&settings.config_path(), false)?;
    Ok(ServerMessage::WebhookList { request_id, webhooks: infos(&config, &store_of(ctx))?, serves_here: ctx.runner.is_some() })
}

/// Answers `ListWebhooks`.
pub(crate) fn handle_list_webhooks(webhooks: Option<&WebhookContext>, settings: Option<&dyn SettingsHost>, request_id: u64) -> ServerMessage {
    match parts(webhooks, settings) {
        Ok((ctx, settings)) => list(ctx, settings, request_id).unwrap_or_else(|err| webhook_error(request_id, format!("{err:#}"), false)),
        Err(message) => webhook_error(request_id, message, false),
    }
}

/// What `SaveWebhook`/`SetWebhookEnabled`/`DeleteWebhook`/`CreateWebhookCredential`/`RevokeWebhookCredential` asks for.
pub(crate) enum WebhookChange {
    Save { original_id: Option<String>, webhook: WebhookDto },
    SetEnabled { id: String, enabled: bool },
    Delete { id: String },
    CreateCredential { id: String },
    RevokeCredential { id: String },
}

/// Everything a webhook change needs from the connection.
pub(crate) struct WebhookAccess<'a> {
    pub webhooks: Option<&'a WebhookContext>,
    pub settings: Option<&'a dyn SettingsHost>,
    pub lock: &'a tokio::sync::Mutex<()>,
    pub auth_key: &'a str,
}

/// A credential just made: the webhook, the credential itself and what kind it is.
struct Created {
    id: String,
    credential: String,
    kind: WebhookAuth,
}

/// Answers a webhook change with the updated `WebhookList`, or `WebhookCreated` for a new credential.
pub(crate) async fn handle_webhook_change(access: &WebhookAccess<'_>, request_id: u64, pairing_key: &str, change: WebhookChange) -> ServerMessage {
    let (ctx, settings) = match parts(access.webhooks, access.settings) {
        Ok(parts) => parts,
        Err(message) => return webhook_error(request_id, message, false),
    };
    let _serialized = access.lock.lock().await;
    if !keys_match(pairing_key, access.auth_key) {
        tokio::time::sleep(WRONG_KEY_DELAY).await;
        return webhook_error(request_id, "wrong pairing key".to_string(), true);
    }
    let config_path = settings.config_path();
    let tokens = store_of(ctx);
    let result = (|| -> anyhow::Result<Option<Created>> {
        let mut config = load_config_from_path(&config_path, false)?;
        match change {
            WebhookChange::Save { original_id, webhook } => {
                let hook = config_of(webhook)?;
                let new_id = hook.id.trim().to_string();
                let before = original_id.as_deref().and_then(|original| config.webhooks.iter().find(|h| h.id == original)).cloned();
                let new_auth = hook.auth;
                save_webhook(&mut config, original_id.as_deref(), hook)?;
                save_config(&config_path, &config)?;
                // The credential follows the name, and a webhook that now wants the other kind has the wrong one: it goes.
                if let Some(original) = original_id.as_deref().filter(|original| *original != new_id) {
                    tokens.rename(original, &new_id)?;
                }
                if before.is_some_and(|before| before.auth != new_auth) {
                    tokens.revoke(&new_id)?;
                }
            }
            WebhookChange::SetEnabled { id, enabled } => {
                set_webhook_enabled(&mut config, &id, enabled)?;
                save_config(&config_path, &config)?;
            }
            WebhookChange::Delete { id } => {
                remove_webhook(&mut config, &id)?;
                save_config(&config_path, &config)?;
                tokens.revoke(&id)?;
            }
            WebhookChange::CreateCredential { id } => {
                let auth = config.webhooks.iter().find(|h| h.id == id).map(|h| h.auth).ok_or_else(|| anyhow::anyhow!("no webhook named '{id}'"))?;
                let created = tokens.create_credential(&id, auth)?;
                return Ok(Some(Created { id, credential: created.token, kind: auth }));
            }
            WebhookChange::RevokeCredential { id } => {
                anyhow::ensure!(config.webhooks.iter().any(|h| h.id == id), "no webhook named '{id}'");
                anyhow::ensure!(tokens.revoke(&id)?, "webhook '{id}' has no credential");
            }
        }
        Ok(None)
    })();
    let reply = result.and_then(|created| {
        let config = load_config_from_path(&config_path, false)?;
        let webhooks = infos(&config, &tokens)?;
        let serves_here = ctx.runner.is_some();
        Ok(match created {
            Some(Created { id, credential, kind }) => ServerMessage::WebhookCreated { request_id, id, credential, kind: kind.as_str().to_string(), webhooks, serves_here },
            None => ServerMessage::WebhookList { request_id, webhooks, serves_here },
        })
    });
    reply.unwrap_or_else(|err| webhook_error(request_id, format!("{err:#}"), false))
}
