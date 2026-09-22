import { useEffect, useState } from "react";
import type { AgentEntry, LimitEntry, LimitScope, PriceEntry } from "../types";

/** The channels a limit can be about — the names `SpendContext` gets from each of them. */
const CHANNELS = ["desktop", "cli", "telegram", "whatsapp", "server"];

const SCOPE_OPTIONS: { value: LimitScope; label: string }[] = [
  { value: "global", label: "Everything together" },
  { value: "agent", label: "One agent" },
  { value: "channel", label: "One channel" },
  { value: "user", label: "One person on a channel" },
];

/** A blank field is `null`; anything that is not a number is `null` too, and the submit check
 * (`validateSpending`) is what tells the person, so a half-typed value never blocks typing. */
function parseNumber(raw: string, integer: boolean): number | null {
  const text = raw.trim();
  if (text === "") return null;
  const n = Number(text);
  if (!Number.isFinite(n)) return null;
  return integer ? Math.round(n) : n;
}

/** A text box for a number. It keeps what was typed (`0.` must stay `0.` while the next digit is
 * on its way) and only hands the parsed number up — resyncing when the value changes from outside,
 * like "Restore safety net". */
function NumberField({
  label,
  value,
  onChange,
  integer = false,
  placeholder,
  ariaLabel,
}: {
  label: string;
  value: number | null;
  onChange: (next: number | null) => void;
  integer?: boolean;
  placeholder?: string;
  ariaLabel?: string;
}) {
  const [draft, setDraft] = useState(value === null ? "" : String(value));
  useEffect(() => {
    setDraft((current) => (parseNumber(current, integer) === value ? current : value === null ? "" : String(value)));
  }, [value, integer]);
  return (
    <label className="settings-field">
      <span className="settings-label">{label}</span>
      <input
        className="settings-input"
        type="text"
        inputMode="decimal"
        placeholder={placeholder}
        aria-label={ariaLabel ?? label}
        value={draft}
        onChange={(e) => {
          const text = e.currentTarget.value;
          setDraft(text);
          onChange(parseNumber(text, integer));
        }}
      />
    </label>
  );
}

/** A fraction (0–1) shown as a percentage — `0.8` reads `80`. Rounded so `0.8 * 100` doesn't show
 * as `80.00000000000001`. */
function PercentField({
  label,
  value,
  onChange,
  placeholder,
}: {
  label: string;
  value: number | null;
  onChange: (next: number | null) => void;
  placeholder: string;
}) {
  return (
    <NumberField
      label={label}
      placeholder={placeholder}
      value={value === null ? null : Math.round(value * 1e4) / 100}
      onChange={(percent) => onChange(percent === null ? null : percent / 100)}
    />
  );
}

function nextLimitId(existing: LimitEntry[]): string {
  let n = existing.length + 1;
  while (existing.some((l) => l.id === `limit-${n}`)) n += 1;
  return `limit-${n}`;
}

const formatTokens = (n: number) => n.toLocaleString("en-US");

/** The limit in one plain sentence, so the numbers don't have to be decoded from the form. */
export function describeLimit(limit: LimitEntry): string {
  const who =
    limit.scope === "global"
      ? "everything together"
      : limit.scope === "agent"
        ? `the agent "${limit.target || "…"}"`
        : limit.scope === "channel"
          ? `the ${limit.target || "…"} channel`
          : `${limit.target || "…"}`;
  const ceilings = [
    limit.maxTokens !== null ? `${formatTokens(limit.maxTokens)} tokens` : null,
    limit.maxCostUsd !== null ? `$${limit.maxCostUsd}` : null,
  ].filter((c) => c !== null);
  const window = limit.windowHours === 1 ? "any 1 hour" : `any ${limit.windowHours} hours`;
  return `${who[0].toUpperCase()}${who.slice(1)}: pauses at ${ceilings.join(" or ") || "…"} within ${window}.`;
}

