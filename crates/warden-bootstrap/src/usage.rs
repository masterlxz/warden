//! Cross-conversation token usage — aggregated on demand from the same `Conversation` files
//! `list_conversations` already reads for the desktop sidebar, not a separately maintained
//! index. Not the `warden-cli` REPL's own `/usage` (see `warden-cli`'s `interactive.rs`): that
//! one only ever sees the current process's in-memory turns and is never persisted, since the
//! CLI's own conversation history doesn't go through `save_conversation` at all. This module is
//! the data these two consumers share:
//! - `UsageStatsTool` — a `Tool` the model itself can call (registered in `bootstrap()`), so a
//!   question like "how many tokens have I used?" costs nothing in every *other* turn's context.
//! - the desktop app's planned usage dashboard (`PENDING.md` P4) will call `aggregate_usage`
//!   directly from a Tauri command, the same way `list_conversations` already backs
//!   `list_conversations` (the IPC command of the same name).
//!
//! Deliberately token-only, no `$` figure: there's no per-model price table in the project yet
//! (same gap noted in `PENDING.md` P4 for the dashboard). Deliberately no per-message model
//! attribution either: `Conversation.agent_id`/`provider_id` record only the *last* selection for
//! the whole conversation (the desktop's per-conversation selectors), not per-message — a
//! conversation that switched provider partway through has every message's usage counted under
//! whichever provider is selected now. Exact per-message attribution would need recording the
//! provider/agent alongside each `ConversationMessage`, which doesn't exist today; out of scope
//! for this on-demand aggregation (the option deliberately chosen over a new persisted index).

use std::collections::HashMap;
use std::path::PathBuf;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use warden_core::model::Usage;
use warden_core::tool::{Tool, ToolSpec};

use crate::{list_conversations, Conversation};

/// One bucket of a `UsageSummary` breakdown — `key` is the conversation-level `agent_id` or
/// `provider_id` being grouped by; `None` means "no override" (see this module's doc comment).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageByKey {
    pub key: Option<String>,
    pub message_count: usize,
    pub usage: Usage,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSummary {
    pub conversation_count: usize,
    pub message_count: usize,
    pub total: Usage,
    pub by_agent: Vec<UsageByKey>,
    pub by_provider: Vec<UsageByKey>,
}

/// Sums every `ConversationMessage.usage` across `conversations` — only assistant messages ever
/// carry one (see `SendMessageResult`/`ConversationMessage` construction in the desktop/Telegram/
/// WhatsApp channels), so `message_count` here means "model calls", not "chat messages sent".
/// Breakdown vectors are sorted by `total_tokens` descending, largest consumer first — the natural
/// order for a dashboard or a model reading this back.
pub fn aggregate_usage(conversations: &[Conversation]) -> UsageSummary {
    let mut summary = UsageSummary { conversation_count: conversations.len(), ..Default::default() };
    let mut by_agent: HashMap<Option<String>, UsageByKey> = HashMap::new();
    let mut by_provider: HashMap<Option<String>, UsageByKey> = HashMap::new();

    for conversation in conversations {
        for message in &conversation.messages {
            let Some(usage) = &message.usage else { continue };
            summary.message_count += 1;
            summary.total += usage;

            let agent_bucket = by_agent.entry(conversation.agent_id.clone()).or_insert_with(|| UsageByKey {
                key: conversation.agent_id.clone(),
                ..Default::default()
            });
            agent_bucket.message_count += 1;
            agent_bucket.usage += usage;

            let provider_bucket = by_provider.entry(conversation.provider_id.clone()).or_insert_with(|| UsageByKey {
                key: conversation.provider_id.clone(),
                ..Default::default()
            });
            provider_bucket.message_count += 1;
            provider_bucket.usage += usage;
        }
    }

    summary.by_agent = by_agent.into_values().collect();
    summary.by_agent.sort_by_key(|b| std::cmp::Reverse(b.usage.total_tokens));
    summary.by_provider = by_provider.into_values().collect();
    summary.by_provider.sort_by_key(|b| std::cmp::Reverse(b.usage.total_tokens));

    summary
}

/// Tokens spent on one calendar day, in `daily_usage`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyUsage {
    /// `YYYY-MM-DD` in the viewer's time zone.
    pub date: String,
    pub calls: usize,
    pub tokens: u64,
}

