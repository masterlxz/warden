//! Spending limits (P4): what a window of time may cost, counted in tokens and/or dollars, per
//! scope (everything, one agent, one channel, one user of a channel).
//!
//! `TurnBudget` (budget.rs) caps what the *sub-agents of one turn* may call; this caps what
//! *everything* may spend over a stretch of time, across turns, conversations and processes. Every
//! model call is appended to a ledger and a limit is the sum of the ledger over its window, so
//! the desktop, the CLI and the Telegram bot — separate processes — all draw from the same
//! account when they point at the same file.
//!
//! Nothing here calls a model or asks a human: `SpendGuard::check` says what state a turn is in,
//! and the `Orchestrator` decides what to do about it (tell the model, pause and ask, refuse).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::model::Usage;

const HOUR_MS: u64 = 3_600_000;
const MINUTE_MS: u64 = 60_000;

pub fn now_millis() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}

/// Who a limit applies to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    /// Everything this installation spends, whatever the channel or agent.
    Global,
    /// One configured agent's id.
    Agent(String),
    /// One channel by name (`desktop`, `cli`, `telegram`, `whatsapp`, `server`).
    Channel(String),
    /// One person on one channel, written `channel:user` (`telegram:12345`).
    User(String),
}

impl std::fmt::Display for Scope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Scope::Global => write!(f, "global"),
            Scope::Agent(id) => write!(f, "agent {id}"),
            Scope::Channel(name) => write!(f, "channel {name}"),
            Scope::User(key) => write!(f, "user {key}"),
        }
    }
}

/// One ceiling: at most `max_tokens` and/or `max_cost_usd` inside any `window_hours` stretch.
/// The window slides — it always covers the last `window_hours` up to now, it does not reset at
/// midnight — so a 1-hour limit catches a loop and a 24-hour one is the daily budget, and both
/// can be set at once.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Limit {
    /// Names the limit in messages and is what an extension is attached to; unique per config.
    pub id: String,
    pub scope: Scope,
    pub window_hours: u32,
    pub max_tokens: Option<u64>,
    pub max_cost_usd: Option<f64>,
    /// Fraction of the ceiling (0–1) from which the model is told how much is left.
    pub warn_at: f64,
    /// Fraction of the ceiling one extension adds while paused.
    pub extend_step: f64,
}

pub const DEFAULT_WARN_AT: f64 = 0.8;
pub const DEFAULT_EXTEND_STEP: f64 = 0.25;

impl Limit {
    pub fn new(id: impl Into<String>, scope: Scope, window_hours: u32) -> Self {
        Self {
            id: id.into(),
            scope,
            window_hours,
            max_tokens: None,
            max_cost_usd: None,
            warn_at: DEFAULT_WARN_AT,
            extend_step: DEFAULT_EXTEND_STEP,
        }
    }

    pub fn with_max_tokens(mut self, max: u64) -> Self {
        self.max_tokens = Some(max);
        self
    }

    pub fn with_max_cost_usd(mut self, max: f64) -> Self {
        self.max_cost_usd = Some(max);
        self
    }

    /// Rejects a limit that could never make sense, so a typo in `config.toml` is reported at
    /// startup rather than silently turning the limit into "never" or "always".
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(!self.id.trim().is_empty(), "a limit needs an id");
        anyhow::ensure!(self.window_hours > 0, "limit '{}': window_hours must be at least 1", self.id);
        anyhow::ensure!(
            self.max_tokens.is_some() || self.max_cost_usd.is_some(),
            "limit '{}': set max_tokens and/or max_cost_usd",
            self.id
        );
        anyhow::ensure!(self.max_tokens != Some(0), "limit '{}': max_tokens must be above 0", self.id);
        anyhow::ensure!(
            self.max_cost_usd.is_none_or(|c| c.is_finite() && c > 0.0),
            "limit '{}': max_cost_usd must be above 0",
            self.id
        );
        anyhow::ensure!(
            self.warn_at > 0.0 && self.warn_at <= 1.0,
            "limit '{}': warn_at must be above 0 and at most 1",
            self.id
        );
        anyhow::ensure!(self.extend_step > 0.0 && self.extend_step.is_finite(), "limit '{}': extend_step must be above 0", self.id);
        match &self.scope {
            Scope::Agent(v) | Scope::Channel(v) => anyhow::ensure!(!v.trim().is_empty(), "limit '{}': empty scope name", self.id),
            Scope::User(v) => anyhow::ensure!(v.contains(':'), "limit '{}': a user scope is written channel:user", self.id),
            Scope::Global => {}
        }
        Ok(())
    }
}

