//! `warden bots pair ...` (P117): the owner's side of the Telegram and WhatsApp bots' pairing. A
//! stranger who wrote to a bot with `pairing` on was given a code; here the owner lists the waiting
//! requests and approves or denies one. It only touches `bot_pairing.json` and the allow-lists in
//! `config.toml`, so it needs no model key, and a running bot sees the change from its next message.

use std::path::Path;

use clap::Subcommand;
use warden_bootstrap::bot_access::unix_now;
use warden_bootstrap::bot_pairing::{display_code, BotPairing, PairingRequest};

#[derive(Subcommand, Debug)]
pub enum BotsCommand {
    /// Pairing requests from people who wrote to a bot.
    Pair {
        #[command(subcommand)]
        action: PairAction,
    },
}

#[derive(Subcommand, Debug)]
pub enum PairAction {
    /// The requests waiting for an answer.
    List,
    /// Let the sender in: adds them to the bot's allow-list.
    Approve {
        /// The code the sender was given, with or without the dash.
        code: String,
    },
    /// Drop a request without letting the sender in.
    Deny { code: String },
}

/// One line per request: the code to type, who it is and how long it still counts.
fn describe(request: &PairingRequest, now: u64) -> String {
    let label = if request.label.is_empty() { String::new() } else { format!(" ({})", request.label) };
    let minutes = request.expires_at.saturating_sub(now).div_ceil(60);
    format!("{}  {} {}{}  expires in {} min", display_code(&request.code), request.channel, request.sender, label, minutes)
}

/// Runs `command` against the config at `config_path`, writing what it says to `out`.
pub fn run(command: BotsCommand, config_path: Option<&Path>, out: &mut impl std::io::Write) -> anyhow::Result<()> {
    let config_path = config_path.ok_or_else(|| anyhow::anyhow!("could not determine the OS config directory; pass --config"))?;
    let store = BotPairing::beside(config_path);
    let now = unix_now();
    let BotsCommand::Pair { action } = command;
    match action {
        PairAction::List => {
            let pending = store.list(now)?;
            if pending.is_empty() {
                writeln!(out, "No pairing requests waiting. A bot only makes them with `pairing = true` under [telegram] or [whatsapp].")?;
            }
            for request in &pending {
                writeln!(out, "{}", describe(request, now))?;
            }
        }
        PairAction::Approve { code } => {
            let request = store.approve(&code, now, config_path)?;
            writeln!(out, "Approved: {} {} is on the {} allow-list now.", request.channel, request.sender, request.channel)?;
        }
        PairAction::Deny { code } => {
            let request = store.deny(&code, now)?;
            writeln!(out, "Denied: {} {}.", request.channel, request.sender)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use warden_bootstrap::bot_pairing::{Issued, TELEGRAM};

    #[test]
    fn list_approve_and_deny_talk_to_the_files_the_bots_use() {
        let dir = std::env::temp_dir().join(format!("warden-cli-bots-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let config = dir.join("config.toml");
        std::fs::write(&config, "").unwrap();
        let store = BotPairing::beside(&config);
        let Issued::Fresh(code) = store.request(TELEGRAM, "42", "ana", unix_now()).unwrap() else { panic!("a fresh request") };
        let Issued::Fresh(other) = store.request(TELEGRAM, "43", "", unix_now()).unwrap() else { panic!("a fresh request") };

        let say = |command: BotsCommand| {
            let mut out = Vec::new();
            run(command, Some(&config), &mut out).map(|()| String::from_utf8(out).unwrap())
        };
        let listed = say(BotsCommand::Pair { action: PairAction::List }).unwrap();
        assert!(listed.contains(&display_code(&code)) && listed.contains("telegram 42 (ana)"), "{listed}");

        let approved = say(BotsCommand::Pair { action: PairAction::Approve { code: display_code(&code).to_lowercase() } }).unwrap();
        assert!(approved.contains("telegram 42"), "{approved}");
        assert_eq!(warden_bootstrap::load_config_from_path(&config, false).unwrap().telegram.allowed_users, [42]);

        say(BotsCommand::Pair { action: PairAction::Deny { code: other } }).unwrap();
        assert!(say(BotsCommand::Pair { action: PairAction::List }).unwrap().starts_with("No pairing requests"));
        assert!(say(BotsCommand::Pair { action: PairAction::Approve { code } }).is_err(), "a code works once");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
