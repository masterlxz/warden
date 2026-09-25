import { useCallback, useEffect, useState } from "react";
import type { ServerConnection } from "../hub/connection";
import type { LimitStatus, SpendBucket, UsageReport } from "../hub/messages";

// Usage across the whole hub (P78): tokens from every device's conversations (all time), and the
// spending limits and recent dollars from the P4 ledger, which every channel on the hub's machine
// shares. A limit that ran out can be extended from here, like the desktop's pause dialog.

const compact = new Intl.NumberFormat("pt-BR", { notation: "compact", maximumFractionDigits: 1 });
const full = new Intl.NumberFormat("pt-BR");

function usd(value: number): string {
  return `US$ ${value.toLocaleString("pt-BR", { minimumFractionDigits: 2, maximumFractionDigits: value < 1 ? 4 : 2 })}`;
}

function message(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

/** `2026-09-25` → `25/09`. */
function shortDate(date: string): string {
  const [, month, day] = date.split("-");
  return `${day}/${month}`;
}

/** `LimitStatus.scope` as the hub writes it (`global`, `agent x`, `channel x`, `user channel:id`). */
function scopeLabel(scope: string): string {
  const [kind, ...rest] = scope.split(" ");
  const target = rest.join(" ");
  switch (kind) {
    case "global":
      return "tudo";
    case "agent":
      return `agente ${target}`;
    case "channel":
      return `canal ${target}`;
    case "user":
      return `usuário ${target}`;
    default:
      return scope;
  }
}

function windowLabel(hours: number): string {
  return hours % 24 === 0 ? (hours === 24 ? "24 h" : `${hours / 24} dias`) : `${hours} h`;
}

function Tile({ label, value, title }: { label: string; value: string; title?: string }) {
  return (
    <div className="usage-tile" title={title}>
      <span className="usage-tile-value">{value}</span>
      <span className="usage-tile-label">{label}</span>
    </div>
  );
}

/** Tokens per day: one series, so no legend — the heading names it. Each column is its own hover
 * and focus target; the table below carries the exact numbers. */
function DailyChart({ daily }: { daily: UsageReport["daily"] }) {
  const max = Math.max(...daily.map((d) => d.tokens), 0);
  return (
    <figure className="usage-daily">
      <div className="usage-daily-plot">
        <span className="usage-daily-axis">{max > 0 ? compact.format(max) : "0"}</span>
        <div className="usage-daily-columns" role="list">
          {daily.map((d) => (
            <div
              key={d.date}
              role="listitem"
              tabIndex={0}
              className="usage-daily-slot"
              aria-label={`${shortDate(d.date)}: ${full.format(d.tokens)} tokens em ${d.calls} chamadas`}
            >
              {d.tokens > 0 && <div className="usage-daily-bar" style={{ height: `${Math.max((d.tokens / max) * 100, 2)}%` }} />}
              <span className="usage-daily-tip" role="tooltip">
                <strong>{shortDate(d.date)}</strong>
                {full.format(d.tokens)} tokens · {d.calls} {d.calls === 1 ? "chamada" : "chamadas"}
              </span>
            </div>
          ))}
        </div>
      </div>
      <div className="usage-daily-dates">
        <span>{daily.length > 0 && shortDate(daily[0].date)}</span>
        <span>hoje</span>
      </div>
      <details className="usage-table-toggle">
        <summary>Ver tabela</summary>
        <table className="usage-table">
          <thead>
            <tr>
              <th>Dia</th>
              <th>Chamadas</th>
              <th>Tokens</th>
            </tr>
          </thead>
          <tbody>
            {daily
              .filter((d) => d.calls > 0)
              .reverse()
              .map((d) => (
                <tr key={d.date}>
                  <td>{shortDate(d.date)}</td>
                  <td>{full.format(d.calls)}</td>
                  <td>{full.format(d.tokens)}</td>
                </tr>
              ))}
          </tbody>
        </table>
      </details>
    </figure>
  );
}

function LimitCard({ limit, busy, onExtend }: { limit: LimitStatus; busy: boolean; onExtend: () => void }) {
  const state = limit.exceeded ? "exceeded" : limit.warn ? "warn" : "ok";
  const parts: string[] = [];
  if (limit.maxTokens !== null) parts.push(`${full.format(limit.usedTokens)} de ${full.format(limit.maxTokens)} tokens`);
  if (limit.maxCostUsd !== null) parts.push(`${usd(limit.usedCostUsd)} de ${usd(limit.maxCostUsd)}`);
  const extension = [
    limit.extendTokens > 0 ? `+${compact.format(limit.extendTokens)} tokens` : null,
    limit.extendCostUsd > 0 ? `+${usd(limit.extendCostUsd)}` : null,
  ]
    .filter(Boolean)
    .join(" / ");

  return (
    <li className={`usage-limit usage-limit--${state}`}>
      <div className="usage-limit-header">
        <span className="usage-limit-name">{limit.id}</span>
        <span className="usage-limit-scope">
          {scopeLabel(limit.scope)} · últimas {windowLabel(limit.windowHours)}
        </span>
      </div>
      <div
        className="usage-meter"
        role="meter"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={Math.round(Math.min(limit.fraction, 1) * 100)}
        aria-label={`Limite ${limit.id}`}
      >
        <div className="usage-meter-fill" style={{ width: `${Math.min(limit.fraction, 1) * 100}%` }} />
      </div>
      <div className="usage-limit-detail">
        <span>
          {state === "exceeded" ? "⛔ Esgotado" : state === "warn" ? "⚠ Perto do limite" : "✓ Com folga"} · {Math.round(limit.fraction * 100)}% ·{" "}
          {parts.join(" · ")}
        </span>
        {limit.freesUpInMinutes !== null && limit.fraction > 0 && <span className="skills-hint">A folga começa a voltar em ~{limit.freesUpInMinutes} min.</span>}
        {limit.unpricedCalls > 0 && limit.maxCostUsd !== null && (
          <span className="skills-hint">
            {limit.unpricedCalls} {limit.unpricedCalls === 1 ? "chamada" : "chamadas"} sem preço cadastrado: o valor em US$ está subestimado.
          </span>
        )}
      </div>
      {state !== "ok" && extension && (
        <button type="button" className="link-button usage-limit-extend" disabled={busy} onClick={onExtend}>
          {busy ? "Liberando…" : `Liberar ${extension} até o fim da janela`}
        </button>
      )}
    </li>
  );
}

function SpendTable({ title, buckets, keyLabel }: { title: string; buckets: SpendBucket[]; keyLabel: string }) {
  if (buckets.length === 0) return null;
  return (
    <table className="usage-table">
      <caption>{title}</caption>
      <thead>
        <tr>
          <th>{keyLabel}</th>
          <th>Chamadas</th>
          <th>Tokens</th>
          <th>US$</th>
        </tr>
      </thead>
      <tbody>
        {buckets.map((b) => (
          <tr key={b.key}>
            <td className="usage-table-key">{b.key}</td>
            <td>{full.format(b.calls)}</td>
            <td>{full.format(b.tokens)}</td>
            <td>
              {b.calls > b.unpricedCalls ? usd(b.costUsd) : "—"}
              {b.unpricedCalls > 0 && b.calls > b.unpricedCalls && <span className="skills-hint"> ({b.unpricedCalls} sem preço)</span>}
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

export default function UsageView({ conn }: { conn: ServerConnection | null }) {
  const [report, setReport] = useState<UsageReport | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [extending, setExtending] = useState<string | null>(null);

  const refresh = useCallback(() => {
    if (!conn) return;
    conn.requestUsage().then(
      (r) => {
        setReport(r);
        setError(null);
      },
      (err) => setError(`Não foi possível carregar o uso: ${message(err)}`),
    );
  }, [conn]);

  // Also re-runs after a reconnect, when `conn` is a new connection.
  useEffect(refresh, [refresh]);

  async function extend(limitId: string) {
    if (!conn) return;
    setExtending(limitId);
    try {
      await conn.extendLimit(limitId);
      refresh();
    } catch (err) {
      setError(`Não foi possível liberar o limite: ${message(err)}`);
    } finally {
      setExtending(null);
    }
  }

  if (report === null) {
    return <div className="usage-view">{error ? <p className="error-banner">{error}</p> : <p className="skills-hint">Carregando…</p>}</div>;
  }

  const maxDevice = Math.max(...report.byDevice.map((d) => d.usage.totalTokens), 1);
  const totalCost = report.recent?.byModel.reduce((sum, b) => sum + b.costUsd, 0) ?? 0;

  return (
    <div className="usage-view">
      <div className="skills-toolbar">
        <span className="skills-hint">Todo o hub: as conversas de todos os devices e os limites de gasto desta máquina.</span>
        <button type="button" className="link-button" onClick={refresh} disabled={!conn}>
          Atualizar
        </button>
      </div>
      {error && <p className="error-banner">{error}</p>}

      <div className="usage-tiles">
        <Tile label="Tokens" value={compact.format(report.total.totalTokens)} title={`${full.format(report.total.totalTokens)} tokens`} />
        <Tile label="Chamadas ao modelo" value={compact.format(report.messageCount)} />
        <Tile label="Conversas" value={compact.format(report.conversationCount)} />
        {report.recent && (
          <Tile label={`Gasto nas últimas ${windowLabel(report.recent.windowHours)}`} value={totalCost > 0 ? usd(totalCost) : "—"} />
        )}
      </div>

      <section className="usage-section">
        <h2 className="usage-heading">Tokens por dia, últimos {report.daily.length} dias</h2>
        <DailyChart daily={report.daily} />
      </section>

      <section className="usage-section">
        <h2 className="usage-heading">Limites de gasto</h2>
        {report.ledgerError && <p className="error-banner">O registro de gasto não pôde ser gravado: {report.ledgerError}. Os números podem estar baixos.</p>}
        {!report.limitsEnabled ? (
          <p className="skills-hint">Os limites estão desligados neste hub (WARDEN_SPEND_LIMITS=off).</p>
        ) : report.limits.length === 0 ? (
          <p className="skills-hint">Nenhum limite configurado.</p>
        ) : (
          <ul className="usage-limits">
            {report.limits.map((limit) => (
              <LimitCard key={limit.id} limit={limit} busy={extending === limit.id || !conn} onExtend={() => void extend(limit.id)} />
            ))}
          </ul>
        )}
      </section>

      <section className="usage-section">
        <h2 className="usage-heading">Por device</h2>
        {report.byDevice.length === 0 ? (
          <p className="skills-hint">Nenhuma conversa ainda.</p>
        ) : (
          <ul className="usage-bars">
            {report.byDevice.map((d) => (
              <li key={d.deviceId} className="usage-bar-row">
                <span className="usage-bar-label" title={d.deviceId}>
                  {d.name ?? d.deviceId}
                </span>
                <div className="usage-bar-track">
                  <div className="usage-bar-fill" style={{ width: `${(d.usage.totalTokens / maxDevice) * 100}%` }} />
                </div>
                <span className="usage-bar-value" title={`${full.format(d.usage.totalTokens)} tokens em ${d.conversationCount} conversas`}>
                  {compact.format(d.usage.totalTokens)}
                </span>
              </li>
            ))}
          </ul>
        )}
      </section>

      {report.recent && (report.recent.byModel.length > 0 || report.recent.byChannel.length > 0) && (
        <section className="usage-section">
          <h2 className="usage-heading">Gasto recente, últimas {windowLabel(report.recent.windowHours)}</h2>
          <p className="skills-hint">Do registro de gasto, que só guarda a janela do limite mais longo. Inclui desktop, Telegram e os outros canais desta máquina.</p>
          <SpendTable title="Por modelo" buckets={report.recent.byModel} keyLabel="Modelo" />
          <SpendTable title="Por canal" buckets={report.recent.byChannel} keyLabel="Canal" />
        </section>
      )}
    </div>
  );
}
