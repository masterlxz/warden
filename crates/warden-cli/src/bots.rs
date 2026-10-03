//! `warden bots ...` (P117): the owner's side of the Telegram and WhatsApp bots.
//!
//! `pair`: a stranger who wrote to a bot with `pairing` on was given a code; here the owner lists the
//! waiting requests and approves or denies one, optionally as a member of the workspace (`--as`).
//! `link` / `unlink`: lets the bots speak to the hub as a member, who types their own password once;
//! only the device token the hub issues is kept. Nothing here needs a model key, and a running bot sees
//! the change from its next message.

use std::io::{IsTerminal, Write};
use std::path::Path;

use anyhow::{bail, Context};
use clap::Subcommand;
use warden_bootstrap::bot_access::{unix_now, BotHubSettings};
use warden_bootstrap::bot_hub::{self, HubTokens};
use warden_bootstrap::bot_pairing::{display_code, BotPairing, PairingRequest};
use warden_bootstrap::{load_config_from_path, save_config};

#[derive(Subcommand, Debug)]
pub enum BotsCommand {
    /// Pairing requests from people who wrote to a bot.
    Pair {
        #[command(subcommand)]
        action: PairAction,
    },
    /// Let the bots speak to the hub as a member: the member types their password once, here.
    Link {
        /// The member's username.
        member: String,
        /// The hub, `ws://host:port` (or `wss://`). Kept under [bot_hub] in config.toml; needed the first time.
        #[arg(long)]
        hub: Option<String>,
    },
    /// Forget a member's link to the hub. Their chats stop being answered until linked again.
    Unlink { member: String },
}

#[derive(Subcommand, Debug)]
pub enum PairAction {
    /// The requests waiting for an answer.
    List,
    /// Let the sender in: adds them to the bot's allow-list.
    Approve {
        /// The code the sender was given, with or without the dash.
        code: String,
        /// Have the chat speak as this member (linked with `warden bots link`): the hub answers it with
        /// their vault and tools instead of yours.
        #[arg(long = "as")]
        as_member: Option<String>,
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

/// What a password prompt gives back (a stand-in in the tests).
pub type AskPassword<'a> = &'a dyn Fn(&str) -> anyhow::Result<String>;

/// Reads a password without echoing it. Piped input (a script, the tests) is read as one plain line.
pub fn prompt_password(prompt: &str) -> anyhow::Result<String> {
    use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};

    if !std::io::stdin().is_terminal() {
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).context("reading the password")?;
        return Ok(line.trim_end_matches(['\r', '\n']).to_string());
    }
    /// Puts the terminal back however the prompt ends.
    struct RawMode;
    impl Drop for RawMode {
        fn drop(&mut self) {
            let _ = crossterm::terminal::disable_raw_mode();
        }
    }
    eprint!("{prompt}");
    std::io::stderr().flush()?;
    crossterm::terminal::enable_raw_mode()?;
    let _raw = RawMode;
    let mut password = String::new();
    loop {
        let Event::Key(key) = event::read()? else { continue };
        if key.kind == KeyEventKind::Release {
            continue;
        }
        match key.code {
            KeyCode::Enter => break,
            KeyCode::Backspace => {
                password.pop();
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                eprintln!();
                bail!("cancelled");
            }
            KeyCode::Char(c) => password.push(c),
            _ => {}
        }
    }
    eprintln!();
    Ok(password)
}