/// What a model charges, per million tokens. There are deliberately no built-in prices: they go
/// stale, and a wrong dollar figure that looks right is worse than none. A model without a price
/// still counts against token limits.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Price {
    pub model: String,
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
}

#[derive(Debug, Clone, Default)]
pub struct PriceTable(Vec<Price>);

impl PriceTable {
    pub fn new(prices: Vec<Price>) -> Self {
        Self(prices)
    }

    /// Dollars for one call, or `None` when this model has no price (exact model-id match).
    pub fn cost(&self, model: &str, usage: &Usage) -> Option<f64> {
        let price = self.0.iter().find(|p| p.model == model)?;
        Some(
            (usage.prompt_tokens as f64 * price.input_per_mtok + usage.completion_tokens as f64 * price.output_per_mtok)
                / 1_000_000.0,
        )
    }
}

/// One model call, as the ledger keeps it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpendEvent {
    pub ts: u64,
    pub channel: String,
    pub user: Option<String>,
    pub agent: Option<String>,
    pub model: String,
    pub tokens: u64,
    /// `None` when the model had no price at the time.
    pub cost_usd: Option<f64>,
}

impl SpendEvent {
    fn user_key(&self) -> Option<String> {
        self.user.as_ref().map(|u| format!("{}:{u}", self.channel))
    }
}

/// More room for one limit, granted by a person while a turn was paused. It lasts as long as the
/// window it was granted in, so it never turns into a permanent raise of the ceiling.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Grant {
    pub ts: u64,
    pub limit_id: String,
    pub tokens: u64,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Entry {
    Spend(SpendEvent),
    Grant(Grant),
}

impl Entry {
    fn ts(&self) -> u64 {
        match self {
            Entry::Spend(e) => e.ts,
            Entry::Grant(g) => g.ts,
        }
    }
}

/// Where the ledger lives.
pub trait SpendStore: Send + Sync {
    fn append(&self, entry: &Entry) -> anyhow::Result<()>;
    /// Every entry at or after `since_ms`. An unreadable ledger reads as empty (the guard records
    /// the failure through `append`'s error path instead): a broken file must not lock the user
    /// out of the app.
    fn entries_since(&self, since_ms: u64) -> Vec<Entry>;
}

#[derive(Default)]
pub struct MemoryStore(Mutex<Vec<Entry>>);

impl SpendStore for MemoryStore {
    fn append(&self, entry: &Entry) -> anyhow::Result<()> {
        self.0.lock().unwrap().push(entry.clone());
        Ok(())
    }

    fn entries_since(&self, since_ms: u64) -> Vec<Entry> {
        self.0.lock().unwrap().iter().filter(|e| e.ts() >= since_ms).cloned().collect()
    }
}

/// A JSON-lines file: one entry per line, appended with a single write so two processes adding at
/// once don't interleave. Lines that don't parse are skipped, not fatal.
pub struct FileStore {
    path: PathBuf,
}

impl FileStore {
    /// Opens (creating on first write) the ledger at `path`, dropping entries older than
    /// `retain_hours` — they can no longer count toward any window, so the file stays as small as
    /// the longest limit needs.
    pub fn open(path: impl Into<PathBuf>, retain_hours: u32) -> Self {
        let store = Self { path: path.into() };
        store.prune(now_millis().saturating_sub(retain_hours as u64 * HOUR_MS));
        store
    }

