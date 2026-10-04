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
pub fn infos(config: &FileConfig, store: &WebhookTokenStore) -> anyhow::Result<Vec<WebhookInfoDto>> {
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
pub enum WebhookChange {
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
pub struct Created {
    pub id: String,
    pub credential: String,
    pub kind: WebhookAuth,
}

/// Applies `change` to the config at `config_path` and to the credentials in `tokens`, with no key asked and no lock held:
/// the hub's handler does both first, and the desktop (the owner's own machine) needs neither. `Some` for a credential
/// just made, which is the only time it exists.
///
/// The credential follows the name when a webhook is renamed, a webhook that now wants the other kind of credential loses
/// the one it had (it would prove nothing), and a removed webhook leaves none behind.
pub fn apply_webhook_change(config_path: &std::path::Path, tokens: &WebhookTokenStore, change: WebhookChange) -> anyhow::Result<Option<Created>> {
    let mut config = load_config_from_path(config_path, false)?;
    match change {
        WebhookChange::Save { original_id, webhook } => {
            let hook = config_of(webhook)?;
            let new_id = hook.id.trim().to_string();
            let before = original_id.as_deref().and_then(|original| config.webhooks.iter().find(|h| h.id == original)).cloned();
            let new_auth = hook.auth;
            save_webhook(&mut config, original_id.as_deref(), hook)?;
            save_config(config_path, &config)?;
            if let Some(original) = original_id.as_deref().filter(|original| *original != new_id) {
                tokens.rename(original, &new_id)?;
            }
            if before.is_some_and(|before| before.auth != new_auth) {
                tokens.revoke(&new_id)?;
            }
        }
        WebhookChange::SetEnabled { id, enabled } => {
            set_webhook_enabled(&mut config, &id, enabled)?;
            save_config(config_path, &config)?;
        }
        WebhookChange::Delete { id } => {
            remove_webhook(&mut config, &id)?;
            save_config(config_path, &config)?;
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
    let result = apply_webhook_change(&config_path, &tokens, change);
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

#[cfg(test)]
mod tests {
    use super::*;
    use warden_bootstrap::FileConfig;

    fn setup(name: &str) -> (std::path::PathBuf, WebhookTokenStore) {
        let dir = std::env::temp_dir().join(format!("warden-webhook-admin-{name}-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&dir).unwrap();
        let config = dir.join("config.toml");
        save_config(&config, &FileConfig::default()).unwrap();
        (config, WebhookTokenStore::new(dir.join("webhook_tokens.json")))
    }

    fn dto(id: &str, auth: &str) -> WebhookDto {
        WebhookDto { id: id.into(), agent_id: None, prompt: "p".into(), enabled: true, auth: auth.into() }
    }

    /// What the desktop calls, with no hub, no key and no lock: the same rules as the hub's handler.
    #[test]
    fn a_change_applied_directly_keeps_the_rules_the_hub_keeps() {
        let (config, tokens) = setup("direct");
        let apply = |change| apply_webhook_change(&config, &tokens, change);
        assert!(apply(WebhookChange::Save { original_id: None, webhook: dto(" build ", "token") }).unwrap().is_none());
        let token = apply(WebhookChange::CreateCredential { id: "build".into() }).unwrap().expect("a credential is made");
        assert_eq!((token.id.as_str(), token.kind), ("build", WebhookAuth::Token));
        assert!(token.credential.starts_with("whk_") && tokens.authenticate("build", &token.credential).unwrap());

        // A rename takes the credential along; editing without touching the mode keeps it.
        apply(WebhookChange::Save { original_id: Some("build".into()), webhook: dto("ci", "token") }).unwrap();
        assert!(tokens.authenticate("ci", &token.credential).unwrap() && !tokens.authenticate("build", &token.credential).unwrap());
        // A change of mode drops it: it would prove nothing for the new kind.
        apply(WebhookChange::Save { original_id: Some("ci".into()), webhook: dto("ci", "hmac") }).unwrap();
        assert_eq!(tokens.kind_of("ci").unwrap(), None);
        let secret = apply(WebhookChange::CreateCredential { id: "ci".into() }).unwrap().unwrap();
        assert!(secret.credential.starts_with("whsec_") && secret.kind == WebhookAuth::Hmac);
        assert_eq!(tokens.secret_of("ci").unwrap().as_deref(), Some(secret.credential.as_str()));

        // Pause, revoke (a second one says there is none), and delete leaves no credential behind.
        apply(WebhookChange::SetEnabled { id: "ci".into(), enabled: false }).unwrap();
        assert!(!load_config_from_path(&config, false).unwrap().webhooks[0].enabled);
        apply(WebhookChange::RevokeCredential { id: "ci".into() }).unwrap();
        assert!(apply(WebhookChange::RevokeCredential { id: "ci".into() }).is_err());
        apply(WebhookChange::CreateCredential { id: "ci".into() }).unwrap();
        apply(WebhookChange::Delete { id: "ci".into() }).unwrap();
        assert_eq!((load_config_from_path(&config, false).unwrap().webhooks.len(), tokens.kind_of("ci").unwrap()), (0, None));

        // Refusals change nothing.
        assert!(apply(WebhookChange::Save { original_id: None, webhook: dto("bad id", "token") }).is_err());
        assert!(apply(WebhookChange::Save { original_id: None, webhook: dto("ok", "basic") }).is_err());
        assert!(apply(WebhookChange::CreateCredential { id: "ghost".into() }).is_err());
        assert!(load_config_from_path(&config, false).unwrap().webhooks.is_empty());
    }

    #[test]
    fn the_list_says_what_each_webhook_wants_and_what_it_has() {
        let (config_path, tokens) = setup("infos");
        apply_webhook_change(&config_path, &tokens, WebhookChange::Save { original_id: None, webhook: dto("a", "hmac") }).unwrap();
        apply_webhook_change(&config_path, &tokens, WebhookChange::Save { original_id: None, webhook: dto("b", "token") }).unwrap();
        // `a` wants a signature but holds a token (a mismatch the screen has to show); `b` has nothing.
        tokens.create("a").unwrap();
        let listed = infos(&load_config_from_path(&config_path, false).unwrap(), &tokens).unwrap();
        assert_eq!((listed[0].auth.as_str(), listed[0].credential.as_deref(), listed[0].conversation.as_str()), ("hmac", Some("token"), "task-hook-a"));
        assert_eq!((listed[1].auth.as_str(), listed[1].credential.as_deref(), listed[1].shown.clone()), ("token", None, None));
        assert!(listed[0].shown.as_deref().is_some_and(|s| s.starts_with("whk_")) && listed[0].created_at_ms.is_some());
    }
}
