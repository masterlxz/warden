//! Tauri commands backing Settings' "Learning and bots" section (P118): `[learning]`, the Telegram and
//! WhatsApp allow-lists and the Telegram bot token, all in this machine's `config.toml`. Stateless
//! like `people_cmds.rs`: each call rereads the file, changes only this slice and saves, so it can't
//! race the main settings form beyond a plain last-write-wins on the file. The checks are the web's
//! (`warden_bootstrap::settings::apply_bots_settings`), so both screens refuse the same things.

use serde::{Deserialize, Serialize};
use warden_bootstrap::settings::{apply_bots_settings, bots_settings, secret_status};
use warden_bootstrap::{load_config_from_path, save_config};
use warden_server_protocol::protocol::{BotsSettingsDto, SecretStatusDto};

/// What the section shows: the shared block plus whether a Telegram token is saved (never the token).
#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BotsPayload {
    bots: BotsSettingsDto,
    telegram_token: SecretStatusDto,
    /// Provider and combo ids, for the learning model picker.
    model_ids: Vec<String>,
}

/// What a save carries. `telegram_token`: `None` keeps the saved one, an empty string removes it, any
/// other text replaces it.
#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct BotsUpdate {
    bots: BotsSettingsDto,
    telegram_token: Option<String>,
}

fn config_path() -> Result<std::path::PathBuf, String> {
    warden_bootstrap::default_config_path().ok_or_else(|| "could not determine the OS config directory".to_string())
}

fn payload(config: &warden_bootstrap::FileConfig) -> BotsPayload {
    BotsPayload {
        bots: bots_settings(config),
        telegram_token: secret_status(config.api_keys.telegram_bot_token.as_deref()),
        model_ids: config.providers.iter().map(|p| p.id.clone()).chain(config.combos.iter().map(|c| c.id.clone())).collect(),
    }
}

/// Applies `update` to `config`: the bots block through the shared checks, then the token.
fn apply(config: &mut warden_bootstrap::FileConfig, update: BotsUpdate) -> Result<(), String> {
    let (providers, combos) = (config.providers.clone(), config.combos.clone());
    apply_bots_settings(config, update.bots, &providers, &combos)?;
    if let Some(token) = update.telegram_token {
        let token = token.trim();
        config.api_keys.telegram_bot_token = (!token.is_empty()).then(|| token.to_string());
    }
    Ok(())
}

#[tauri::command]
pub fn get_bots_settings() -> Result<BotsPayload, String> {
    let config = load_config_from_path(&config_path()?, false).map_err(|e| format!("{e:#}"))?;
    Ok(payload(&config))
}

#[tauri::command]
pub fn save_bots_settings(update: BotsUpdate) -> Result<BotsPayload, String> {
    let path = config_path()?;
    let mut config = load_config_from_path(&path, false).map_err(|e| format!("{e:#}"))?;
    apply(&mut config, update)?;
    save_config(&path, &config).map_err(|e| format!("{e:#}"))?;
    Ok(payload(&config))
}

#[cfg(test)]
mod tests {
    use super::*;
    use warden_bootstrap::{ComboConfig, FileConfig};

    // The frontend (`desktop/src/types.ts`) sends the update in exactly this shape.
    fn update(json: serde_json::Value) -> BotsUpdate {
        serde_json::from_value(json).unwrap()
    }

    fn block() -> serde_json::Value {
        serde_json::json!({
            "learningEnabled": true, "learningProvider": "", "learningMaxPerDay": 3,
            "learningBotChats": ["telegram:42"], "telegramAllowedUsers": [42], "whatsappAllowedChats": ["5511999999999"]
        })
    }

    #[test]
    fn the_wire_names_are_the_ones_the_frontend_uses() {
        let mut config = FileConfig::default();
        apply(&mut config, update(serde_json::json!({ "bots": block(), "telegramToken": "123:abc" }))).unwrap();
        assert!(config.learning.enabled);
        assert_eq!(config.telegram.allowed_users, [42]);
        assert_eq!(config.whatsapp.allowed_chats, ["5511999999999"]);
        let json = serde_json::to_value(payload(&config)).unwrap();
        assert_eq!(json["bots"]["telegramAllowedUsers"], serde_json::json!([42]));
        assert_eq!(json["telegramToken"]["set"], true);
        assert!(!json.to_string().contains("123:abc"), "the token never comes back");
    }

    /// The token: absent keeps it, empty removes it, text replaces it. A refused block changes nothing.
    #[test]
    fn the_token_is_kept_removed_or_replaced_and_a_bad_block_is_refused() {
        let mut config = FileConfig::default();
        config.api_keys.telegram_bot_token = Some("old".into());
        apply(&mut config, update(serde_json::json!({ "bots": block() }))).unwrap();
        assert_eq!(config.api_keys.telegram_bot_token.as_deref(), Some("old"));
        apply(&mut config, update(serde_json::json!({ "bots": block(), "telegramToken": " new " }))).unwrap();
        assert_eq!(config.api_keys.telegram_bot_token.as_deref(), Some("new"));
        apply(&mut config, update(serde_json::json!({ "bots": block(), "telegramToken": "" }))).unwrap();
        assert_eq!(config.api_keys.telegram_bot_token, None);

        let mut bad = block();
        bad["learningProvider"] = "ghost".into();
        assert!(apply(&mut config, update(serde_json::json!({ "bots": bad }))).is_err());
        config.combos.push(ComboConfig { id: "cheap".into(), providers: vec![] });
        let mut good = block();
        good["learningProvider"] = "cheap".into();
        apply(&mut config, update(serde_json::json!({ "bots": good }))).unwrap();
        assert_eq!(config.learning.provider.as_deref(), Some("cheap"));
    }
}
