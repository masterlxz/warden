//! The spending limits of P4 as `config.toml` describes them, and the guard built from them.
//!
//! ```toml
//! [[limits]]
//! id = "daily"
//! window_hours = 24          # a sliding window, not "since midnight"
//! max_tokens = 2000000
//! max_cost_usd = 5.0         # needs a [[prices]] entry for the model to count
//!
//! [[limits]]
//! id = "telegram-ana"
//! scope = "user"             # global (default) | agent | channel | user
//! target = "telegram:12345"  # who: an agent id, a channel name, or channel:user
//! window_hours = 1
//! max_tokens = 100000
//!
//! [[prices]]
//! model = "gpt-4o-mini"
//! input_per_mtok = 0.15
//! output_per_mtok = 0.60
//! ```
//!
//! No `[[limits]]` at all means the built-in safety net (`default_limits`); `limits = []` means
//! none. There are no built-in prices — see `warden_core::spend::Price`.

use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use warden_core::spend::{FileStore, Limit, MemoryStore, Price, PriceTable, Scope, SpendGuard, SpendStore, DEFAULT_EXTEND_STEP, DEFAULT_WARN_AT};

use crate::FileConfig;

/// The safety net for someone who never configured a limit: generous enough not to get in the way of
/// normal use, low enough that an agent stuck in a loop hits it within the hour. A person who hits
/// it is asked whether to allow more (desktop, CLI) rather than being locked out.
pub const DEFAULT_HOUR_TOKENS: u64 = 500_000;
pub const DEFAULT_DAY_TOKENS: u64 = 2_000_000;

#[derive(Deserialize, Serialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LimitScope {
    #[default]
    Global,
    Agent,
    Channel,
    User,
}

/// One `[[limits]]` entry.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LimitConfig {
    pub id: String,
    #[serde(default)]
    pub scope: LimitScope,
    /// Who the limit is about: an agent id, a channel name (`desktop`, `cli`, `telegram`, `whatsapp`,
    /// `server`) or `channel:user` (`telegram:12345`). Left out for `global`.
    #[serde(default)]
    pub target: Option<String>,
    pub window_hours: u32,
    #[serde(default)]
    pub max_tokens: Option<u64>,
    #[serde(default)]
    pub max_cost_usd: Option<f64>,
    /// Fraction (0–1) of the ceiling from which the model is told how much is left. Default 0.8.
    #[serde(default)]
    pub warn_at: Option<f64>,
    /// Fraction of the ceiling one extension adds when a paused turn is allowed to go on. Default 0.25.
    #[serde(default)]
    pub extend_step: Option<f64>,
}

impl LimitConfig {
    pub fn to_limit(&self) -> anyhow::Result<Limit> {
        let target = self.target.as_deref().map(str::trim).filter(|t| !t.is_empty());
        let scope = match (self.scope, target) {
            (LimitScope::Global, None) => Scope::Global,
            (LimitScope::Global, Some(_)) => anyhow::bail!("limit '{}': a global limit takes no target", self.id),
            (LimitScope::Agent, Some(t)) => Scope::Agent(t.to_string()),
            (LimitScope::Channel, Some(t)) => Scope::Channel(t.to_string()),
            (LimitScope::User, Some(t)) => Scope::User(t.to_string()),
            (_, None) => anyhow::bail!("limit '{}': scope {:?} needs a target", self.id, self.scope),
        };
        let limit = Limit {
            id: self.id.trim().to_string(),
            scope,
            window_hours: self.window_hours,
            max_tokens: self.max_tokens,
            max_cost_usd: self.max_cost_usd,
            warn_at: self.warn_at.unwrap_or(DEFAULT_WARN_AT),
            extend_step: self.extend_step.unwrap_or(DEFAULT_EXTEND_STEP),
        };
        limit.validate()?;
        Ok(limit)
    }
}

pub fn default_limits() -> Vec<Limit> {
    vec![
        Limit::new("default-hour", Scope::Global, 1).with_max_tokens(DEFAULT_HOUR_TOKENS),
        Limit::new("default-day", Scope::Global, 24).with_max_tokens(DEFAULT_DAY_TOKENS),
    ]
}

