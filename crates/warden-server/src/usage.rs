//! Usage over the wire (P78) — what `RequestUsage`/`ExtendLimit` answer. Tokens come from every
//! conversation this hub keeps, for every device and all time (`aggregate_usage`/`daily_usage`
//! from `warden-bootstrap`); limits and dollars come from the P4 ledger through the `SpendGuard`,
//! which is shared with every other channel on this machine. Pure functions over paths and the
//! guard, like `skills.rs`, so they're testable without a socket.

use std::collections::HashMap;
use std::path::Path;

use warden_bootstrap::usage::daily_usage;
use warden_bootstrap::{aggregate_usage, list_conversations, Conversation};
use warden_core::spend::SpendGuard;
use warden_server_protocol::protocol::{DailyUsageDto, DeviceUsage, LimitStatusDto, UsageReportDto};
use warden_server_protocol::ServerMessage;

use crate::device_registry::PairingStore;

/// How many days the daily series covers, today included.
pub const USAGE_DAYS: u32 = 30;

/// Everything `RequestUsage` reports. `conversations_root` holds one folder per device (P78) and,
/// for devices that haven't reconnected since, their old single `<device_id>.json`.
pub fn build_usage_report(
    conversations_root: &Path,
    pairing: &PairingStore,
    guard: Option<&SpendGuard>,
    tz_offset_minutes: i32,
    now_ms: i64,
) -> anyhow::Result<UsageReportDto> {
    let names: HashMap<String, String> = pairing.list().unwrap_or_default().into_iter().map(|(id, d)| (id, d.device_name)).collect();

    let mut per_device: Vec<(String, Vec<Conversation>)> = Vec::new();
    match std::fs::read_dir(conversations_root) {
        Ok(entries) => {
            for entry in entries {
                let path = entry?.path();
                if path.is_dir() {
                    let device_id = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                    per_device.push((device_id, list_conversations(&path)?));
                }
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err.into()),
    }
    for legacy in list_conversations(conversations_root)? {
        match per_device.iter_mut().find(|(id, _)| *id == legacy.id) {
            Some((_, conversations)) => conversations.push(legacy),
            None => per_device.push((legacy.id.clone(), vec![legacy])),
        }
    }

    let mut by_device: Vec<DeviceUsage> = per_device
        .iter()
        .map(|(device_id, conversations)| {
            let summary = aggregate_usage(conversations);
            DeviceUsage {
                device_id: device_id.clone(),
                name: names.get(device_id).cloned(),
                conversation_count: summary.conversation_count,
                message_count: summary.message_count,
                usage: summary.total,
            }
        })
        .filter(|d| d.conversation_count > 0)
        .collect();
    by_device.sort_by_key(|d| std::cmp::Reverse(d.usage.total_tokens));

    let all: Vec<Conversation> = per_device.into_iter().flat_map(|(_, c)| c).collect();
    let summary = aggregate_usage(&all);
    let daily = daily_usage(&all, USAGE_DAYS, tz_offset_minutes, now_ms)
        .into_iter()
        .map(|d| DailyUsageDto { date: d.date, calls: d.calls, tokens: d.tokens })
        .collect();

    Ok(UsageReportDto {
        total: summary.total,
        conversation_count: summary.conversation_count,
        message_count: summary.message_count,
        by_device,
        daily,
        limits_enabled: guard.is_some(),
        limits: guard.map(LimitStatusDto::all).unwrap_or_default(),
        recent: guard.map(|g| g.breakdown().into()),
        ledger_error: guard.and_then(SpendGuard::last_error),
    })
}

/// Answers `RequestUsage`.
pub fn handle_usage_request(conversations_root: &Path, pairing: &PairingStore, guard: Option<&SpendGuard>, request_id: u64, tz_offset_minutes: i32) -> ServerMessage {
    let now = warden_core::spend::now_millis() as i64;
    match build_usage_report(conversations_root, pairing, guard, tz_offset_minutes, now) {
        Ok(report) => ServerMessage::UsageReport { request_id, report },
        Err(err) => ServerMessage::UsageError { request_id, message: format!("{err:#}") },
    }
}

/// Answers `ExtendLimit`: one `extend_step` more for that limit, and where it stands after.
pub fn handle_extend_limit(guard: Option<&SpendGuard>, request_id: u64, limit_id: &str) -> ServerMessage {
    let Some(guard) = guard else {
        return ServerMessage::UsageError { request_id, message: "spending limits are switched off on this hub".into() };
    };
    let extended = guard.extend(limit_id).and_then(|_| {
        LimitStatusDto::all(guard).into_iter().find(|l| l.id == limit_id).ok_or_else(|| anyhow::anyhow!("no limit named '{limit_id}'"))
    });
    match extended {
        Ok(limit) => ServerMessage::LimitExtended { request_id, limit },
        Err(err) => ServerMessage::UsageError { request_id, message: format!("{err:#}") },
    }
}

/// The id of the spending limit a failed turn stopped on, if that's why it failed (P4) — so the
/// client can offer `ExtendLimit`.
pub fn spend_limit_id(err: &anyhow::Error) -> Option<String> {
    err.downcast_ref::<warden_core::budget::SpendLimitReached>().map(|reached| reached.0.id.clone())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use warden_bootstrap::{save_conversation, ChatRole, ConversationMessage};
    use warden_core::model::Usage;
    use warden_core::spend::{Limit, MemoryStore, Price, PriceTable, Scope, SpendContext};

    use super::*;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "warden-server-usage-{name}-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ))
    }

    fn conversation(id: &str, tokens: &[u32], created_at: i64) -> Conversation {
        let messages = tokens
            .iter()
            .map(|&t| ConversationMessage {
                id: "m".into(),
                role: ChatRole::Assistant,
                content: String::new(),
                created_at,
                usage: Some(Usage { prompt_tokens: t, completion_tokens: 0, total_tokens: t }),
                attachments: Vec::new(),
                generated_files: Vec::new(),
            })
            .collect();
        Conversation { id: id.into(), title: id.into(), messages, created_at, updated_at: created_at, agent_id: None, provider_id: None }
    }

    fn guard(max_tokens: u64) -> SpendGuard {
        SpendGuard::new(
            Arc::new(MemoryStore::default()),
            vec![Limit::new("day", Scope::Global, 24).with_max_tokens(max_tokens)],
            PriceTable::new(vec![Price { model: "m".into(), input_per_mtok: 1.0, output_per_mtok: 1.0 }]),
        )
    }

    #[test]
    fn reports_every_device_with_its_registry_name_including_an_old_single_file() {
        let root = temp_dir("root");
        let now = warden_core::spend::now_millis() as i64;
        save_conversation(&root.join("phone"), &conversation("default", &[10, 20], now)).unwrap();
        save_conversation(&root.join("phone"), &conversation("trip", &[5], now)).unwrap();
        save_conversation(&root.join("web-1"), &conversation("default", &[100], now)).unwrap();
        save_conversation(&root, &conversation("old-laptop", &[1], now)).unwrap(); // pre-P78 layout
        std::fs::create_dir_all(root.join("empty")).unwrap();

        let pairing = PairingStore::new(temp_dir("devices").join("devices.json"));
        pairing.authenticate("web-1", "Browser", None, true).unwrap().unwrap();

        let report = build_usage_report(&root, &pairing, None, 0, now).unwrap();
        assert_eq!(report.total.total_tokens, 136);
        assert_eq!((report.conversation_count, report.message_count), (4, 5));
        let devices: Vec<_> = report.by_device.iter().map(|d| (d.device_id.as_str(), d.name.as_deref(), d.conversation_count, d.usage.total_tokens)).collect();
        assert_eq!(devices, vec![("web-1", Some("Browser"), 1, 100), ("phone", None, 2, 35), ("old-laptop", None, 1, 1)]);
        assert_eq!(report.daily.len(), USAGE_DAYS as usize);
        assert_eq!(report.daily.last().unwrap().tokens, 136);
        assert!(!report.limits_enabled && report.limits.is_empty() && report.recent.is_none());
    }

    #[test]
    fn a_hub_with_no_conversations_yet_reports_zeros() {
        let pairing = PairingStore::new(temp_dir("devices").join("devices.json"));
        let report = build_usage_report(&temp_dir("missing"), &pairing, None, 0, 0).unwrap();
        assert_eq!((report.conversation_count, report.by_device.len()), (0, 0));
    }

    #[test]
    fn limits_and_recent_spending_come_from_the_guard_and_extend_raises_the_ceiling() {
        let guard = guard(100);
        guard.record(&SpendContext::new("server").with_user("web-1"), "m", &Usage { prompt_tokens: 150, completion_tokens: 0, total_tokens: 150 });
        let pairing = PairingStore::new(temp_dir("devices").join("devices.json"));

        let report = build_usage_report(&temp_dir("none"), &pairing, Some(&guard), 0, 0).unwrap();
        let day = &report.limits[0];
        assert!(report.limits_enabled && day.exceeded);
        assert_eq!((day.used_tokens, day.max_tokens, day.extend_tokens), (150, Some(100), 25));
        let recent = report.recent.unwrap();
        assert_eq!((recent.by_model[0].key.as_str(), recent.by_channel[0].key.as_str()), ("m", "server"));

        match handle_extend_limit(Some(&guard), 7, "day") {
            ServerMessage::LimitExtended { request_id: 7, limit } => assert_eq!(limit.max_tokens, Some(125)),
            other => panic!("unexpected {other:?}"),
        }
        assert!(matches!(handle_extend_limit(Some(&guard), 8, "nope"), ServerMessage::UsageError { request_id: 8, .. }));
        assert!(matches!(handle_extend_limit(None, 9, "day"), ServerMessage::UsageError { request_id: 9, .. }));
    }

    #[test]
    fn spend_limit_id_is_found_only_on_a_spend_limit_error() {
        let guard = guard(1);
        guard.record(&SpendContext::new("server"), "m", &Usage { prompt_tokens: 5, completion_tokens: 0, total_tokens: 5 });
        let status = guard.status(None).remove(0);
        let err = anyhow::Error::new(warden_core::budget::SpendLimitReached(Box::new(status))).context("turn failed");
        assert_eq!(spend_limit_id(&err).as_deref(), Some("day"));
        assert_eq!(spend_limit_id(&anyhow::anyhow!("provider error")), None);
    }
}