/// Runs `command` against the config at `config_path`, writing what it says to `out`.
pub async fn run(command: BotsCommand, config_path: Option<&Path>, out: &mut impl std::io::Write, ask_password: AskPassword<'_>) -> anyhow::Result<()> {
    let config_path = config_path.ok_or_else(|| anyhow::anyhow!("could not determine the OS config directory; pass --config"))?;
    let store = BotPairing::beside(config_path);
    let now = unix_now();
    match command {
        BotsCommand::Pair { action } => match action {
            PairAction::List => {
                let pending = store.list(now)?;
                if pending.is_empty() {
                    writeln!(out, "No pairing requests waiting. A bot only makes them with `pairing = true` under [telegram] or [whatsapp].")?;
                }
                for request in &pending {
                    writeln!(out, "{}", describe(request, now))?;
                }
            }
            PairAction::Approve { code, as_member } => {
                let request = store.approve_as(&code, now, config_path, as_member.as_deref())?;
                writeln!(out, "Approved: {} {} is on the {} allow-list now.", request.channel, request.sender, request.channel)?;
                if let Some(member) = as_member {
                    writeln!(out, "That chat speaks as {member}: the hub answers it with their vault and tools.")?;
                }
            }
            PairAction::Deny { code } => {
                let request = store.deny(&code, now)?;
                writeln!(out, "Denied: {} {}.", request.channel, request.sender)?;
            }
        },
        BotsCommand::Link { member, hub } => link(config_path, &member, hub.as_deref(), out, ask_password).await?,
        BotsCommand::Unlink { member } => {
            if HubTokens::beside(config_path).remove(&member)? {
                writeln!(out, "{member} is no longer linked. Chats that speak as them get a notice instead of an answer.")?;
            } else {
                writeln!(out, "{member} wasn't linked.")?;
            }
        }
    }
    Ok(())
}