/// The limits in force, and what was wrong with the entries that were left out.
#[derive(Debug, Default, PartialEq)]
pub struct ResolvedLimits {
    pub limits: Vec<Limit>,
    pub notes: Vec<String>,
}

/// Decides which limits apply. `WARDEN_SPEND_LIMITS=off` (also `0`, `false`, `no`, `none`) switches
/// every limit off, whatever the file says; otherwise the file's `[[limits]]` are used, or
/// `default_limits` when it has none. A broken entry is skipped with a note instead of stopping
/// startup, so one typo doesn't lock the user out of the app that would let them fix it.
pub fn resolve_limits(from_env: Option<String>, from_file: Option<&[LimitConfig]>) -> ResolvedLimits {
    let off = from_env.is_some_and(|v| matches!(v.trim().to_lowercase().as_str(), "off" | "0" | "false" | "no" | "none"));
    if off {
        return ResolvedLimits::default();
    }
    let Some(entries) = from_file else {
        return ResolvedLimits { limits: default_limits(), notes: Vec::new() };
    };
    let mut resolved = ResolvedLimits::default();
    for entry in entries {
        match entry.to_limit() {
            Ok(limit) if resolved.limits.iter().any(|l| l.id == limit.id) => {
                resolved.notes.push(format!("limit '{}' skipped — another limit already uses that id", limit.id))
            }
            Ok(limit) => resolved.limits.push(limit),
            Err(err) => resolved.notes.push(format!("limit skipped — {err:#}")),
        }
    }
    resolved
}

/// What to tell someone in a chat app when their message failed: the plain reason for a spending
/// limit (they can act on it — wait, or ask the owner), the usual apology for anything else. The
/// details of a failure other than this one stay in the log, not in the chat.
pub fn chat_error_reply(err: &anyhow::Error) -> String {
    match err.downcast_ref::<warden_core::budget::SpendLimitReached>() {
        Some(reached) => {
            let back = reached.0.frees_up_in_minutes.map(|m| format!(" It should free up in about {m} minutes.")).unwrap_or_default();
            format!("I've reached my spending limit ('{}') for now, so I can't answer.{back}", reached.0.id)
        }
        None => "Sorry, something went wrong handling your message.".to_string(),
    }
}

/// Prices with a usable number; the rest are reported.
fn usable_prices(prices: &[Price]) -> (Vec<Price>, Vec<String>) {
    let mut notes = Vec::new();
    let ok = |n: f64| n.is_finite() && n >= 0.0;
    let kept = prices
        .iter()
        .filter(|p| {
            let good = !p.model.trim().is_empty() && ok(p.input_per_mtok) && ok(p.output_per_mtok);
            if !good {
                notes.push(format!("price for '{}' skipped — model and non-negative prices are required", p.model));
            }
            good
        })
        .cloned()
        .collect();
    (kept, notes)
}

/// Where the ledger of what was spent lives, next to `config.toml`. Shared by every Warden process
/// on the machine (desktop, CLI, Telegram bot...) so they draw from one account.
pub fn default_spend_ledger_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("warden").join("spend_ledger.jsonl"))
}

/// `WARDEN_SPEND_LEDGER` (a file path) wins over the default location.
pub fn resolve_ledger_path(from_env: Option<String>) -> Option<PathBuf> {
    from_env.map(|v| v.trim().to_string()).filter(|v| !v.is_empty()).map(PathBuf::from).or_else(default_spend_ledger_path)
}

/// The guard `bootstrap()` hands the orchestrator, or `None` when no limit is in force.
pub fn build_spend_guard(config: &FileConfig) -> Option<Arc<SpendGuard>> {
    build_spend_guard_with(
        config,
        std::env::var("WARDEN_SPEND_LIMITS").ok(),
        resolve_ledger_path(std::env::var("WARDEN_SPEND_LEDGER").ok()),
    )
}

