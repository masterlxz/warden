//! The spending half of the Settings screen (P4): the IPC shapes for `[[limits]]` and `[[prices]]`
//! and the checks `save_settings` runs on them. Same split as `ssh_cmds.rs` — the values themselves
//! are saved through `save_settings` like agents are.
//!
//! Unlike startup (where a broken entry is skipped with a note so a typo can't lock the user out of
//! the app), a save **refuses** a broken entry: the person is right there and can fix it. What it
//! deliberately does not check is whether an agent a limit names still exists — an agent can be
//! deleted (by the agent-manager tool, or on this very screen) and the limit left behind is harmless;
//! refusing every later save over it would be the dangling-reference trap the SSH hosts had.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use warden_bootstrap::{LimitConfig, LimitScope};
use warden_core::spend::Price;

/// One `[[limits]]` entry as the form edits it. `target` is an empty string for a global limit — the
/// "not set is an empty string" convention every other payload here uses. `warn_at`/`extend_step` are
/// fractions (0–1), `None` meaning the default.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LimitPayload {
    pub id: String,
    pub scope: LimitScope,
    pub target: String,
    pub window_hours: u32,
    pub max_tokens: Option<u64>,
    pub max_cost_usd: Option<f64>,
    pub warn_at: Option<f64>,
    pub extend_step: Option<f64>,
}

impl From<LimitConfig> for LimitPayload {
    fn from(c: LimitConfig) -> Self {
        Self {
            id: c.id,
            scope: c.scope,
            target: c.target.unwrap_or_default(),
            window_hours: c.window_hours,
            max_tokens: c.max_tokens,
            max_cost_usd: c.max_cost_usd,
            warn_at: c.warn_at,
            extend_step: c.extend_step,
        }
    }
}

impl LimitPayload {
    /// Trims, then runs the very validation the guard applies at startup (`LimitConfig::to_limit`),
    /// so what is saved is what will be enforced rather than skipped with a note at the next launch.
    pub fn into_config(self) -> Result<LimitConfig, String> {
        let config = LimitConfig {
            id: self.id.trim().to_string(),
            scope: self.scope,
            target: Some(self.target.trim().to_string()).filter(|t| !t.is_empty()),
            window_hours: self.window_hours,
            max_tokens: self.max_tokens,
            max_cost_usd: self.max_cost_usd,
            warn_at: self.warn_at,
            extend_step: self.extend_step,
        };
        config.to_limit().map_err(|e| format!("{e:#}"))?;
        Ok(config)
    }
}

/// The whole list, with the cross-entry check: ids unique.
pub fn limits_into_config(payloads: Vec<LimitPayload>) -> Result<Vec<LimitConfig>, String> {
    let mut seen = HashSet::new();
    let mut limits = Vec::with_capacity(payloads.len());
    for payload in payloads {
        let limit = payload.into_config()?;
        if !seen.insert(limit.id.clone()) {
            return Err(format!("duplicate limit name: {}", limit.id));
        }
        limits.push(limit);
    }
    Ok(limits)
}

/// One `[[prices]]` entry: dollars per million tokens, for the model with exactly this id.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PricePayload {
    pub model: String,
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
}

impl From<Price> for PricePayload {
    fn from(p: Price) -> Self {
        Self { model: p.model, input_per_mtok: p.input_per_mtok, output_per_mtok: p.output_per_mtok }
    }
}

