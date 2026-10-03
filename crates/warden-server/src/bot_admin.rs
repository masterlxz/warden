//! The owner answering the Telegram and WhatsApp bots' pairing requests (P117) from a client: the web's
//! Settings screen does what `warden bots pair` and the desktop do. The requests live in
//! `bot_pairing.json` beside the hub's `config.toml` (`warden_bootstrap::bot_pairing`); approving
//! puts the sender on the bot's allow-list in that file, which a running bot reads again.
//! Listing is the root's only (`people::member_refusal` stops a member first); deciding also asks for
//! the pairing key, with the same 1 s wait on a wrong one and the same per-hub lock as a settings save.

use warden_bootstrap::bot_access::unix_now;
use warden_bootstrap::bot_pairing::BotPairing;
use warden_server_protocol::ServerMessage;

use crate::settings::{keys_match, SettingsHost, WRONG_KEY_DELAY};

const NO_SETTINGS: &str = "this hub has no settings file, so it has no bots to pair";

fn error(request_id: u64, message: String, auth_rejected: bool) -> ServerMessage {
    ServerMessage::UserError { request_id, message, auth_rejected }
}

/// What is waiting, oldest first.
fn pairings(settings: &dyn SettingsHost, request_id: u64) -> ServerMessage {
    match BotPairing::beside(&settings.config_path()).list(unix_now()) {
        Ok(requests) => ServerMessage::BotPairings { request_id, pairings: requests.into_iter().map(Into::into).collect() },
        Err(err) => error(request_id, format!("{err:#}"), false),
    }
}

/// Answers `ListBotPairings`.
pub fn handle_list_bot_pairings(settings: Option<&dyn SettingsHost>, request_id: u64) -> ServerMessage {
    match settings {
        Some(settings) => pairings(settings, request_id),
        None => error(request_id, NO_SETTINGS.to_string(), false),
    }
}

/// Answers `ResolveBotPairing` with what is still waiting: `approve` lets the sender in, otherwise the
/// request is dropped. An unknown or expired code is an error and changes nothing.
pub async fn handle_resolve_bot_pairing(
    settings: Option<&dyn SettingsHost>,
    lock: &tokio::sync::Mutex<()>,
    auth_key: &str,
    request_id: u64,
    pairing_key: &str,
    code: &str,
    approve: bool,
) -> ServerMessage {
    let Some(settings) = settings else { return error(request_id, NO_SETTINGS.to_string(), false) };
    let _serialized = lock.lock().await;
    if !keys_match(pairing_key, auth_key) {
        tokio::time::sleep(WRONG_KEY_DELAY).await;
        return error(request_id, "wrong pairing key".to_string(), true);
    }
    let config_path = settings.config_path();
    let store = BotPairing::beside(&config_path);
    let done = if approve { store.approve(code, unix_now(), &config_path) } else { store.deny(code, unix_now()) };
    match done {
        Ok(_) => pairings(settings, request_id),
        Err(err) => error(request_id, format!("{err:#}"), false),
    }
}