    fn read_all(&self) -> Vec<Entry> {
        let Ok(text) = std::fs::read_to_string(&self.path) else { return Vec::new() };
        text.lines().filter_map(|line| serde_json::from_str(line).ok()).collect()
    }

    fn prune(&self, keep_from_ms: u64) {
        let entries = self.read_all();
        if entries.iter().all(|e| e.ts() >= keep_from_ms) {
            return;
        }
        let kept: String = entries
            .iter()
            .filter(|e| e.ts() >= keep_from_ms)
            .filter_map(|e| serde_json::to_string(e).ok())
            .map(|line| line + "\n")
            .collect();
        let tmp = self.path.with_extension("tmp");
        if std::fs::write(&tmp, kept).is_ok() {
            let _ = std::fs::rename(&tmp, &self.path);
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl SpendStore for FileStore {
    fn append(&self, entry: &Entry) -> anyhow::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut line = serde_json::to_string(entry)?;
        line.push('\n');
        let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&self.path)?;
        file.write_all(line.as_bytes())?;
        Ok(())
    }

    fn entries_since(&self, since_ms: u64) -> Vec<Entry> {
        self.read_all().into_iter().filter(|e| e.ts() >= since_ms).collect()
    }
}

/// Who is spending: the turn's channel, the person on it, and the agent it speaks as.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpendContext {
    pub channel: String,
    pub user: Option<String>,
    pub agent: Option<String>,
}

impl SpendContext {
    pub fn new(channel: impl Into<String>) -> Self {
        Self { channel: channel.into(), user: None, agent: None }
    }

    pub fn with_user(mut self, user: impl Into<String>) -> Self {
        self.user = Some(user.into());
        self
    }

    pub fn with_agent(mut self, agent: Option<String>) -> Self {
        self.agent = agent;
        self
    }

    fn user_key(&self) -> Option<String> {
        self.user.as_ref().map(|u| format!("{}:{u}", self.channel))
    }

    fn is_covered_by(&self, scope: &Scope) -> bool {
        match scope {
            Scope::Global => true,
            Scope::Agent(id) => self.agent.as_deref() == Some(id),
            Scope::Channel(name) => &self.channel == name,
            Scope::User(key) => self.user_key().as_deref() == Some(key),
        }
    }
}

fn counts_toward(scope: &Scope, event: &SpendEvent) -> bool {
    match scope {
        Scope::Global => true,
        Scope::Agent(id) => event.agent.as_deref() == Some(id),
        Scope::Channel(name) => &event.channel == name,
        Scope::User(key) => event.user_key().as_deref() == Some(key),
    }
}

/// Where one limit stands right now.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LimitStatus {
    pub id: String,
    pub scope: String,
    pub window_hours: u32,
    pub used_tokens: u64,
    /// Ceiling including whatever extensions are still in force.
    pub max_tokens: Option<u64>,
    pub remaining_tokens: Option<u64>,
    pub used_cost_usd: f64,
    pub max_cost_usd: Option<f64>,
    pub remaining_cost_usd: Option<f64>,
    /// The closer of the two ceilings to being hit, 0 when nothing is spent; 1 or more = exhausted.
    pub fraction: f64,
    pub warn: bool,
    pub exceeded: bool,
    /// Calls in the window whose model had no price — the dollar figure undercounts by these.
    pub unpriced_calls: u64,
    /// Minutes until the oldest spend leaves the window and room starts to come back.
    pub frees_up_in_minutes: Option<u64>,
}

impl LimitStatus {
    /// One line a person or a model can read.
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        if let Some(max) = self.max_tokens {
            parts.push(format!("{} of {} tokens ({} left)", self.used_tokens, max, self.remaining_tokens.unwrap_or(0)));
        }
        if let Some(max) = self.max_cost_usd {
            parts.push(format!("${:.4} of ${:.2} ({:.4} left)", self.used_cost_usd, max, self.remaining_cost_usd.unwrap_or(0.0)));
        }
        let mut line = format!(
            "{} [{}, last {}h]: {:.0}% used — {}",
            self.id,
            self.scope,
            self.window_hours,
            self.fraction * 100.0,
            parts.join("; ")
        );
        if self.unpriced_calls > 0 && self.max_cost_usd.is_some() {
            line.push_str(&format!(" (dollars undercounted: {} calls to models with no price)", self.unpriced_calls));
        }
        if let Some(min) = self.frees_up_in_minutes {
            line.push_str(&format!("; room starts coming back in ~{min} min"));
        }
        line
    }
}

