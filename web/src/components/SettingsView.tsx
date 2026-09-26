import { useCallback, useEffect, useMemo, useState, type ReactNode } from "react";
import { SettingsError, type LoadedSettings, type ServerConnection } from "../hub/connection";
import type {
  AgentSettings,
  HubSettings,
  HubSettingsUpdate,
  LimitScope,
  LimitSettings,
  PriceSettings,
  ProviderKind,
  SecretEdit,
  SecretStatus,
} from "../hub/messages";

// The hub's settings (P78): providers, agents, the Tavily/Whisper keys, spending limits and prices.
// Everything is edited as one draft and saved at once; the hub asks for the pairing key again on
// every save, checks the file wasn't changed meanwhile, and restarts its orchestrator with the new
// settings (or keeps the old ones if it can't start with them). API keys never come back from the
// hub: a key field shows whether one is saved and can only be replaced or removed.

const KINDS: { value: ProviderKind; label: string }[] = [
  { value: "gemini", label: "Gemini" },
  { value: "openai", label: "OpenAI" },
  { value: "anthropic", label: "Anthropic" },
  { value: "openai_compatible", label: "Compatível com OpenAI (Ollama, OpenRouter…)" },
];

const SCOPES: { value: LimitScope; label: string; target: string }[] = [
  { value: "global", label: "Tudo", target: "" },
  { value: "agent", label: "Um agente", target: "id do agente" },
  { value: "channel", label: "Um canal", target: "server, desktop, telegram…" },
  { value: "user", label: "Um usuário", target: "canal:id, ex. telegram:12345" },
];

type Keyed<T> = T & { key: number };

interface SecretDraft {
  saved: SecretStatus;
  edit: SecretEdit;
}

type ProviderDraft = Keyed<{ originalId?: string; id: string; kind: ProviderKind; baseUrl: string; model: string; apiKey: SecretDraft }>;

type LimitsMode = "default" | "custom" | "off";

interface Draft {
  providers: ProviderDraft[];
  activeProvider: string;
  agents: Keyed<AgentSettings>[];
  tavilyKey: SecretDraft;
  whisperKey: SecretDraft;
  limitsMode: LimitsMode;
  limits: Keyed<LimitSettings>[];
  prices: Keyed<PriceSettings>[];
}

let nextKey = 1;
function keyed<T>(value: T): Keyed<T> {
  return { ...value, key: nextKey++ };
}

const KEEP: SecretEdit = { action: "keep" };

function toDraft(s: HubSettings): Draft {
  return {
    providers: s.providers.map((p) => keyed({ originalId: p.id, id: p.id, kind: p.kind, baseUrl: p.baseUrl, model: p.model, apiKey: { saved: p.apiKey, edit: KEEP } })),
    activeProvider: s.activeProvider,
    agents: s.agents.map((a) => keyed({ ...a, originalId: a.id })),
    tavilyKey: { saved: s.tavilyKey, edit: KEEP },
    whisperKey: { saved: s.whisperKey, edit: KEEP },
    limitsMode: s.limits === null ? "default" : s.limits.length === 0 ? "off" : "custom",
    limits: (s.limits ?? []).map(keyed),
    prices: s.prices.map(keyed),
  };
}

function strip<T>({ key: _key, ...rest }: Keyed<T>): T {
  return rest as T;
}

function toUpdate(d: Draft): HubSettingsUpdate {
  return {
    providers: d.providers.map((p) => ({ originalId: p.originalId, id: p.id, kind: p.kind, baseUrl: p.baseUrl, model: p.model, apiKey: p.apiKey.edit })),
    activeProvider: d.activeProvider,
    agents: d.agents.map(strip),
    tavilyKey: d.tavilyKey.edit,
    whisperKey: d.whisperKey.edit,
    limits: d.limitsMode === "default" ? null : d.limitsMode === "off" ? [] : d.limits.map(strip),
    prices: d.prices.map(strip),
  };
}