pub fn build_spend_guard_with(config: &FileConfig, limits_env: Option<String>, ledger: Option<PathBuf>) -> Option<Arc<SpendGuard>> {
    let ResolvedLimits { limits, mut notes } = resolve_limits(limits_env, config.limits.as_deref());
    let (prices, price_notes) = usable_prices(&config.prices);
    notes.extend(price_notes);
    for note in &notes {
        eprintln!("note: {note}\n");
    }
    if limits.is_empty() {
        return None;
    }
    let longest = limits.iter().map(|l| l.window_hours).max().unwrap_or(1);
    let store: Arc<dyn SpendStore> = match ledger {
        Some(path) => Arc::new(FileStore::open(path, longest)),
        None => {
            eprintln!("note: no config directory found — spending is counted for this run only, not remembered\n");
            Arc::new(MemoryStore::default())
        }
    };
    Some(Arc::new(SpendGuard::new(store, limits, PriceTable::new(prices))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use warden_core::model::Usage;
    use warden_core::spend::SpendContext;

    fn entry(id: &str) -> LimitConfig {
        LimitConfig {
            id: id.into(),
            scope: LimitScope::Global,
            target: None,
            window_hours: 24,
            max_tokens: Some(1000),
            max_cost_usd: None,
            warn_at: None,
            extend_step: None,
        }
    }

    #[test]
    fn an_entry_becomes_a_limit_with_the_default_marks() {
        let limit = entry("day").to_limit().unwrap();
        assert_eq!((limit.scope.clone(), limit.warn_at, limit.extend_step), (Scope::Global, DEFAULT_WARN_AT, DEFAULT_EXTEND_STEP));

        let mut user = entry("ana");
        user.scope = LimitScope::User;
        user.target = Some(" telegram:42 ".into());
        user.warn_at = Some(0.5);
        let limit = user.to_limit().unwrap();
        assert_eq!((limit.scope, limit.warn_at), (Scope::User("telegram:42".into()), 0.5));
    }

    #[test]
    fn scope_and_target_have_to_agree() {
        let mut global_with_target = entry("g");
        global_with_target.target = Some("cli".into());
        assert!(global_with_target.to_limit().unwrap_err().to_string().contains("takes no target"));

        let mut agent_without_target = entry("a");
        agent_without_target.scope = LimitScope::Agent;
        assert!(agent_without_target.to_limit().unwrap_err().to_string().contains("needs a target"));

        let mut bad_user = entry("u");
        bad_user.scope = LimitScope::User;
        bad_user.target = Some("no-colon".into());
        assert!(bad_user.to_limit().is_err());
    }

    #[test]
    fn no_limits_in_the_file_means_the_safety_net_and_an_empty_list_means_none() {
        let net = resolve_limits(None, None);
        assert_eq!(net.limits.iter().map(|l| l.id.as_str()).collect::<Vec<_>>(), ["default-hour", "default-day"]);
        assert!(resolve_limits(None, Some(&[])).limits.is_empty());
    }

    #[test]
    fn the_env_switch_turns_everything_off_and_other_values_are_ignored() {
        for off in ["off", "0", "FALSE", " no "] {
            assert!(resolve_limits(Some(off.into()), Some(&[entry("x")])).limits.is_empty(), "{off}");
        }
        assert_eq!(resolve_limits(Some("nonsense".into()), Some(&[entry("x")])).limits.len(), 1);
        assert_eq!(resolve_limits(Some("on".into()), None).limits.len(), 2);
    }

    #[test]
    fn a_broken_or_repeated_entry_is_skipped_with_a_note_and_the_rest_stay() {
        let mut broken = entry("broken");
        broken.window_hours = 0;
        let resolved = resolve_limits(None, Some(&[entry("a"), broken, entry("a"), entry("b")]));
        assert_eq!(resolved.limits.iter().map(|l| l.id.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!(resolved.notes.len(), 2);
        assert!(resolved.notes[0].contains("window_hours"), "{:?}", resolved.notes);
        assert!(resolved.notes[1].contains("already uses that id"), "{:?}", resolved.notes);
    }

    #[test]
    fn limits_and_prices_parse_from_toml_and_survive_a_save() {
        let text = r#"
            [[limits]]
            id = "daily"
            window_hours = 24
            max_tokens = 2000000
            max_cost_usd = 5.0

            [[limits]]
            id = "ana"
            scope = "user"
            target = "telegram:42"
            window_hours = 1
            max_tokens = 100000
            extend_step = 0.1

            [[prices]]
            model = "gpt-4o-mini"
            input_per_mtok = 0.15
            output_per_mtok = 0.6
        "#;
        let config: FileConfig = toml::from_str(text).unwrap();
        let limits = config.limits.as_deref().unwrap();
        assert_eq!((limits.len(), limits[1].scope, limits[0].max_cost_usd), (2, LimitScope::User, Some(5.0)));
        assert_eq!(config.prices[0].model, "gpt-4o-mini");

        let again: FileConfig = toml::from_str(&toml::to_string_pretty(&config).unwrap()).unwrap();
        assert_eq!(again, config);

        // `limits = []` is a deliberate "none", and survives a save as such rather than turning
        // back into the defaults.
        let none: FileConfig = toml::from_str("limits = []").unwrap();
        let none_again: FileConfig = toml::from_str(&toml::to_string_pretty(&none).unwrap()).unwrap();
        assert_eq!(none_again.limits, Some(Vec::new()));
        assert_eq!(toml::from_str::<FileConfig>("").unwrap().limits, None);
    }

    #[test]
    fn an_unknown_field_in_a_limit_is_an_error_not_a_silently_ignored_typo() {
        let err = toml::from_str::<FileConfig>("[[limits]]\nid = \"a\"\nwindow_hours = 1\nmax_token = 5\n").unwrap_err();
        assert!(err.to_string().contains("max_token"), "{err}");
    }

    fn temp_ledger() -> PathBuf {
        std::env::temp_dir()
            .join(format!("warden-spend-cfg-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()))
            .join("ledger.jsonl")
    }

    #[test]
    fn the_guard_counts_in_a_ledger_file_and_prices_only_what_the_config_lists() {
        let config: FileConfig = toml::from_str(
            r#"
            [[limits]]
            id = "money"
            window_hours = 24
            max_cost_usd = 1.0

            [[prices]]
            model = "m"
            input_per_mtok = 1000.0
            output_per_mtok = 1000.0

            [[prices]]
            model = "broken"
            input_per_mtok = -1.0
            output_per_mtok = 1.0
        "#,
        )
        .unwrap();
        let path = temp_ledger();
        let guard = build_spend_guard_with(&config, None, Some(path.clone())).expect("one limit is in force");

        let ctx = SpendContext::new("cli");
        guard.record(&ctx, "m", &Usage { prompt_tokens: 300, completion_tokens: 200, total_tokens: 500 });
        guard.record(&ctx, "broken", &Usage { prompt_tokens: 1_000_000, completion_tokens: 0, total_tokens: 1_000_000 });

        let status = &guard.status(None)[0];
        assert!((status.used_cost_usd - 0.5).abs() < 1e-9, "{status:?}");
        assert_eq!(status.unpriced_calls, 1, "the negative price was dropped, so that model is unpriced");
        assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 2);
    }

    #[test]
    fn no_limit_in_force_means_no_guard_at_all() {
        let config = FileConfig::default();
        assert!(build_spend_guard_with(&config, Some("off".into()), None).is_none());
        assert!(build_spend_guard_with(&config, None, Some(temp_ledger())).is_some(), "the defaults apply");
    }

    #[test]
    fn a_chat_user_is_told_about_a_spending_limit_and_nothing_else() {
        let status = warden_core::spend::LimitStatus {
            id: "ana".into(),
            scope: "user telegram:42".into(),
            window_hours: 1,
            used_tokens: 10,
            max_tokens: Some(10),
            remaining_tokens: Some(0),
            used_cost_usd: 0.0,
            max_cost_usd: None,
            remaining_cost_usd: None,
            fraction: 1.0,
            warn: true,
            exceeded: true,
            unpriced_calls: 0,
            frees_up_in_minutes: Some(12),
        };
        let reply = chat_error_reply(&anyhow::Error::new(warden_core::budget::SpendLimitReached(Box::new(status))));
        assert!(reply.contains("'ana'") && reply.contains("12 minutes"), "{reply}");
        assert!(!reply.contains("10"), "no usage numbers in a chat: {reply}");
        assert_eq!(chat_error_reply(&anyhow::anyhow!("boom: secret detail")), "Sorry, something went wrong handling your message.");
    }

    #[test]
    fn the_ledger_path_env_wins_over_the_default() {
        assert_eq!(resolve_ledger_path(Some(" /tmp/x.jsonl ".into())), Some(PathBuf::from("/tmp/x.jsonl")));
        assert_eq!(resolve_ledger_path(Some("  ".into())), default_spend_ledger_path());
    }
}