/// The verdict for one turn about to spend.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Check {
    /// The most-used limit that has no room left; `None` means the call may go ahead.
    pub exceeded: Option<LimitStatus>,
    /// Limits past their `warn_at` that still have room.
    pub warnings: Vec<LimitStatus>,
}

/// Tells the model where it stands, for a turn that is running low — `None` when there's nothing
/// to say, so a turn far from every limit costs no context at all.
pub fn meter_notice(warnings: &[LimitStatus]) -> Option<String> {
    if warnings.is_empty() {
        return None;
    }
    let lines = warnings.iter().map(|s| format!("- {}", s.describe())).collect::<Vec<_>>().join("\n");
    Some(format!(
        "Spending meter — you are close to a spending limit:\n{lines}\nWhen a limit runs out the turn is paused and the user has to allow more. \
         Prefer finishing with what you have over starting new work, and don't delegate more than you must. The `budget` tool shows the exact numbers."
    ))
}

pub struct SpendGuard {
    store: Arc<dyn SpendStore>,
    limits: Vec<Limit>,
    prices: PriceTable,
    clock: Arc<dyn Fn() -> u64 + Send + Sync>,
    last_error: Mutex<Option<String>>,
}

impl SpendGuard {
    pub fn new(store: Arc<dyn SpendStore>, limits: Vec<Limit>, prices: PriceTable) -> Self {
        Self { store, limits, prices, clock: Arc::new(now_millis), last_error: Mutex::new(None) }
    }

    /// Replaces the clock — lets a test slide the window without sleeping.
    pub fn with_clock(mut self, clock: impl Fn() -> u64 + Send + Sync + 'static) -> Self {
        self.clock = Arc::new(clock);
        self
    }

    pub fn limits(&self) -> &[Limit] {
        &self.limits
    }

    /// The longest window among the limits, in hours — how much history the ledger has to keep.
    pub fn longest_window_hours(&self) -> u32 {
        self.limits.iter().map(|l| l.window_hours).max().unwrap_or(0)
    }

    /// Why the ledger could not be written the last time it failed, if it did.
    pub fn last_error(&self) -> Option<String> {
        self.last_error.lock().unwrap().clone()
    }

    /// Where each limit stands: the ones that apply to `ctx`, or every configured limit for `None`.
    pub fn status(&self, ctx: Option<&SpendContext>) -> Vec<LimitStatus> {
        let now = (self.clock)();
        let limits: Vec<&Limit> = self.limits.iter().filter(|l| ctx.is_none_or(|c| c.is_covered_by(&l.scope))).collect();
        let Some(longest) = limits.iter().map(|l| l.window_hours).max() else { return Vec::new() };
        let entries = self.store.entries_since(now.saturating_sub(longest as u64 * HOUR_MS));
        limits.into_iter().map(|limit| evaluate(limit, &entries, now)).collect()
    }

    pub fn check(&self, ctx: &SpendContext) -> Check {
        let mut check = Check::default();
        for status in self.status(Some(ctx)) {
            if status.exceeded {
                if check.exceeded.as_ref().is_none_or(|worst| status.fraction > worst.fraction) {
                    check.exceeded = Some(status);
                }
            } else if status.warn {
                check.warnings.push(status);
            }
        }
        check
    }

    /// Books one model call. Nothing to book for a call that reported no tokens.
    pub fn record(&self, ctx: &SpendContext, model: &str, usage: &Usage) {
        let tokens = usage.total_tokens.max(usage.prompt_tokens + usage.completion_tokens) as u64;
        if tokens == 0 {
            return;
        }
        let event = SpendEvent {
            ts: (self.clock)(),
            channel: ctx.channel.clone(),
            user: ctx.user.clone(),
            agent: ctx.agent.clone(),
            model: model.to_string(),
            tokens,
            cost_usd: self.prices.cost(model, usage),
        };
        self.note(self.store.append(&Entry::Spend(event)));
    }