function LimitCard({
  limit,
  agents,
  onChange,
  onDelete,
}: {
  limit: LimitEntry;
  agents: AgentEntry[];
  onChange: (next: LimitEntry) => void;
  onDelete: () => void;
}) {
  const knownAgent = agents.some((a) => a.id === limit.target);
  const knownChannel = CHANNELS.includes(limit.target);
  return (
    <div className="provider-card">
      <div className="provider-card-header">
        <input
          className="settings-input provider-name-input"
          type="text"
          placeholder="Name (e.g. daily-budget)"
          value={limit.id}
          onChange={(e) => onChange({ ...limit, id: e.currentTarget.value })}
        />
        <button
          type="button"
          className="provider-delete-btn"
          onClick={onDelete}
          aria-label={`Delete ${limit.id || "this limit"}`}
          title="Delete this limit"
        >
          🗑
        </button>
      </div>

      <div className="spend-grid">
        <label className="settings-field">
          <span className="settings-label">Applies to</span>
          <select
            className="settings-select"
            value={limit.scope}
            onChange={(e) => onChange({ ...limit, scope: e.currentTarget.value as LimitScope, target: "" })}
          >
            {SCOPE_OPTIONS.map((o) => (
              <option key={o.value} value={o.value}>
                {o.label}
              </option>
            ))}
          </select>
        </label>

        {limit.scope === "agent" && (
          <label className="settings-field">
            <span className="settings-label">Agent</span>
            <select className="settings-select" value={limit.target} onChange={(e) => onChange({ ...limit, target: e.currentTarget.value })}>
              <option value="">(pick an agent)</option>
              {!knownAgent && limit.target !== "" && <option value={limit.target}>{limit.target} (no longer exists)</option>}
              {agents.map((a) => (
                <option key={a.id} value={a.id}>
                  {a.id}
                </option>
              ))}
            </select>
          </label>
        )}
        {limit.scope === "channel" && (
          <label className="settings-field">
            <span className="settings-label">Channel</span>
            <select className="settings-select" value={limit.target} onChange={(e) => onChange({ ...limit, target: e.currentTarget.value })}>
              <option value="">(pick a channel)</option>
              {!knownChannel && limit.target !== "" && <option value={limit.target}>{limit.target}</option>}
              {CHANNELS.map((c) => (
                <option key={c} value={c}>
                  {c}
                </option>
              ))}
            </select>
          </label>
        )}
        {limit.scope === "user" && (
          <label className="settings-field">
            <span className="settings-label">Person (channel:user)</span>
            <input
              className="settings-input"
              type="text"
              placeholder="telegram:12345"
              value={limit.target}
              onChange={(e) => onChange({ ...limit, target: e.currentTarget.value })}
            />
          </label>
        )}

        <NumberField label="Window (hours)" integer value={limit.windowHours} onChange={(n) => onChange({ ...limit, windowHours: n ?? 0 })} />
        <NumberField label="Max tokens" integer placeholder="no cap" value={limit.maxTokens} onChange={(n) => onChange({ ...limit, maxTokens: n })} />
        <NumberField label="Max cost ($)" placeholder="no cap" value={limit.maxCostUsd} onChange={(n) => onChange({ ...limit, maxCostUsd: n })} />
        <PercentField label="Warn the model at (%)" placeholder="80" value={limit.warnAt} onChange={(n) => onChange({ ...limit, warnAt: n })} />
        <PercentField label="Each extension adds (%)" placeholder="25" value={limit.extendStep} onChange={(n) => onChange({ ...limit, extendStep: n })} />
      </div>
      <p className="settings-hint">{describeLimit(limit)}</p>
    </div>
  );
}

function PriceRow({
  price,
  onChange,
  onDelete,
}: {
  price: PriceEntry;
  onChange: (next: PriceEntry) => void;
  onDelete: () => void;
}) {
  return (
    <div className="spend-price-row">
      <input
        className="settings-input"
        type="text"
        list="spend-model-suggestions"
        placeholder="Model id (exactly as the provider reports it)"
        aria-label="Model id"
        value={price.model}
        onChange={(e) => onChange({ ...price, model: e.currentTarget.value })}
      />
      {/* A blank or unreadable price is kept as NaN so `validateSpending` names it on save; the field
          itself sees it as "no number" (NaN never equals itself, which would fight the resync). */}
      <NumberField
        label="Input $ / 1M tokens"
        ariaLabel={`Input price for ${price.model || "this model"}`}
        value={Number.isNaN(price.inputPerMtok) ? null : price.inputPerMtok}
        onChange={(n) => onChange({ ...price, inputPerMtok: n ?? NaN })}
      />
      <NumberField
        label="Output $ / 1M tokens"
        ariaLabel={`Output price for ${price.model || "this model"}`}
        value={Number.isNaN(price.outputPerMtok) ? null : price.outputPerMtok}
        onChange={(n) => onChange({ ...price, outputPerMtok: n ?? NaN })}
      />
      <button type="button" className="provider-delete-btn" onClick={onDelete} aria-label={`Delete price for ${price.model || "this model"}`} title="Delete this price">
        🗑
      </button>
    </div>
  );
}

/** Mirrors `spend_cmds` (the backend stays the authority) so a mistake is caught before the IPC
 * round-trip and named the way the form names things. `null` = fine. */
export function validateSpending(limits: LimitEntry[] | null, prices: PriceEntry[]): string | null {
  const ids = new Set<string>();
  for (const l of limits ?? []) {
    const id = l.id.trim();
    if (id === "") return "Every spending limit needs a name.";
    if (ids.has(id)) return `Duplicate limit name: ${id}`;
    ids.add(id);
    if (!(l.windowHours >= 1)) return `Limit '${id}': the window must be at least 1 hour.`;
    if (l.maxTokens === null && l.maxCostUsd === null) return `Limit '${id}': set a token cap, a cost cap, or both.`;
    if (l.maxTokens !== null && !(l.maxTokens > 0)) return `Limit '${id}': the token cap must be above 0.`;
    if (l.maxCostUsd !== null && !(l.maxCostUsd > 0)) return `Limit '${id}': the cost cap must be above 0.`;
    if (l.scope !== "global" && l.target.trim() === "") return `Limit '${id}': pick who it applies to.`;
    if (l.scope === "user" && !l.target.includes(":")) return `Limit '${id}': write the person as channel:user, e.g. telegram:12345.`;
    if (l.warnAt !== null && !(l.warnAt > 0 && l.warnAt <= 1)) return `Limit '${id}': "Warn the model at" must be between 1 and 100.`;
    if (l.extendStep !== null && !(l.extendStep > 0)) return `Limit '${id}': "Each extension adds" must be above 0.`;
  }
  const models = new Set<string>();
  for (const p of prices) {
    const model = p.model.trim();
    if (model === "") return "Every price needs a model id.";
    if (models.has(model)) return `Duplicate price for model: ${model}`;
    models.add(model);
    if (!(p.inputPerMtok >= 0) || !(p.outputPerMtok >= 0)) return `Price for '${model}': both prices must be numbers of 0 or more.`;
  }
  return null;
}

