//! Tauri commands backing Settings' "Learning and bots" section (P118): `[learning]`, the Telegram and
//! WhatsApp allow-lists and the Telegram bot token, all in this machine's `config.toml`. Stateless
//! like `people_cmds.rs`: each call rereads the file, changes only this slice and saves, so it can't
//! race the main settings form beyond a plain last-write-wins on the file. The checks are the web's
//! (`warden_bootstrap::settings::apply_bots_settings`), so both screens refuse the same things.

use serde::{Deserialize, Serialize};
use warden_bootstrap::bot_access::unix_now;
use warden_bootstrap::bot_hub::bot_members;
use warden_bootstrap::bot_pairing::BotPairing;
use warden_bootstrap::settings::{apply_bots_settings, bots_settings, secret_status};
use warden_bootstrap::{load_config_from_path, save_config};
use warden_server_protocol::protocol::{BotMemberDto, BotPairingDto, BotsSettingsDto, SecretStatusDto};

/// What the section shows: the shared block plus whether a Telegram token is saved (never the token).
#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BotsPayload {
    bots: BotsSettingsDto,
    telegram_token: SecretStatusDto,
    /// Provider and combo ids, for the learning model picker.
    model_ids: Vec<String>,
    /// Strangers waiting for the owner to let them talk to a bot (P117).
    pairings: Vec<BotPairingDto>,
    /// Who an approved chat may speak as, and whether the bots are linked to them (`warden bots link`).
    members: Vec<BotMemberDto>,
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

