import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { UsageByKey, UsageSummary } from "../types";

/** `1,284` up to 999, `12.9K`/`1.2M` beyond — the stat-tile contract's "auto-compact" value
 * format, so a heavy user's token count never wraps a tile onto two lines. */
function formatCompact(n: number): string {
  return new Intl.NumberFormat("en-US", { notation: "compact", maximumFractionDigits: 1 }).format(n);
}

function StatTile({ label, value }: { label: string; value: number }) {
  return (
    <div className="usage-stat-tile">
      <span className="usage-stat-value">{formatCompact(value)}</span>
      <span className="usage-stat-label">{label}</span>
    </div>
  );
}

/** A magnitude-by-category breakdown (by agent, by provider) — one accent-colored bar per row,
 * `key` already carries the display name (`AgentEntry`/`ProviderEntry.id` doubles as its own
 * name, see `types.ts`), so no separate name lookup against Settings is needed. One series (the
 * same metric, total tokens, repeated per category) needs no legend — identity comes from the
 * row label, not the bar's color. Value sits at the bar's tip, the direct-label spec for bars. */
function UsageBreakdown({ title, entries, emptyKeyLabel }: { title: string; entries: UsageByKey[]; emptyKeyLabel: string }) {
  if (entries.length === 0) return null;
  const maxTokens = Math.max(...entries.map((entry) => entry.usage.totalTokens), 1);

  return (
    <section className="settings-section">
      <div className="settings-section-header">
        <h3 className="settings-section-title">{title}</h3>
      </div>
      <div className="usage-bar-list">
        {entries.map((entry) => (
          <div className="usage-bar-row" key={entry.key ?? "none"}>
            <span className="usage-bar-label">{entry.key ?? emptyKeyLabel}</span>
            <div className="usage-bar-track">
              <div className="usage-bar-fill" style={{ width: `${(entry.usage.totalTokens / maxTokens) * 100}%` }} />
            </div>
            <span className="usage-bar-value">{formatCompact(entry.usage.totalTokens)}</span>
          </div>
        ))}
      </div>
    </section>
  );
}

/** Mirrors `warden_server_protocol::protocol::LimitStatusDto` — the same shape the web UI gets. */
interface LimitStatus {
  id: string;
  scope: string;
  windowHours: number;
  usedTokens: number;
  maxTokens: number | null;
  usedCostUsd: number;
  maxCostUsd: number | null;
  fraction: number;
  warn: boolean;
  exceeded: boolean;
  unpricedCalls: number;
  freesUpInMinutes: number | null;
  extendTokens: number;
  extendCostUsd: number;
}

interface SpendStatus {
  limitsEnabled: boolean;
  limits: LimitStatus[];
  ledgerError: string | null;
}

function usd(value: number): string {
  return `$${value.toFixed(value < 1 ? 4 : 2)}`;
}

/** Where each spending limit (P4) stands, with "Allow more" on one that's running out — the same
 * grant the pause dialog makes mid-turn. The meter's fill carries severity (accent → warning →
 * danger), always next to an icon and a label, never color alone. */