/// Signs `member` in at the hub with the password they type and keeps the token the hub issues.
async fn link(config_path: &Path, member: &str, hub: Option<&str>, out: &mut impl std::io::Write, ask_password: AskPassword<'_>) -> anyhow::Result<()> {
    let mut config = load_config_from_path(config_path, false)?;
    let member = member.trim().to_ascii_lowercase();
    if !config.users.iter().any(|u| u.id == member) {
        bail!("there is no member '{member}' in this workspace: add them first (`warden-server users add`)");
    }
    let hub_url = hub
        .map(|url| url.trim().to_string())
        .filter(|url| !url.is_empty())
        .or_else(|| config.bot_hub.as_ref().map(|h| h.url.trim().to_string()).filter(|url| !url.is_empty()))
        .ok_or_else(|| anyhow::anyhow!("no hub to sign in at: pass --hub ws://host:port (it is kept under [bot_hub] in config.toml)"))?;
    if !hub_url.starts_with("ws://") && !hub_url.starts_with("wss://") {
        bail!("'{hub_url}' is not a hub address: write ws://host:port (or wss://host:port)");
    }

    let password = ask_password(&format!("Password of {member} (typed by them, not kept): "))?;
    let user = bot_hub::link(config_path, &hub_url, &member, &password).await?;
    // Only after it worked: a hub address that failed isn't worth remembering.
    if config.bot_hub.as_ref().map(|h| h.url.as_str()) != Some(hub_url.as_str()) {
        config.bot_hub = Some(BotHubSettings { url: hub_url.clone() });
        save_config(config_path, &config)?;
    }
    writeln!(out, "Linked {} ({member}) at {hub_url}. The password was not kept, only the hub's device token.", user.name)?;
    if user.must_change_password {
        writeln!(out, "Note: {member} still has the provisional password and must choose their own before the hub answers them.")?;
    }
    if user.locked {
        writeln!(out, "Note: their data is locked on the hub until they sign in there with their password (web, desktop or phone); until then their chats are told so.")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use warden_bootstrap::bot_pairing::{Issued, TELEGRAM};

    /// A password prompt that was never meant to be asked.
    fn no_password(_: &str) -> anyhow::Result<String> {
        panic!("no password expected")
    }

    async fn say(config: &Path, command: BotsCommand) -> anyhow::Result<String> {
        let mut out = Vec::new();
        run(command, Some(config), &mut out, &no_password).await.map(|()| String::from_utf8(out).unwrap())
    }

    fn scratch(name: &str, config: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("warden-cli-bots-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, config).unwrap();
        (dir, path)
    }

    #[tokio::test]
    async fn list_approve_and_deny_talk_to_the_files_the_bots_use() {
        let (dir, config) = scratch("pair", "");
        let store = BotPairing::beside(&config);
        let Issued::Fresh(code) = store.request(TELEGRAM, "42", "ana", unix_now()).unwrap() else { panic!("a fresh request") };
        let Issued::Fresh(other) = store.request(TELEGRAM, "43", "", unix_now()).unwrap() else { panic!("a fresh request") };

        let listed = say(&config, BotsCommand::Pair { action: PairAction::List }).await.unwrap();
        assert!(listed.contains(&display_code(&code)) && listed.contains("telegram 42 (ana)"), "{listed}");

        let approved = say(&config, BotsCommand::Pair { action: PairAction::Approve { code: display_code(&code).to_lowercase(), as_member: None } }).await.unwrap();
        assert!(approved.contains("telegram 42"), "{approved}");
        assert_eq!(load_config_from_path(&config, false).unwrap().telegram.allowed_users, [42]);

        say(&config, BotsCommand::Pair { action: PairAction::Deny { code: other } }).await.unwrap();
        assert!(say(&config, BotsCommand::Pair { action: PairAction::List }).await.unwrap().starts_with("No pairing requests"));
        assert!(say(&config, BotsCommand::Pair { action: PairAction::Approve { code, as_member: None } }).await.is_err(), "a code works once");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn approving_as_a_member_needs_a_linked_member_and_maps_the_chat() {
        let (dir, config) = scratch("approve-as", "");
        let mut file = load_config_from_path(&config, false).unwrap();
        warden_bootstrap::users::add_user(&mut file, "ana", "Ana", "provisional-pass").unwrap();
        save_config(&config, &file).unwrap();
        let store = BotPairing::beside(&config);
        let Issued::Fresh(code) = store.request(TELEGRAM, "42", "ana", unix_now()).unwrap() else { panic!("a fresh request") };
        let approve = |member: &str| BotsCommand::Pair { action: PairAction::Approve { code: code.clone(), as_member: Some(member.to_string()) } };

        let err = say(&config, approve("zed")).await.unwrap_err().to_string();
        assert!(err.contains("no member 'zed'"), "{err}");
        let err = say(&config, approve("ana")).await.unwrap_err().to_string();
        assert!(err.contains("[bot_hub]"), "no hub yet: {err}");
        std::fs::write(&config, format!("{}\n[bot_hub]\nurl = \"ws://127.0.0.1:7420\"\n", std::fs::read_to_string(&config).unwrap())).unwrap();
        let err = say(&config, approve("ana")).await.unwrap_err().to_string();
        assert!(err.contains("warden bots link ana"), "not linked yet: {err}");
        assert!(load_config_from_path(&config, false).unwrap().telegram.allowed_users.is_empty(), "a refusal changes nothing");
        assert_eq!(store.list(unix_now()).unwrap().len(), 1, "and the request stays");

        HubTokens::beside(&config).set("ana", bot_hub::Linked { device_id: "warden-bot-ana".into(), device_token: "t".into() }).unwrap();
        let approved = say(&config, approve("ana")).await.unwrap();
        assert!(approved.contains("speaks as ana"), "{approved}");
        let after = load_config_from_path(&config, false).unwrap();
        assert_eq!(after.telegram.allowed_users, [42]);
        assert_eq!(after.telegram.member_for(42), Some("ana"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn link_refuses_what_it_can_see_is_wrong_before_asking_for_a_password() {
        let (dir, config) = scratch("link", "");
        let mut file = load_config_from_path(&config, false).unwrap();
        warden_bootstrap::users::add_user(&mut file, "ana", "Ana", "provisional-pass").unwrap();
        save_config(&config, &file).unwrap();
        let link = |member: &str, hub: Option<&str>| BotsCommand::Link { member: member.to_string(), hub: hub.map(String::from) };

        let err = say(&config, link("zed", Some("ws://h:1"))).await.unwrap_err().to_string();
        assert!(err.contains("no member 'zed'"), "{err}");
        let err = say(&config, link("ana", None)).await.unwrap_err().to_string();
        assert!(err.contains("--hub"), "{err}");
        let err = say(&config, link("ana", Some("http://h:1"))).await.unwrap_err().to_string();
        assert!(err.contains("not a hub address"), "{err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn unlink_forgets_one_member_and_says_when_there_was_nothing() {
        let (dir, config) = scratch("unlink", "");
        HubTokens::beside(&config).set("ana", bot_hub::Linked { device_id: "d".into(), device_token: "t".into() }).unwrap();
        assert!(say(&config, BotsCommand::Unlink { member: "ana".into() }).await.unwrap().contains("no longer linked"));
        assert!(say(&config, BotsCommand::Unlink { member: "ana".into() }).await.unwrap().contains("wasn't linked"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