export default function SpendingSection({
  limits,
  defaultLimits,
  disabledByEnv,
  prices,
  agents,
  modelSuggestions,
  onLimitsChange,
  onPricesChange,
}: {
  limits: LimitEntry[] | null;
  defaultLimits: LimitEntry[];
  disabledByEnv: boolean;
  prices: PriceEntry[];
  agents: AgentEntry[];
  /** Model ids already known from the providers, offered while typing a price's model. */
  modelSuggestions: string[];
  onLimitsChange: (next: LimitEntry[] | null) => void;
  onPricesChange: (next: PriceEntry[]) => void;
}) {
  const addLimit = () =>
    onLimitsChange([
      ...(limits ?? []),
      { id: nextLimitId(limits ?? []), scope: "global", target: "", windowHours: 24, maxTokens: null, maxCostUsd: null, warnAt: null, extendStep: null },
    ]);

  return (
    <section className="settings-section">
      <div className="settings-section-header">
        <h3 className="settings-section-title">Spending limits</h3>
        {limits !== null && (
          <button type="button" className="settings-browse-btn" onClick={addLimit}>
            + Add limit
          </button>
        )}
      </div>
      <p className="settings-hint">
        A ceiling on how much the model may use in any sliding stretch of time, so an agent stuck in a loop can't burn
        through your account. When a limit runs out the turn pauses and asks you whether to allow a bit more (here and in the
        terminal); Telegram, WhatsApp and the server simply decline. The agent can see how much is left and is told when it
        gets close.
      </p>

      {disabledByEnv && (
        <p className="settings-warning-banner">
          <code>WARDEN_SPEND_LIMITS=off</code> is set in the environment, so every limit below is ignored until it is removed.
        </p>
      )}

      {limits === null && (
        <div className="provider-card">
          <p className="settings-label">Built-in safety net is on</p>
          <ul className="spend-net-list">
            {defaultLimits.map((l) => (
              <li key={l.id} className="settings-hint">
                {describeLimit(l)}
              </li>
            ))}
          </ul>
          <div className="mcp-quick-add-row">
            <button type="button" className="settings-browse-btn" onClick={() => onLimitsChange(defaultLimits.map((l) => ({ ...l })))}>
              Customize limits
            </button>
            <button type="button" className="settings-browse-btn" onClick={() => onLimitsChange([])}>
              Turn all limits off
            </button>
          </div>
        </div>
      )}

      {limits !== null && limits.length === 0 && (
        <p className="settings-warning-banner">
          Every spending limit is off — nothing stops a runaway agent. Add one below or restore the safety net.
        </p>
      )}

      {limits !== null && (
        <>
          <div className="provider-list">
            {limits.map((l, i) => (
              <LimitCard
                key={i}
                limit={l}
                agents={agents}
                onChange={(next) => onLimitsChange(limits.map((x, j) => (j === i ? next : x)))}
                onDelete={() => onLimitsChange(limits.filter((_, j) => j !== i))}
              />
            ))}
          </div>
          <div className="mcp-quick-add-row">
            <button type="button" className="settings-browse-btn" onClick={() => onLimitsChange(null)}>
              Restore built-in safety net
            </button>
          </div>
        </>
      )}

      <div className="settings-section-header">
        <h4 className="settings-label">Model prices</h4>
        <button type="button" className="settings-browse-btn" onClick={() => onPricesChange([...prices, { model: "", inputPerMtok: 0, outputPerMtok: 0 }])}>
          + Add price
        </button>
      </div>
      <p className="settings-hint">
        Needed for a dollar cap. Nothing is built in — prices change, and a wrong figure that looks right is worse than none —
        so a model with no price here counts against token caps only, and the dollar total says how many calls it left out.
      </p>
      <datalist id="spend-model-suggestions">
        {modelSuggestions.map((m) => (
          <option key={m} value={m} />
        ))}
      </datalist>
      {prices.length === 0 && <p className="settings-hint">No prices yet.</p>}
      <div className="provider-list">
        {prices.map((p, i) => (
          <PriceRow
            key={i}
            price={p}
            onChange={(next) => onPricesChange(prices.map((x, j) => (j === i ? next : x)))}
            onDelete={() => onPricesChange(prices.filter((_, j) => j !== i))}
          />
        ))}
      </div>
    </section>
  );
}