/// Trims each model id and refuses an empty or repeated one and any price that is not a number of
/// zero or more (a free model is a fine price; a negative one is a typo).
pub fn prices_into_config(payloads: Vec<PricePayload>) -> Result<Vec<Price>, String> {
    let mut seen = HashSet::new();
    let mut prices = Vec::with_capacity(payloads.len());
    for payload in payloads {
        let model = payload.model.trim().to_string();
        if model.is_empty() {
            return Err("every price needs a model id".to_string());
        }
        let ok = |n: f64| n.is_finite() && n >= 0.0;
        if !ok(payload.input_per_mtok) || !ok(payload.output_per_mtok) {
            return Err(format!("price for '{model}': prices must be numbers of 0 or more"));
        }
        if !seen.insert(model.clone()) {
            return Err(format!("duplicate price for model: {model}"));
        }
        prices.push(Price { model, input_per_mtok: payload.input_per_mtok, output_per_mtok: payload.output_per_mtok });
    }
    Ok(prices)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limit(id: &str) -> LimitPayload {
        LimitPayload {
            id: id.into(),
            scope: LimitScope::Global,
            target: String::new(),
            window_hours: 24,
            max_tokens: Some(1_000),
            max_cost_usd: None,
            warn_at: None,
            extend_step: None,
        }
    }

    fn price(model: &str) -> PricePayload {
        PricePayload { model: model.into(), input_per_mtok: 3.0, output_per_mtok: 15.0 }
    }

    #[test]
    fn a_limit_round_trips_through_the_form_shape_without_losing_a_field() {
        let config = LimitConfig {
            id: "ana".into(),
            scope: LimitScope::User,
            target: Some("telegram:42".into()),
            window_hours: 6,
            max_tokens: Some(50_000),
            max_cost_usd: Some(1.5),
            warn_at: Some(0.5),
            extend_step: Some(0.1),
        };
        let back = LimitPayload::from(config.clone()).into_config().unwrap();
        assert_eq!(back, config);

        let global = LimitConfig { scope: LimitScope::Global, target: None, ..config };
        assert_eq!(LimitPayload::from(global.clone()).target, "", "no target reaches the form as an empty string");
        assert_eq!(LimitPayload::from(global.clone()).into_config().unwrap(), global);
    }

    #[test]
    fn the_wire_names_are_the_ones_the_frontend_uses() {
        let json = serde_json::to_value(limit("day")).unwrap();
        assert_eq!(json["windowHours"], 24);
        assert_eq!(json["scope"], "global");
        assert_eq!(json["maxTokens"], 1_000);
        assert!(json["maxCostUsd"].is_null());
        let parsed: LimitPayload = serde_json::from_value(serde_json::json!({
            "id": "a", "scope": "agent", "target": "pirate", "windowHours": 1,
            "maxTokens": null, "maxCostUsd": 2.5, "warnAt": null, "extendStep": 0.5
        }))
        .unwrap();
        assert_eq!((parsed.scope, parsed.max_cost_usd, parsed.extend_step), (LimitScope::Agent, Some(2.5), Some(0.5)));
    }

    #[test]
    fn a_limit_is_trimmed_and_checked_with_the_same_rules_as_startup() {
        let mut padded = limit("  day  ");
        padded.scope = LimitScope::Channel;
        padded.target = " telegram ".into();
        let config = padded.into_config().unwrap();
        assert_eq!((config.id.as_str(), config.target.as_deref()), ("day", Some("telegram")));

        let mut no_ceiling = limit("x");
        no_ceiling.max_tokens = None;
        assert!(no_ceiling.into_config().unwrap_err().contains("max_tokens and/or max_cost_usd"));

        let mut no_window = limit("x");
        no_window.window_hours = 0;
        assert!(no_window.into_config().unwrap_err().contains("window_hours"));

        let mut global_with_target = limit("x");
        global_with_target.target = "cli".into();
        assert!(global_with_target.into_config().unwrap_err().contains("takes no target"));

        let mut agent_without_target = limit("x");
        agent_without_target.scope = LimitScope::Agent;
        assert!(agent_without_target.into_config().unwrap_err().contains("needs a target"));

        let mut warn_over_one = limit("x");
        warn_over_one.warn_at = Some(1.5);
        assert!(warn_over_one.into_config().unwrap_err().contains("warn_at"));

        assert!(limit("   ").into_config().unwrap_err().contains("needs an id"));
    }

    #[test]
    fn the_list_refuses_a_repeated_name_and_stops_at_the_first_bad_entry() {
        assert_eq!(limits_into_config(vec![limit("a"), limit("b")]).unwrap().len(), 2);
        assert!(limits_into_config(vec![limit("a"), limit(" a ")]).unwrap_err().contains("duplicate limit name: a"));
        let mut bad = limit("b");
        bad.max_tokens = None;
        assert!(limits_into_config(vec![limit("a"), bad]).is_err());
        assert!(limits_into_config(vec![]).unwrap().is_empty(), "an empty list is a valid way to say every limit is off");
    }

    #[test]
    fn a_limit_on_an_agent_that_no_longer_exists_is_still_saved() {
        let mut orphan = limit("ghost-cap");
        orphan.scope = LimitScope::Agent;
        orphan.target = "deleted-agent".into();
        assert!(limits_into_config(vec![orphan]).is_ok());
    }

    #[test]
    fn prices_need_a_model_a_number_of_zero_or_more_and_no_repeats() {
        let prices = prices_into_config(vec![price(" gpt-4o-mini "), price("claude")]).unwrap();
        assert_eq!(prices[0].model, "gpt-4o-mini");

        assert!(prices_into_config(vec![price("  ")]).unwrap_err().contains("model id"));
        assert!(prices_into_config(vec![price("m"), price("m")]).unwrap_err().contains("duplicate price"));
        for bad in [-1.0, f64::NAN, f64::INFINITY] {
            let mut p = price("m");
            p.input_per_mtok = bad;
            assert!(prices_into_config(vec![p]).is_err(), "{bad}");
        }
        let free = PricePayload { model: "local".into(), input_per_mtok: 0.0, output_per_mtok: 0.0 };
        assert_eq!(prices_into_config(vec![free]).unwrap().len(), 1, "free is a price");
    }
}