/// `config` as the section shows it, with the pairing requests waiting beside the file at `path`.
fn payload(config: &warden_bootstrap::FileConfig, path: &std::path::Path) -> BotsPayload {
    let pairings = match BotPairing::beside(path).list(unix_now()) {
        Ok(requests) => requests.into_iter().map(Into::into).collect(),
        Err(err) => {
            eprintln!("can't read the pairing requests: {err:#}");
            Vec::new()
        }
    };
    let members = bot_members(path).unwrap_or_else(|err| {
        eprintln!("can't read who a chat may speak as: {err:#}");
        Vec::new()
    });
    BotsPayload {
        bots: bots_settings(config),
        telegram_token: secret_status(config.api_keys.telegram_bot_token.as_deref()),
        model_ids: config.providers.iter().map(|p| p.id.clone()).chain(config.combos.iter().map(|c| c.id.clone())).collect(),
        pairings,
        members,
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
    let path = config_path()?;
    let config = load_config_from_path(&path, false).map_err(|e| format!("{e:#}"))?;
    Ok(payload(&config, &path))
}

#[tauri::command]
pub fn save_bots_settings(update: BotsUpdate) -> Result<BotsPayload, String> {
    let path = config_path()?;
    let mut config = load_config_from_path(&path, false).map_err(|e| format!("{e:#}"))?;
    apply(&mut config, update)?;
    save_config(&path, &config).map_err(|e| format!("{e:#}"))?;
    Ok(payload(&config, &path))
}

/// Lets the sender behind `code` talk to their bot (P117): they land on its allow-list. With `member`, the chat
/// speaks as that member of the workspace instead of as the owner; that is refused, changing nothing, when there
/// is no such member, no `[bot_hub]` or the bots aren't linked to them. Answers with the section as it is now,
/// so the lists and the waiting requests both show the change.
#[tauri::command]
pub fn approve_bot_pairing(code: String, member: Option<String>) -> Result<BotsPayload, String> {
    let path = config_path()?;
    approve_in(&path, &code, member.as_deref())?;
    get_bots_settings()
}

/// `approve_bot_pairing` on the config at `path`; split out so a test needn't touch the user's own file.
fn approve_in(path: &std::path::Path, code: &str, member: Option<&str>) -> Result<(), String> {
    BotPairing::beside(path).approve_as(code, unix_now(), path, member.filter(|m| !m.is_empty())).map(|_| ()).map_err(|e| format!("{e:#}"))
}

/// Drops the request behind `code` without letting the sender in.
#[tauri::command]
pub fn deny_bot_pairing(code: String) -> Result<BotsPayload, String> {
    let path = config_path()?;
    BotPairing::beside(&path).deny(&code, unix_now()).map_err(|e| format!("{e:#}"))?;
    get_bots_settings()
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
        let dir = std::env::temp_dir().join(format!("warden-bot-cmds-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        BotPairing::beside(&path).request("telegram", "7", "ana", unix_now()).unwrap();

        let json = serde_json::to_value(payload(&config, &path)).unwrap();
        assert_eq!(json["bots"]["telegramAllowedUsers"], serde_json::json!([42]));
        assert_eq!(json["bots"]["telegramPairing"], false);
        assert_eq!(json["telegramToken"]["set"], true);
        assert!(!json.to_string().contains("123:abc"), "the token never comes back");
        let pairing = &json["pairings"][0];
        assert_eq!((pairing["channel"].as_str(), pairing["sender"].as_str(), pairing["label"].as_str()), (Some("telegram"), Some("7"), Some("ana")));
        assert!(pairing["code"].as_str().unwrap().contains('-'), "shown as ABCD-EFGH");
        assert!(pairing["expiresAt"].is_u64());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// P117: the section lists who a chat may speak as (and whether the bots are linked to them), and an approval
    /// can name one. A refusal lets nobody in and keeps the request; linked, the chat lands on the list and in
    /// `members`; an empty name is no name.
    #[test]
    fn an_approval_can_name_the_member_the_chat_speaks_as() {
        use warden_bootstrap::bot_access::BotHubSettings;
        use warden_bootstrap::bot_hub::{HubTokens, Linked};
        let dir = std::env::temp_dir().join(format!("warden-bot-cmds-members-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        let mut config = FileConfig::default();
        warden_bootstrap::users::add_user(&mut config, "ana", "Ana", "provisional-pass").unwrap();
        config.bot_hub = Some(BotHubSettings { url: "ws://127.0.0.1:7420".into() });
        save_config(&path, &config).unwrap();
        let store = BotPairing::beside(&path);
        let request = |sender: &str| match store.request("telegram", sender, "", unix_now()).unwrap() {
            warden_bootstrap::bot_pairing::Issued::Fresh(code) => code,
            other => panic!("{other:?}"),
        };
        let (first, second, third) = (request("7"), request("8"), request("9"));
        let reload = || load_config_from_path(&path, false).unwrap();

        // The wire names the frontend reads.
        let json = serde_json::to_value(payload(&reload(), &path)).unwrap();
        assert_eq!(json["members"], serde_json::json!([{ "id": "ana", "name": "Ana", "linked": false }]));

        // Not linked: refused with the way out, nothing changes, the request is still there.
        let err = approve_in(&path, &first, Some("ana")).unwrap_err();
        assert!(err.contains("warden bots link ana"), "{err}");
        assert!(reload().telegram.allowed_users.is_empty());
        assert_eq!(store.list(unix_now()).unwrap().len(), 3);

        HubTokens::beside(&path).set("ana", Linked { device_id: "warden-bot-ana".into(), device_token: "t".into() }).unwrap();
        assert_eq!(serde_json::to_value(payload(&reload(), &path)).unwrap()["members"][0]["linked"], true);
        approve_in(&path, &first, Some("ana")).unwrap();
        assert_eq!((reload().telegram.allowed_users, reload().telegram.member_for(7).map(String::from)), (vec![7], Some("ana".to_string())));

        // No member, or an empty one (what a select left on "me" sends): the owner's assistant, nothing mapped.
        approve_in(&path, &second, None).unwrap();
        approve_in(&path, &third, Some("")).unwrap();
        let after = reload();
        assert_eq!(after.telegram.allowed_users, [7, 8, 9]);
        assert_eq!(after.telegram.members.len(), 1, "only the first chat speaks as a member");
        std::fs::remove_dir_all(&dir).unwrap();
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