function message(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

function numberOrNull(value: string): number | null {
  return value.trim() === "" ? null : Number(value);
}

function Section({ title, hint, action, children }: { title: string; hint?: string; action?: ReactNode; children: ReactNode }) {
  return (
    <section className="usage-section settings-section">
      <div className="settings-section-header">
        <h2 className="usage-heading">{title}</h2>
        {action}
      </div>
      {hint && <p className="skills-hint">{hint}</p>}
      {children}
    </section>
  );
}

function Field({ label, hint, children, wide }: { label: string; hint?: string; children: ReactNode; wide?: boolean }) {
  return (
    <label className={wide ? "settings-field settings-field--wide" : "settings-field"}>
      {label}
      {children}
      {hint && <span className="field-hint">{hint}</span>}
    </label>
  );
}

/** A saved secret the page never sees: shows whether one is there and lets it be replaced or removed. */
function SecretField({ label, value, writable, onChange }: { label: string; value: SecretDraft; writable: boolean; onChange: (edit: SecretEdit) => void }) {
  const { saved, edit } = value;
  let status: string;
  if (edit.action === "set") status = "Nova chave (ainda não salva)";
  else if (edit.action === "clear") status = "Será removida ao salvar";
  else if (saved.set) status = saved.hint ? `Salva, termina em …${saved.hint}` : "Salva";
  else status = "Nenhuma";

  return (
    <div className="settings-field settings-field--wide">
      <span className="settings-secret-label">{label}</span>
      {edit.action === "set" ? (
        <input
          type="password"
          autoComplete="new-password"
          placeholder="Cole a chave"
          value={edit.value}
          onChange={(e) => onChange({ action: "set", value: e.target.value })}
        />
      ) : (
        <span className={edit.action === "clear" ? "settings-secret-status skills-danger" : "settings-secret-status"}>{status}</span>
      )}
      <span className="skills-actions">
        {edit.action === "keep" ? (
          <>
            <button type="button" className="link-button" disabled={!writable} onClick={() => onChange({ action: "set", value: "" })}>
              {saved.set ? "Trocar" : "Adicionar"}
            </button>
            {saved.set && (
              <button type="button" className="link-button skills-danger" onClick={() => onChange({ action: "clear" })}>
                Remover
              </button>
            )}
          </>
        ) : (
          <button type="button" className="link-button" onClick={() => onChange(KEEP)}>
            Desfazer
          </button>
        )}
      </span>
    </div>
  );
}

export default function SettingsView({ conn }: { conn: ServerConnection | null }) {
  const [loaded, setLoaded] = useState<LoadedSettings | null>(null);
  const [draft, setDraft] = useState<Draft | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [saveError, setSaveError] = useState<{ text: string; conflict: boolean } | null>(null);
  const [asking, setAsking] = useState(false);
  const [pairingKey, setPairingKey] = useState("");
  const [keyError, setKeyError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [savedAt, setSavedAt] = useState<number | null>(null);

  const load = useCallback(() => {
    if (!conn) return;
    conn.requestSettings().then(
      (result) => {
        setLoaded(result);
        setDraft(toDraft(result.settings));
        setLoadError(null);
        setSaveError(null);
      },
      (err) => setLoadError(`Não foi possível carregar as configurações: ${message(err)}`),
    );
  }, [conn]);

  // Also re-runs after a reconnect, when `conn` is a new connection.
  useEffect(load, [load]);

  const dirty = useMemo(() => {
    if (!loaded || !draft) return false;
    return JSON.stringify(toUpdate(draft)) !== JSON.stringify(toUpdate(toDraft(loaded.settings)));
  }, [loaded, draft]);

  if (!loaded || !draft) {
    return <div className="usage-view">{loadError ? <p className="error-banner">{loadError}</p> : <p className="skills-hint">Carregando…</p>}</div>;
  }

  const { settings, secretsWritable } = loaded;
  const update = (change: (d: Draft) => Draft) => {
    setDraft((d) => (d ? change(d) : d));
    setSavedAt(null);
  };

  function patchProvider(key: number, patch: Partial<ProviderDraft>) {
    update((d) => {
      const before = d.providers.find((p) => p.key === key);
      const providers = d.providers.map((p) => (p.key === key ? { ...p, ...patch } : p));
      // A renamed provider stays the active one and every agent's default (same cascade as the desktop).
      if (before && patch.id !== undefined && patch.id !== before.id) {
        return {
          ...d,
          providers,
          activeProvider: d.activeProvider === before.id ? patch.id : d.activeProvider,
          agents: d.agents.map((a) => (a.providerId === before.id ? { ...a, providerId: patch.id! } : a)),
        };
      }
      return { ...d, providers };
    });
  }

  function removeProvider(key: number) {
    update((d) => {
      const removed = d.providers.find((p) => p.key === key);
      return {
        ...d,
        providers: d.providers.filter((p) => p.key !== key),
        activeProvider: d.activeProvider === removed?.id ? "" : d.activeProvider,
        agents: d.agents.map((a) => (a.providerId === removed?.id ? { ...a, providerId: "" } : a)),
      };
    });
  }

  function patchAgent(key: number, patch: Partial<AgentSettings>) {
    update((d) => ({ ...d, agents: d.agents.map((a) => (a.key === key ? { ...a, ...patch } : a)) }));
  }

  function patchLimit(key: number, patch: Partial<LimitSettings>) {
    update((d) => ({ ...d, limits: d.limits.map((l) => (l.key === key ? { ...l, ...patch } : l)) }));
  }

  function patchPrice(key: number, patch: Partial<PriceSettings>) {
    update((d) => ({ ...d, prices: d.prices.map((p) => (p.key === key ? { ...p, ...patch } : p)) }));
  }

  function setLimitsMode(mode: LimitsMode) {
    update((d) => ({
      ...d,
      limitsMode: mode,
      // "Customize" starts from the built-in limits, so the numbers live in one place (the hub).
      limits: mode === "custom" && d.limits.length === 0 ? settings.defaultLimits.map(keyed) : d.limits,
    }));
  }

  async function save() {
    if (!conn || !draft || !loaded) return;
    setSaving(true);
    setKeyError(null);
    setSaveError(null);
    try {
      const result = await conn.saveSettings(pairingKey, loaded.version, toUpdate(draft));
      setLoaded({ ...loaded, settings: result.settings, version: result.version });
      setDraft(toDraft(result.settings));
      setAsking(false);
      setPairingKey("");
      setSavedAt(Date.now());
    } catch (err) {
      if (err instanceof SettingsError && err.authRejected) {
        setKeyError("Chave de pareamento errada.");
      } else {
        setAsking(false);
        setPairingKey("");
        setSaveError({ text: message(err), conflict: err instanceof SettingsError && err.conflict });
      }
    } finally {
      setSaving(false);
    }
  }

  const providerIds = draft.providers.map((p) => p.id.trim()).filter((id) => id !== "");

  return (
    <div className="usage-view settings-view">
      <div className="skills-toolbar">
        <span className="skills-hint">A parte da configuração do hub que dá para mexer daqui. Shell, MCP, SSH e armazenamento ficam no desktop ou no config.toml.</span>
        <button type="button" className="link-button" onClick={load} disabled={!conn || saving}>
          Recarregar
        </button>
      </div>

      {settings.notes.map((note) => (
        <p key={note} className="banner settings-note">
          {note}
        </p>
      ))}
      {!secretsWritable && (
        <p className="banner settings-note">
          Esta página está em http:// vindo de outra máquina, então trocar ou adicionar chaves de API fica bloqueado: a chave passaria sem
          criptografia pela rede. Abra pelo endereço https:// do hub (Tailscale) ou no próprio computador dele. Remover uma chave e o
          resto das configurações funcionam normalmente.
        </p>
      )}

      <Section
        title="Provedores de modelo"
        hint="O provedor ativo responde todas as conversas do hub. Deixe o modelo vazio para usar o padrão do tipo."
        action={
          <button
            type="button"
            className="link-button"
            onClick={() =>
              update((d) => ({
                ...d,
                providers: [...d.providers, keyed({ id: "", kind: "gemini" as ProviderKind, baseUrl: "", model: "", apiKey: { saved: { set: false }, edit: KEEP } })],
              }))
            }
          >
            + Provedor
          </button>
        }
      >
        {draft.providers.length === 0 && <p className="skills-hint">Nenhum provedor salvo.</p>}
        <ul className="skills-list">
          {draft.providers.map((p) => (
            <li key={p.key} className="skills-item settings-card">
              <div className="skills-item-header">
                <label className="settings-radio">
                  <input
                    type="radio"
                    name="active-provider"
                    checked={p.id.trim() !== "" && draft.activeProvider === p.id}
                    disabled={p.id.trim() === ""}
                    onChange={() => update((d) => ({ ...d, activeProvider: p.id }))}
                  />
                  {draft.activeProvider === p.id && p.id.trim() !== "" ? "Ativo" : "Usar este"}
                </label>
                <button type="button" className="link-button skills-danger" onClick={() => removeProvider(p.key)}>
                  Remover
                </button>
              </div>
              <div className="settings-grid">
                <Field label="Nome">
                  <input value={p.id} onChange={(e) => patchProvider(p.key, { id: e.target.value })} />
                </Field>
                <Field label="Tipo">
                  <select value={p.kind} onChange={(e) => patchProvider(p.key, { kind: e.target.value as ProviderKind })}>
                    {KINDS.map((k) => (
                      <option key={k.value} value={k.value}>
                        {k.label}
                      </option>
                    ))}
                  </select>
                </Field>
                <Field label="Modelo">
                  <input
                    value={p.model}
                    placeholder={settings.defaultModels[p.kind] ?? "obrigatório para este tipo"}
                    onChange={(e) => patchProvider(p.key, { model: e.target.value })}
                  />
                </Field>
                {p.kind === "openai_compatible" && (
                  <Field label="URL base" hint="Ex.: http://localhost:11434/v1">
                    <input value={p.baseUrl} onChange={(e) => patchProvider(p.key, { baseUrl: e.target.value })} />
                  </Field>
                )}
                <SecretField
                  label="Chave de API"
                  value={p.apiKey}
                  writable={secretsWritable}
                  onChange={(edit) => patchProvider(p.key, { apiKey: { ...p.apiKey, edit } })}
                />
              </div>
            </li>
          ))}
        </ul>
      </Section>

      <Section
        title="Agentes"
        hint="Personas que uma conversa pode escolher (no desktop e nas delegações entre agentes)."
        action={
          <button
            type="button"
            className="link-button"
            onClick={() =>
              update((d) => ({
                ...d,
                agents: [...d.agents, keyed({ id: "", persona: "", providerId: "", canDelegateToAgents: false, canManageAgents: false, allowedTools: null })],
              }))
            }
          >
            + Agente
          </button>
        }
      >
        {draft.agents.length === 0 && <p className="skills-hint">Nenhum agente.</p>}
        <ul className="skills-list">
          {draft.agents.map((a) => (
            <li key={a.key} className="skills-item settings-card">
              <div className="settings-grid">
                <Field label="Nome">
                  <input value={a.id} onChange={(e) => patchAgent(a.key, { id: e.target.value })} />
                </Field>
                <Field label="Modelo padrão">
                  <select value={a.providerId} onChange={(e) => patchAgent(a.key, { providerId: e.target.value })}>
                    <option value="">O da conversa</option>
                    {providerIds.map((id) => (
                      <option key={id} value={id}>
                        {id}
                      </option>
                    ))}
                  </select>
                </Field>
                <Field label="Persona" wide>
                  <textarea rows={4} value={a.persona} onChange={(e) => patchAgent(a.key, { persona: e.target.value })} />
                </Field>
              </div>
              <div className="settings-checks">
                <label className="settings-check">
                  <input type="checkbox" checked={a.canDelegateToAgents} onChange={(e) => patchAgent(a.key, { canDelegateToAgents: e.target.checked })} />
                  Pode delegar a outros agentes
                </label>
                <label className="settings-check">
                  <input type="checkbox" checked={a.canManageAgents} onChange={(e) => patchAgent(a.key, { canManageAgents: e.target.checked })} />
                  Pode criar e editar agentes (sempre com a sua aprovação)
                </label>
                <label className="settings-check">
                  <input
                    type="checkbox"
                    checked={a.allowedTools !== null}
                    onChange={(e) => patchAgent(a.key, { allowedTools: e.target.checked ? [] : null })}
                  />
                  Limitar as ferramentas
                </label>
              </div>
              {a.allowedTools !== null && (
                <fieldset className="settings-tools">
                  <legend className="skills-hint">Só estas ferramentas:</legend>
                  {settings.toolNames.map((tool) => (
                    <label key={tool} className="settings-check">
                      <input
                        type="checkbox"
                        checked={a.allowedTools!.includes(tool)}
                        onChange={(e) =>
                          patchAgent(a.key, {
                            allowedTools: e.target.checked ? [...a.allowedTools!, tool] : a.allowedTools!.filter((t) => t !== tool),
                          })
                        }
                      />
                      <code>{tool}</code>
                    </label>
                  ))}
                </fieldset>
              )}
              <div className="skills-actions">
                <button type="button" className="link-button skills-danger" onClick={() => update((d) => ({ ...d, agents: d.agents.filter((x) => x.key !== a.key) }))}>
                  Remover agente
                </button>
              </div>
            </li>
          ))}
        </ul>
      </Section>

      <Section title="Outras chaves" hint="Cada uma liga um recurso, qualquer que seja o provedor ativo.">
        <div className="settings-grid">
          <SecretField
            label="Tavily (busca na web)"
            value={draft.tavilyKey}
            writable={secretsWritable}
            onChange={(edit) => update((d) => ({ ...d, tavilyKey: { ...d.tavilyKey, edit } }))}
          />
          <SecretField
            label="OpenAI para voz (Whisper)"
            value={draft.whisperKey}
            writable={secretsWritable}
            onChange={(edit) => update((d) => ({ ...d, whisperKey: { ...d.whisperKey, edit } }))}
          />
        </div>
      </Section>

      <Section title="Limites de gasto" hint="Valem para todos os canais desta máquina. A janela é móvel: “24 h” são as últimas 24 horas, não desde a meia-noite.">
        {settings.limitsDisabledByEnv && <p className="banner settings-note">WARDEN_SPEND_LIMITS=off está definido no ambiente do hub e desliga tudo o que for salvo aqui.</p>}
        <div className="settings-checks" role="radiogroup" aria-label="Limites">
          {(
            [
              ["default", "Os padrão (500 mil tokens por hora, 2 milhões por dia)"],
              ["custom", "Personalizados"],
              ["off", "Nenhum limite"],
            ] as [LimitsMode, string][]
          ).map(([mode, label]) => (
            <label key={mode} className="settings-check">
              <input type="radio" name="limits-mode" checked={draft.limitsMode === mode} onChange={() => setLimitsMode(mode)} />
              {label}
            </label>
          ))}
        </div>
        {draft.limitsMode === "custom" && (
          <>
            <ul className="skills-list">
              {draft.limits.map((l) => {
                const scope = SCOPES.find((s) => s.value === l.scope) ?? SCOPES[0];
                return (
                  <li key={l.key} className="skills-item settings-card">
                    <div className="settings-grid">
                      <Field label="Nome">
                        <input value={l.id} onChange={(e) => patchLimit(l.key, { id: e.target.value })} />
                      </Field>
                      <Field label="Vale para">
                        <select
                          value={l.scope}
                          onChange={(e) => patchLimit(l.key, { scope: e.target.value as LimitScope, target: e.target.value === "global" ? "" : l.target })}
                        >
                          {SCOPES.map((s) => (
                            <option key={s.value} value={s.value}>
                              {s.label}
                            </option>
                          ))}
                        </select>
                      </Field>
                      {l.scope !== "global" && (
                        <Field label="Quem">
                          <input value={l.target} placeholder={scope.target} onChange={(e) => patchLimit(l.key, { target: e.target.value })} />
                        </Field>
                      )}
                      <Field label="Janela (horas)">
                        <input type="number" min={1} value={l.windowHours} onChange={(e) => patchLimit(l.key, { windowHours: Number(e.target.value) })} />
                      </Field>
                      <Field label="Máx. tokens">
                        <input type="number" min={1} value={l.maxTokens ?? ""} onChange={(e) => patchLimit(l.key, { maxTokens: numberOrNull(e.target.value) })} />
                      </Field>
                      <Field label="Máx. US$" hint="Precisa do preço do modelo abaixo.">
                        <input
                          type="number"
                          min={0}
                          step="0.01"
                          value={l.maxCostUsd ?? ""}
                          onChange={(e) => patchLimit(l.key, { maxCostUsd: numberOrNull(e.target.value) })}
                        />
                      </Field>
                    </div>
                    <div className="skills-actions">
                      <button type="button" className="link-button skills-danger" onClick={() => update((d) => ({ ...d, limits: d.limits.filter((x) => x.key !== l.key) }))}>
                        Remover limite
                      </button>
                    </div>
                  </li>
                );
              })}
            </ul>
            <button
              type="button"
              className="link-button settings-add"
              onClick={() =>
                update((d) => ({
                  ...d,
                  limits: [
                    ...d.limits,
                    keyed({ id: "", scope: "global" as LimitScope, target: "", windowHours: 24, maxTokens: null, maxCostUsd: null, warnAt: null, extendStep: null }),
                  ],
                }))
              }
            >
              + Limite
            </button>
          </>
        )}
      </Section>

      <Section
        title="Preços"
        hint="Quanto cada modelo cobra por milhão de tokens, para os limites em dólar. O id tem de ser exatamente o do modelo."
        action={
          <button type="button" className="link-button" onClick={() => update((d) => ({ ...d, prices: [...d.prices, keyed({ model: "", inputPerMtok: 0, outputPerMtok: 0 })] }))}>
            + Preço
          </button>
        }
      >
        {draft.prices.length === 0 && <p className="skills-hint">Nenhum preço cadastrado.</p>}
        <ul className="skills-list">
          {draft.prices.map((p) => (
            <li key={p.key} className="skills-item settings-card settings-price">
              <Field label="Modelo">
                <input value={p.model} onChange={(e) => patchPrice(p.key, { model: e.target.value })} />
              </Field>
              <Field label="Entrada (US$/M)">
                <input type="number" min={0} step="0.01" value={p.inputPerMtok} onChange={(e) => patchPrice(p.key, { inputPerMtok: Number(e.target.value) })} />
              </Field>
              <Field label="Saída (US$/M)">
                <input type="number" min={0} step="0.01" value={p.outputPerMtok} onChange={(e) => patchPrice(p.key, { outputPerMtok: Number(e.target.value) })} />
              </Field>
              <button
                type="button"
                className="link-button skills-danger settings-price-remove"
                aria-label={`Remover o preço de ${p.model || "modelo sem nome"}`}
                onClick={() => update((d) => ({ ...d, prices: d.prices.filter((x) => x.key !== p.key) }))}
              >
                Remover
              </button>
            </li>
          ))}
        </ul>
      </Section>

      <div className="settings-footer">
        {saveError && (
          <p className="error-banner">
            {saveError.text}
            {saveError.conflict && (
              <>
                {" "}
                <button type="button" className="link-button" onClick={load}>
                  Recarregar (descarta o que você mudou aqui)
                </button>
              </>
            )}
          </p>
        )}
        {savedAt !== null && !dirty && <p className="settings-saved">✓ Salvo. O hub já está usando as novas configurações.</p>}
        {asking ? (
          <form
            className="settings-confirm"
            onSubmit={(e) => {
              e.preventDefault();
              void save();
            }}
          >
            <Field label="Chave de pareamento do hub" hint="A mesma do primeiro login. É pedida a cada vez que se salva.">
              <input type="password" autoComplete="current-password" autoFocus value={pairingKey} onChange={(e) => setPairingKey(e.target.value)} />
            </Field>
            {keyError && <p className="error-banner">{keyError}</p>}
            <div className="skills-actions">
              <button type="submit" className="primary-button" disabled={saving || pairingKey.trim() === "" || !conn}>
                {saving ? "Salvando e reiniciando…" : "Confirmar"}
              </button>
              <button
                type="button"
                className="link-button"
                disabled={saving}
                onClick={() => {
                  setAsking(false);
                  setPairingKey("");
                  setKeyError(null);
                }}
              >
                Cancelar
              </button>
            </div>
          </form>
        ) : (
          <div className="skills-actions">
            <button type="button" className="primary-button" disabled={!dirty || !conn} onClick={() => setAsking(true)}>
              Salvar
            </button>
            <button type="button" className="link-button" disabled={!dirty} onClick={() => setDraft(toDraft(settings))}>
              Descartar mudanças
            </button>
          </div>
        )}
      </div>
    </div>
  );
}
