//! The Usage screen's limits panel (P78): `spend_status` and `extend_spend_limit`.
//!
//! The Settings screen's `[[limits]]`/`[[prices]]` are saved through `save_settings` like agents
//! are, in the wire shapes of `warden_server_protocol` (`LimitSettingsDto`/`PriceSettingsDto`) and
//! checked by `warden_bootstrap::settings`, the same checks the hub's web settings run.

use serde::Serialize;
use tauri::State;
use warden_core::spend::SpendGuard;
use warden_server_protocol::protocol::{LimitStatusDto, RecentSpendDto};

use crate::AppState;

/// Where the spending limits stand, for the Usage screen (P78) — the same `LimitStatusDto` the web
/// UI gets from the hub, read from this app's own guard. `limits_enabled: false` means the app runs
/// with `WARDEN_SPEND_LIMITS=off` (no guard at all). `recent` (P10) is what the ledger holds, in dollars,
/// by model, channel, provider, agent and person — the same `RecentSpendDto` the web gets.
#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SpendStatusPayload {
    limits_enabled: bool,
    limits: Vec<LimitStatusDto>,
    recent: Option<RecentSpendDto>,
    ledger_error: Option<String>,
}

/// What the Usage screen shows of `guard` (`None`: the limits are switched off).
fn status_of(guard: Option<&SpendGuard>) -> SpendStatusPayload {
    SpendStatusPayload {
        limits_enabled: guard.is_some(),
        limits: guard.map(LimitStatusDto::all).unwrap_or_default(),
        recent: guard.map(|g| g.breakdown().into()),
        ledger_error: guard.and_then(SpendGuard::last_error),
    }
}

#[tauri::command]
pub fn spend_status(state: State<'_, AppState>) -> Result<SpendStatusPayload, String> {
    let orchestrator = { state.orchestrator.lock().unwrap().clone() }?;
    Ok(status_of(orchestrator.spend_guard().map(|guard| guard.as_ref())))
}

/// "Allow more" from the Usage screen: one `extend_step` for the rest of that limit's window — the
/// same grant the pause dialog makes mid-turn, for a limit that ran out between turns.
#[tauri::command]
pub fn extend_spend_limit(state: State<'_, AppState>, limit_id: String) -> Result<(), String> {
    let orchestrator = { state.orchestrator.lock().unwrap().clone() }?;
    let guard = orchestrator.spend_guard().ok_or_else(|| "spending limits are switched off".to_string())?;
    guard.extend(&limit_id).map(|_| ()).map_err(|e| format!("{e:#}"))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use warden_bootstrap::settings::limit_into_config;
    use warden_core::model::Usage;
    use warden_core::spend::{Limit, MemoryStore, Price, PriceTable, Scope, SpendContext, SpendGuard};
    use warden_server_protocol::protocol::LimitSettingsDto;

    use super::status_of;

    /// P10: the Usage screen gets dollars by provider, agent and person from the ledger, under the names the
    /// frontend reads; with the limits off there is no guard and nothing to show.
    #[test]
    fn the_status_carries_the_ledgers_dollars_under_the_names_the_frontend_reads() {
        let guard = SpendGuard::new(
            Arc::new(MemoryStore::default()),
            vec![Limit::new("day", Scope::Global, 24).with_max_tokens(10_000_000)],
            PriceTable::new(vec![Price { model: "m".into(), input_per_mtok: 2.0, output_per_mtok: 0.0 }]),
        );
        let usage = Usage { prompt_tokens: 1_000_000, completion_tokens: 0, total_tokens: 1_000_000 };
        guard.record_served(&SpendContext::new("desktop").with_agent(Some("writer".to_string())), "m", "main", &usage);

        let json = serde_json::to_value(status_of(Some(&guard))).unwrap();
        assert_eq!(json["limitsEnabled"], true);
        let recent = &json["recent"];
        assert_eq!(recent["windowHours"], 24);
        assert_eq!((recent["byProvider"][0]["key"].as_str(), recent["byProvider"][0]["costUsd"].as_f64()), (Some("main"), Some(2.0)));
        assert_eq!(recent["byAgent"][0]["key"], "writer");
        assert_eq!(recent["byPerson"][0]["key"], "", "the owner has no person: the empty key");
        assert_eq!(recent["byModel"][0]["unpricedCalls"], 0);

        let off = serde_json::to_value(status_of(None)).unwrap();
        assert_eq!((off["limitsEnabled"].clone(), off["recent"].clone()), (serde_json::json!(false), serde_json::Value::Null));
    }

    // The frontend (`desktop/src/types.ts`) sends limits in exactly this shape.
    #[test]
    fn the_wire_names_are_the_ones_the_frontend_uses() {
        let parsed: LimitSettingsDto = serde_json::from_value(serde_json::json!({
            "id": "a", "scope": "agent", "target": "pirate", "windowHours": 1,
            "maxTokens": null, "maxCostUsd": 2.5, "warnAt": null, "extendStep": 0.5
        }))
        .unwrap();
        let config = limit_into_config(parsed.clone()).unwrap();
        assert_eq!((config.max_cost_usd, config.extend_step), (Some(2.5), Some(0.5)));
        let json = serde_json::to_value(LimitSettingsDto::from(config)).unwrap();
        assert_eq!(json["windowHours"], 1);
        assert_eq!(json["scope"], "agent");
        assert!(json["maxTokens"].is_null());
    }
}