const DAY_MS: i64 = 86_400_000;

/// The last `days` days (today included, oldest first, empty days as zero) of model calls across
/// `conversations`, bucketed by each message's `created_at` shifted by `tz_offset_minutes` — the
/// viewer's offset from UTC (UTC−3 is `-180`), so a day starts at the viewer's midnight.
pub fn daily_usage(conversations: &[Conversation], days: u32, tz_offset_minutes: i32, now_ms: i64) -> Vec<DailyUsage> {
    let offset_ms = tz_offset_minutes as i64 * 60_000;
    let today = (now_ms + offset_ms).div_euclid(DAY_MS);
    let first = today - days as i64 + 1;
    let mut out: Vec<DailyUsage> = (first..=today).map(|day| DailyUsage { date: format_day(day), ..Default::default() }).collect();

    for message in conversations.iter().flat_map(|c| &c.messages) {
        let Some(usage) = &message.usage else { continue };
        let day = (message.created_at + offset_ms).div_euclid(DAY_MS);
        if let Some(bucket) = (day >= first && day <= today).then(|| &mut out[(day - first) as usize]) {
            bucket.calls += 1;
            bucket.tokens += usage.total_tokens as u64;
        }
    }
    out
}

/// Days since 1970-01-01 as `YYYY-MM-DD` (proleptic Gregorian) — Howard Hinnant's `civil_from_days`,
/// so a date needs no calendar crate.
fn format_day(days: i64) -> String {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// The `Tool` the model itself can call to answer a question like "how many tokens have I used?"
/// — reads every persisted conversation fresh on each call (no caching), the same "never stale"
/// tradeoff `resolve_turn_context` makes elsewhere in this crate, since a dashboard/tool answering
/// with yesterday's numbers would be worse than a bit of extra disk I/O.
pub struct UsageStatsTool {
    conversations_dir: Option<PathBuf>,
}

impl UsageStatsTool {
    pub fn new(conversations_dir: Option<PathBuf>) -> Self {
        Self { conversations_dir }
    }
}

#[async_trait]
impl Tool for UsageStatsTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "usage_stats".to_string(),
            description: "Get token usage across every saved conversation on this device: total prompt/completion/total tokens, how many model calls and conversations, and a breakdown by named agent and by model provider. Token counts only — no dollar cost estimate is available.".to_string(),
            parameters: json!({ "type": "object", "properties": {} }),
        }
    }

    async fn call(&self, _args: Value) -> anyhow::Result<Value> {
        let Some(dir) = &self.conversations_dir else {
            return Ok(json!({ "error": "no conversations directory available on this system" }));
        };
        let conversations = list_conversations(dir)?;
        Ok(serde_json::to_value(aggregate_usage(&conversations))?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ChatRole;

    fn message(usage: Option<Usage>) -> crate::ConversationMessage {
        crate::ConversationMessage { id: "m".to_string(), role: ChatRole::Assistant, content: String::new(), created_at: 0, usage, attachments: Vec::new(), generated_files: Vec::new() }
    }

    fn conversation(agent_id: Option<&str>, provider_id: Option<&str>, messages: Vec<crate::ConversationMessage>) -> Conversation {
        Conversation {
            id: "c".to_string(),
            title: "t".to_string(),
            messages,
            created_at: 0,
            updated_at: 0,
            agent_id: agent_id.map(str::to_string),
            provider_id: provider_id.map(str::to_string),
        }
    }

    #[test]
    fn format_day_matches_known_dates() {
        assert_eq!(format_day(0), "1970-01-01");
        assert_eq!(format_day(-1), "1969-12-31");
        assert_eq!(format_day(11_016), "2000-02-29");
        assert_eq!(format_day(20_721), "2026-09-25");
    }

    #[test]
    fn daily_usage_buckets_by_the_viewers_day_and_fills_empty_days() {
        let at = |created_at: i64, tokens: u32| crate::ConversationMessage {
            created_at,
            ..message(Some(Usage { prompt_tokens: tokens, completion_tokens: 0, total_tokens: tokens }))
        };
        let day = 20_721 * DAY_MS; // 2026-09-25T00:00Z
        let now = day + 12 * 3_600_000; // noon UTC
        let conversations = vec![conversation(None, None, vec![
            at(day + 3_600_000, 10),          // 01:00Z: the 25th in UTC, still the 24th at UTC-3
            at(day - 2 * DAY_MS + 6 * 3_600_000, 5), // the 23rd in both zones
            at(day - 10 * DAY_MS, 99),         // outside a 3-day range
            crate::ConversationMessage { created_at: day, ..message(None) }, // no usage, not a call
        ])];

        let utc = daily_usage(&conversations, 3, 0, now);
        assert_eq!(utc.iter().map(|d| (d.date.as_str(), d.calls, d.tokens)).collect::<Vec<_>>(), vec![
            ("2026-09-23", 1, 5),
            ("2026-09-24", 0, 0),
            ("2026-09-25", 1, 10),
        ]);

        let brasilia = daily_usage(&conversations, 3, -180, now);
        assert_eq!(brasilia.iter().map(|d| (d.date.as_str(), d.tokens)).collect::<Vec<_>>(), vec![
            ("2026-09-23", 5),
            ("2026-09-24", 10),
            ("2026-09-25", 0),
        ]);
    }

    #[test]
    fn sums_total_tokens_across_conversations_and_ignores_messages_without_usage() {
        let usage_a = Usage { prompt_tokens: 10, completion_tokens: 5, total_tokens: 15 };
        let usage_b = Usage { prompt_tokens: 3, completion_tokens: 2, total_tokens: 5 };
        let conversations = vec![
            conversation(None, None, vec![message(Some(usage_a)), message(None)]),
            conversation(None, None, vec![message(Some(usage_b))]),
        ];

        let summary = aggregate_usage(&conversations);

        assert_eq!(summary.conversation_count, 2);
        assert_eq!(summary.message_count, 2);
        assert_eq!(summary.total, Usage { prompt_tokens: 13, completion_tokens: 7, total_tokens: 20 });
    }

    #[test]
    fn breaks_down_by_agent_and_provider_keyed_by_the_conversations_last_selection() {
        let usage = Usage { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 };
        let conversations = vec![
            conversation(Some("assistant-a"), Some("openai-work"), vec![message(Some(usage))]),
            conversation(Some("assistant-a"), Some("openai-work"), vec![message(Some(usage))]),
            conversation(None, Some("gemini-default"), vec![message(Some(usage))]),
        ];

        let summary = aggregate_usage(&conversations);

        let agent_a = summary.by_agent.iter().find(|b| b.key.as_deref() == Some("assistant-a")).unwrap();
        assert_eq!(agent_a.message_count, 2);
        assert_eq!(agent_a.usage.total_tokens, 4);

        let no_agent = summary.by_agent.iter().find(|b| b.key.is_none()).unwrap();
        assert_eq!(no_agent.message_count, 1);

        assert_eq!(summary.by_provider.len(), 2);
        // Sorted by total_tokens descending — the two-conversation provider comes first.
        assert_eq!(summary.by_provider[0].key.as_deref(), Some("openai-work"));
    }

    #[tokio::test]
    async fn tool_call_reads_conversations_from_disk_and_returns_the_aggregated_summary() {
        let dir = std::env::temp_dir().join(format!(
            "warden-bootstrap-usage-tool-test-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        let usage = Usage { prompt_tokens: 7, completion_tokens: 3, total_tokens: 10 };
        crate::save_conversation(&dir, &conversation(None, None, vec![message(Some(usage))])).unwrap();

        let tool = UsageStatsTool::new(Some(dir.clone()));
        let result = tool.call(json!({})).await.unwrap();
        let summary: UsageSummary = serde_json::from_value(result).unwrap();

        assert_eq!(summary.message_count, 1);
        assert_eq!(summary.total, usage);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn tool_call_reports_an_error_value_when_no_conversations_dir_is_available() {
        let tool = UsageStatsTool::new(None);
        let result = tool.call(json!({})).await.unwrap();
        assert!(result.get("error").is_some());
    }

    #[test]
    fn empty_conversation_list_summarizes_to_all_zeros() {
        let summary = aggregate_usage(&[]);
        assert_eq!(summary.conversation_count, 0);
        assert_eq!(summary.message_count, 0);
        assert_eq!(summary.total, Usage::default());
        assert!(summary.by_agent.is_empty());
        assert!(summary.by_provider.is_empty());
    }
}
