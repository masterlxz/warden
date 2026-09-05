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
      </div>
    );
  }

  return (
    <div className="settings-view">
      <h2 className="settings-title">Usage</h2>
      <p className="settings-hint">
        Token usage across every conversation saved on this device. Token counts only — there's no dollar cost estimate
        yet (no per-model price table).
      </p>

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