    /// What one extension of `limit_id` would add, as `(tokens, dollars)` — `0` for a ceiling the
    /// limit doesn't have. `None` when no limit has that id.
    pub fn extension_size(&self, limit_id: &str) -> Option<(u64, f64)> {
        let limit = self.limits.iter().find(|l| l.id == limit_id)?;
        Some((
            limit.max_tokens.map_or(0, |m| ((m as f64 * limit.extend_step).ceil() as u64).max(1)),
            limit.max_cost_usd.map_or(0.0, |m| m * limit.extend_step),
        ))
    }

    /// Lets one limit go one `extend_step` further for the rest of its current window.
    pub fn extend(&self, limit_id: &str) -> anyhow::Result<Grant> {
        let (tokens, cost_usd) = self.extension_size(limit_id).ok_or_else(|| anyhow::anyhow!("no limit named '{limit_id}'"))?;
        let grant = Grant { ts: (self.clock)(), limit_id: limit_id.to_string(), tokens, cost_usd };
        self.store.append(&Entry::Grant(grant.clone()))?;
        Ok(grant)
    }

    fn note(&self, result: anyhow::Result<()>) {
        *self.last_error.lock().unwrap() = result.err().map(|e| format!("{e:#}"));
    }
}

fn evaluate(limit: &Limit, entries: &[Entry], now: u64) -> LimitStatus {
    let window_start = now.saturating_sub(limit.window_hours as u64 * HOUR_MS);
    let (mut used_tokens, mut used_cost, mut unpriced) = (0u64, 0f64, 0u64);
    let (mut extra_tokens, mut extra_cost) = (0u64, 0f64);
    let mut oldest: Option<u64> = None;
    for entry in entries.iter().filter(|e| e.ts() > window_start) {
        match entry {
            Entry::Spend(event) if counts_toward(&limit.scope, event) => {
                used_tokens += event.tokens;
                match event.cost_usd {
                    Some(c) => used_cost += c,
                    None => unpriced += 1,
                }
                oldest = Some(oldest.map_or(event.ts, |o| o.min(event.ts)));
            }
            Entry::Grant(grant) if grant.limit_id == limit.id => {
                extra_tokens += grant.tokens;
                extra_cost += grant.cost_usd;
            }
            _ => {}
        }
    }
    let max_tokens = limit.max_tokens.map(|m| m + extra_tokens);
    let max_cost_usd = limit.max_cost_usd.map(|m| m + extra_cost);
    let fraction = [
        max_tokens.map(|m| used_tokens as f64 / m as f64),
        max_cost_usd.map(|m| used_cost / m),
    ]
    .into_iter()
    .flatten()
    .fold(0.0, f64::max);
    let exceeded = fraction >= 1.0;
    LimitStatus {
        id: limit.id.clone(),
        scope: limit.scope.to_string(),
        window_hours: limit.window_hours,
        used_tokens,
        max_tokens,
        remaining_tokens: max_tokens.map(|m| m.saturating_sub(used_tokens)),
        used_cost_usd: used_cost,
        max_cost_usd,
        remaining_cost_usd: max_cost_usd.map(|m| (m - used_cost).max(0.0)),
        fraction,
        warn: fraction >= limit.warn_at,
        exceeded,
        unpriced_calls: unpriced,
        frees_up_in_minutes: oldest
            .map(|o| (o + limit.window_hours as u64 * HOUR_MS).saturating_sub(now).div_ceil(MINUTE_MS)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn usage(prompt: u32, completion: u32) -> Usage {
        Usage { prompt_tokens: prompt, completion_tokens: completion, total_tokens: prompt + completion }
    }

    /// A guard whose clock the test moves by hand, starting well after the epoch so windows
    /// reaching back a few hours don't underflow.
    fn guard(limits: Vec<Limit>, prices: Vec<Price>) -> (SpendGuard, Arc<AtomicU64>) {
        let now = Arc::new(AtomicU64::new(1_000 * HOUR_MS));
        let clock = now.clone();
        let guard = SpendGuard::new(Arc::new(MemoryStore::default()), limits, PriceTable::new(prices))
            .with_clock(move || clock.load(Ordering::SeqCst));
        (guard, now)
    }

    fn cli() -> SpendContext {
        SpendContext::new("cli")
    }

    #[test]
    fn prices_split_input_and_output_and_unknown_models_have_none() {
        let table = PriceTable::new(vec![Price { model: "m".into(), input_per_mtok: 3.0, output_per_mtok: 15.0 }]);
        let cost = table.cost("m", &usage(1_000_000, 100_000)).unwrap();
        assert!((cost - 4.5).abs() < 1e-9, "{cost}");
        assert_eq!(table.cost("other", &usage(10, 10)), None);
    }

    #[test]
    fn a_limit_is_clear_then_warns_then_is_exceeded() {
        let (guard, _) = guard(vec![Limit::new("day", Scope::Global, 24).with_max_tokens(1000)], vec![]);
        assert_eq!(guard.check(&cli()), Check::default());

        guard.record(&cli(), "m", &usage(500, 300));
        let check = guard.check(&cli());
        assert!(check.exceeded.is_none());
        assert_eq!(check.warnings.len(), 1, "800 of 1000 is exactly the 80% mark");

        guard.record(&cli(), "m", &usage(100, 100));
        let exceeded = guard.check(&cli()).exceeded.expect("1000 of 1000 leaves no room");
        assert_eq!((exceeded.used_tokens, exceeded.remaining_tokens), (1000, Some(0)));
    }

    #[test]
    fn the_window_slides_so_old_spending_stops_counting() {
        let (guard, now) = guard(vec![Limit::new("hour", Scope::Global, 1).with_max_tokens(100)], vec![]);
        guard.record(&cli(), "m", &usage(60, 60));
        assert!(guard.check(&cli()).exceeded.is_some());

        now.fetch_add(HOUR_MS - 1, Ordering::SeqCst);
        assert!(guard.check(&cli()).exceeded.is_some(), "one millisecond before the hour is up");
        now.fetch_add(2, Ordering::SeqCst);
        assert_eq!(guard.check(&cli()), Check::default());
    }

    #[test]
    fn scopes_only_apply_to_and_count_their_own_spending() {
        let (guard, _) = guard(
            vec![
                Limit::new("chief", Scope::Agent("chief".into()), 24).with_max_tokens(100),
                Limit::new("tg", Scope::Channel("telegram".into()), 24).with_max_tokens(100),
                Limit::new("ana", Scope::User("telegram:42".into()), 24).with_max_tokens(100),
            ],
            vec![],
        );
        let chief = SpendContext::new("desktop").with_agent(Some("chief".into()));
        let ana = SpendContext::new("telegram").with_user("42");
        let bob = SpendContext::new("telegram").with_user("7");

        guard.record(&chief, "m", &usage(100, 100));
        assert!(guard.check(&chief).exceeded.is_some());
        assert_eq!(guard.check(&ana), Check::default(), "the chief's spending is not Ana's, nor Telegram's");
        assert_eq!(guard.check(&cli()), Check::default(), "no limit covers the CLI");

        guard.record(&ana, "m", &usage(100, 100));
        assert!(guard.check(&ana).exceeded.is_some());
        assert!(guard.check(&bob).exceeded.is_some(), "the channel limit is shared by everyone on it");
        assert_eq!(guard.status(Some(&bob)).len(), 1, "Bob is covered by the channel limit only");
    }

    #[test]
    fn the_global_limit_adds_up_every_channel_and_the_worst_limit_is_reported() {
        let (guard, _) = guard(
            vec![
                Limit::new("all", Scope::Global, 24).with_max_tokens(1000),
                Limit::new("cli-only", Scope::Channel("cli".into()), 24).with_max_tokens(100),
            ],
            vec![],
        );
        guard.record(&SpendContext::new("telegram").with_user("1"), "m", &usage(300, 300));
        guard.record(&cli(), "m", &usage(60, 60));
        let exceeded = guard.check(&cli()).exceeded.unwrap();
        assert_eq!(exceeded.id, "cli-only", "120/100 is further gone than 720/1000");
    }

    #[test]
    fn dollars_are_a_limit_of_their_own_and_unpriced_calls_are_flagged() {
        let price = Price { model: "priced".into(), input_per_mtok: 1000.0, output_per_mtok: 1000.0 };
        let (guard, _) = guard(vec![Limit::new("money", Scope::Global, 24).with_max_cost_usd(1.0)], vec![price]);
        guard.record(&cli(), "priced", &usage(400, 100));
        guard.record(&cli(), "mystery", &usage(9_000_000, 0));
        let status = &guard.status(None)[0];
        assert!((status.used_cost_usd - 0.5).abs() < 1e-9);
        assert!(!status.exceeded, "9M unpriced tokens don't count against a dollar limit");
        assert_eq!(status.unpriced_calls, 1);
        assert!(status.describe().contains("undercounted"));

        guard.record(&cli(), "priced", &usage(400, 100));
        assert!(guard.check(&cli()).exceeded.is_some());
    }

    #[test]
    fn a_limit_with_both_ceilings_trips_on_whichever_comes_first() {
        let price = Price { model: "m".into(), input_per_mtok: 1.0, output_per_mtok: 1.0 };
        let (guard, _) = guard(vec![Limit::new("both", Scope::Global, 24).with_max_tokens(10_000).with_max_cost_usd(1.0)], vec![price]);
        guard.record(&cli(), "m", &usage(5000, 5000));
        let status = guard.check(&cli()).exceeded.expect("tokens ran out; the money did not");
        assert!(status.used_cost_usd < 0.1);
    }

    #[test]
    fn an_extension_adds_one_step_lasts_the_window_and_stacks() {
        let mut limit = Limit::new("day", Scope::Global, 24).with_max_tokens(1000);
        limit.extend_step = 0.5;
        let (guard, now) = guard(vec![limit], vec![]);
        guard.record(&cli(), "m", &usage(600, 500));
        assert!(guard.check(&cli()).exceeded.is_some());

        let grant = guard.extend("day").unwrap();
        assert_eq!(grant.tokens, 500);
        let status = &guard.status(None)[0];
        assert_eq!((status.max_tokens, status.remaining_tokens, status.exceeded), (Some(1500), Some(400), false));

        guard.record(&cli(), "m", &usage(200, 200));
        assert!(guard.check(&cli()).exceeded.is_some());
        guard.extend("day").unwrap();
        assert_eq!(guard.status(None)[0].max_tokens, Some(2000));

        now.fetch_add(24 * HOUR_MS + 1, Ordering::SeqCst);
        assert_eq!(guard.status(None)[0].max_tokens, Some(1000), "extensions expire with the window");
        assert!(guard.extend("nope").is_err());
    }

    #[test]
    fn frees_up_reports_when_the_oldest_spending_leaves_the_window() {
        let (guard, now) = guard(vec![Limit::new("hour", Scope::Global, 1).with_max_tokens(100)], vec![]);
        assert_eq!(guard.status(None)[0].frees_up_in_minutes, None);
        guard.record(&cli(), "m", &usage(50, 50));
        now.fetch_add(20 * MINUTE_MS, Ordering::SeqCst);
        assert_eq!(guard.status(None)[0].frees_up_in_minutes, Some(40));
    }

    #[test]
    fn a_call_that_reports_no_tokens_books_nothing() {
        let (guard, _) = guard(vec![Limit::new("day", Scope::Global, 24).with_max_tokens(10)], vec![]);
        guard.record(&cli(), "m", &Usage::default());
        assert_eq!(guard.status(None)[0].used_tokens, 0);
    }

    #[test]
    fn the_notice_is_silent_until_a_limit_warns_and_then_says_what_is_left() {
        let (guard, _) = guard(vec![Limit::new("day", Scope::Global, 24).with_max_tokens(1000)], vec![]);
        assert_eq!(meter_notice(&guard.check(&cli()).warnings), None);
        guard.record(&cli(), "m", &usage(450, 450));
        let notice = meter_notice(&guard.check(&cli()).warnings).unwrap();
        assert!(notice.contains("900 of 1000 tokens (100 left)"), "{notice}");
        assert!(notice.contains("`budget` tool"));
    }

    #[test]
    fn validation_rejects_limits_that_could_never_work() {
        let ok = Limit::new("a", Scope::Global, 1).with_max_tokens(1);
        assert!(ok.validate().is_ok());
        assert!(Limit::new("a", Scope::Global, 0).with_max_tokens(1).validate().is_err());
        assert!(Limit::new("a", Scope::Global, 1).validate().is_err(), "no ceiling at all");
        assert!(Limit::new("a", Scope::Global, 1).with_max_tokens(0).validate().is_err());
        assert!(Limit::new("a", Scope::Global, 1).with_max_cost_usd(-1.0).validate().is_err());
        assert!(Limit::new("", Scope::Global, 1).with_max_tokens(1).validate().is_err());
        assert!(Limit::new("a", Scope::User("no-colon".into()), 1).with_max_tokens(1).validate().is_err());
        let mut bad = ok.clone();
        bad.warn_at = 1.5;
        assert!(bad.validate().is_err());
        let mut bad = ok;
        bad.extend_step = 0.0;
        assert!(bad.validate().is_err());
    }

    fn temp_ledger(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "warden-spend-{name}-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        dir.join("usage-ledger.jsonl")
    }

    #[test]
    fn the_file_store_is_shared_between_handles_and_skips_garbage() {
        let path = temp_ledger("shared");
        let limits = vec![Limit::new("day", Scope::Global, 24).with_max_tokens(1000)];
        let writer = SpendGuard::new(Arc::new(FileStore::open(&path, 24)), limits.clone(), PriceTable::default());
        let reader = SpendGuard::new(Arc::new(FileStore::open(&path, 24)), limits, PriceTable::default());

        writer.record(&cli(), "m", &usage(100, 50));
        std::fs::OpenOptions::new().append(true).open(&path).unwrap().write_all(b"not json\n").unwrap();
        writer.record(&cli(), "m", &usage(10, 10));

        assert_eq!(reader.status(None)[0].used_tokens, 170, "a second process sees the first one's spending");
        assert_eq!(writer.last_error(), None);
    }

    #[test]
    fn opening_the_file_store_drops_entries_no_window_needs() {
        let path = temp_ledger("prune");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let old = Entry::Spend(SpendEvent { ts: 1, channel: "cli".into(), user: None, agent: None, model: "m".into(), tokens: 5, cost_usd: None });
        let fresh = Entry::Spend(SpendEvent { ts: now_millis(), ..match old.clone() { Entry::Spend(e) => e, _ => unreachable!() } });
        let text = format!("{}\n{}\n", serde_json::to_string(&old).unwrap(), serde_json::to_string(&fresh).unwrap());
        std::fs::write(&path, text).unwrap();

        let store = FileStore::open(&path, 24);
        assert_eq!(store.entries_since(0), vec![fresh]);
        assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 1, "rewritten, not just filtered on read");
    }

    #[test]
    fn a_ledger_that_cannot_be_written_is_reported_not_fatal() {
        let dir = temp_ledger("unwritable");
        std::fs::create_dir_all(&dir).unwrap();
        // The ledger path is a directory, so opening it for append fails.
        let guard = SpendGuard::new(Arc::new(FileStore::open(&dir, 24)), vec![], PriceTable::default());
        guard.record(&cli(), "m", &usage(1, 1));
        assert!(guard.last_error().is_some());
        assert_eq!(guard.check(&cli()), Check::default());
    }
}
