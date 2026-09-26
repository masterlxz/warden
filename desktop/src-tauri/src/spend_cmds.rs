//! The Usage screen's limits panel (P78): `spend_status` and `extend_spend_limit`.
//!
//! The Settings screen's `[[limits]]`/`[[prices]]` are saved through `save_settings` like agents
//! are, in the wire shapes of `warden_server_protocol` (`LimitSettingsDto`/`PriceSettingsDto`) and
//! checked by `warden_bootstrap::settings`, the same checks the hub's web settings run.

use serde::Serialize;
use tauri::State;
use warden_server_protocol::protocol::LimitStatusDto;

use crate::AppState;

/// Where the spending limits stand, for the Usage screen (P78) — the same `LimitStatusDto` the web
/// UI gets from the hub, read from this app's own guard. `limits_enabled: false` means the app runs
/// with `WARDEN_SPEND_LIMITS=off` (no guard at all).
#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SpendStatusPayload {
    limits_enabled: bool,
    limits: Vec<LimitStatusDto>,
    ledger_error: Option<String>,
}

#[tauri::command]
pub fn spend_status(state: State<'_, AppState>) -> Result<SpendStatusPayload, String> {
    let orchestrator = { state.orchestrator.lock().unwrap().clone() }?;
    let guard = orchestrator.spend_guard();
    Ok(SpendStatusPayload {
        limits_enabled: guard.is_some(),
        limits: guard.map(|g| LimitStatusDto::all(g)).unwrap_or_default(),
        ledger_error: guard.and_then(|g| g.last_error()),
    })
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
    use warden_bootstrap::settings::limit_into_config;
    use warden_server_protocol::protocol::LimitSettingsDto;

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