function SpendingLimits() {
  const [status, setStatus] = useState<SpendStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [extending, setExtending] = useState<string | null>(null);

  function refresh() {
    invoke<SpendStatus>("spend_status")
      .then(setStatus)
      .catch((err) => setError(String(err)));
  }

  useEffect(refresh, []);

  async function extend(limitId: string) {
    setExtending(limitId);
    setError(null);
    try {
      await invoke("extend_spend_limit", { limitId });
      refresh();
    } catch (err) {
      setError(String(err));
    } finally {
      setExtending(null);
    }
  }

  return (
    <section className="settings-section">
      <div className="settings-section-header">
        <h3 className="settings-section-title">Spending limits</h3>
      </div>
      {error && <p className="settings-error-banner">{error}</p>}
      {status?.ledgerError && <p className="settings-error-banner">The spending ledger couldn't be written: {status.ledgerError}. Numbers may be low.</p>}
      {status === null ? (
        !error && <p className="settings-hint">Loading…</p>
      ) : !status.limitsEnabled ? (
        <p className="settings-hint">Spending limits are switched off (WARDEN_SPEND_LIMITS=off).</p>
      ) : status.limits.length === 0 ? (
        <p className="settings-hint">No limits configured. Add them in Settings.</p>
      ) : (
        <div className="spend-limit-list">
          {status.limits.map((limit) => {
            const state = limit.exceeded ? "exceeded" : limit.warn ? "warn" : "ok";
            const parts = [
              limit.maxTokens !== null ? `${limit.usedTokens.toLocaleString("en-US")} of ${limit.maxTokens.toLocaleString("en-US")} tokens` : null,
              limit.maxCostUsd !== null ? `${usd(limit.usedCostUsd)} of ${usd(limit.maxCostUsd)}` : null,
            ].filter(Boolean);
            const extension = [
              limit.extendTokens > 0 ? `+${formatCompact(limit.extendTokens)} tokens` : null,
              limit.extendCostUsd > 0 ? `+${usd(limit.extendCostUsd)}` : null,
            ]
              .filter(Boolean)
              .join(" / ");
            return (
              <div key={limit.id} className={`spend-limit spend-limit--${state}`}>
                <div className="spend-limit-header">
                  <span className="spend-limit-name">{limit.id}</span>
                  <span className="settings-hint">
                    {limit.scope} · last {limit.windowHours}h
                  </span>
                </div>
                <div
                  className="spend-meter"
                  role="meter"
                  aria-valuemin={0}
                  aria-valuemax={100}
                  aria-valuenow={Math.round(Math.min(limit.fraction, 1) * 100)}
                  aria-label={`Limit ${limit.id}`}
                >
                  <div className="spend-meter-fill" style={{ width: `${Math.min(limit.fraction, 1) * 100}%` }} />
                </div>
                <span className="spend-limit-detail">
                  {state === "exceeded" ? "⛔ Exhausted" : state === "warn" ? "⚠ Running low" : "✓ Room left"} · {Math.round(limit.fraction * 100)}% · {parts.join(" · ")}
                </span>
                {limit.freesUpInMinutes !== null && limit.fraction > 0 && (
                  <span className="settings-hint">Room starts coming back in ~{limit.freesUpInMinutes} min.</span>
                )}
                {limit.unpricedCalls > 0 && limit.maxCostUsd !== null && (
                  <span className="settings-hint">{limit.unpricedCalls} calls to models with no price — the dollar figure undercounts.</span>
                )}
                {state !== "ok" && extension && (
                  <button type="button" className="settings-browse-btn spend-limit-extend" disabled={extending === limit.id} onClick={() => void extend(limit.id)}>
                    {extending === limit.id ? "Allowing…" : `Allow ${extension} until the window ends`}
                  </button>
                )}
              </div>
            );
          })}
        </div>
      )}
    </section>
  );
}

function UsageView() {
  const [summary, setSummary] = useState<UsageSummary | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Fetched fresh every time this view mounts (the sidebar remounts it on each visit, same as
  // `SettingsView`) rather than cached in `App` — a dashboard showing yesterday's numbers right
  // after sending a message would be worse than the extra disk read.
  useEffect(() => {
    invoke<UsageSummary>("usage_summary")
      .then(setSummary)
      .catch((err) => setError(String(err)));
  }, []);

  if (error) {
    return (
      <div className="settings-view">
        <h2 className="settings-title">Usage</h2>
        <p className="usage-error">{error}</p>
      </div>
    );
  }

  if (!summary) {
    return (
      <div className="settings-view">
        <p>Loading usage…</p>
      </div>
    );
  }

  if (summary.messageCount === 0) {
    return (
      <div className="settings-view">
        <h2 className="settings-title">Usage</h2>
        <p className="settings-hint">No usage recorded yet — send a message to see stats here.</p>
        <SpendingLimits />
      </div>
    );
  }

  return (
    <div className="settings-view">
      <h2 className="settings-title">Usage</h2>
      <p className="settings-hint">
        Token usage across every conversation saved on this device. Dollar amounts show in the spending limits, for
        models that have a price in Settings.
      </p>

      <SpendingLimits />

      <div className="usage-stat-grid">
        <StatTile label="Total tokens" value={summary.total.totalTokens} />
        <StatTile label="Prompt tokens" value={summary.total.promptTokens} />
        <StatTile label="Completion tokens" value={summary.total.completionTokens} />
        <StatTile label="Model calls" value={summary.messageCount} />
        <StatTile label="Conversations" value={summary.conversationCount} />
      </div>

      <UsageBreakdown title="By agent" entries={summary.byAgent} emptyKeyLabel="No agent" />
      <UsageBreakdown title="By provider" entries={summary.byProvider} emptyKeyLabel="Default provider" />
    </div>
  );
}

export default UsageView;
