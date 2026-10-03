import { useCallback, useEffect, useMemo, useState } from "react";
import { SettingsError, UserError, type LoadedSettings, type ServerConnection } from "../hub/connection";
import ApiKeysSection from "./ApiKeysSection";
import AdvancedSection from "./AdvancedSection";
import MachineSection from "./MachineSection";
import { advancedError, machineError, machineSummary, toAdvanced, toAdvancedDraft, toMachineDraft, toMachineEdit, type AdvancedDraft, type MachineDraft } from "./machineDraft";
import { Field, KEEP, keyed, SecretField, Section, strip, type Keyed, type SecretDraft } from "./settingsParts";
import type {
  AdvancedSettings,
  AgentSettings,
  BotPairing,
  BotsSettings,
  Combo,
  HubSettings,
  HubSettingsUpdate,
  LimitScope,
  LimitSettings,
  MachineEdit,
  PriceSettings,
  ProviderKind,
  UserInfo,
} from "../hub/messages";

// The hub's settings (P78): providers, agents, the Tavily/Whisper keys, spending limits, prices and
// the git remote the vault syncs to (P61; the Sync tab runs it).
// Everything is edited as one draft and saved at once; the hub asks for the pairing key again on
// every save, checks the file wasn't changed meanwhile, and restarts its orchestrator with the new
// settings (or keeps the old ones if it can't start with them). API keys never come back from the
// hub: a key field shows whether one is saved and can only be replaced or removed.

const KINDS: { value: ProviderKind; label: string }[] = [
  { value: "gemini", label: "Gemini" },
  { value: "openai", label: "OpenAI" },
  { value: "anthropic", label: "Anthropic" },
  { value: "openai_compatible", label: "Compatível com OpenAI (Ollama, OpenRouter…)" },
  { value: "node", label: "Modelo de um nó (outra máquina)" },
];

const SCOPES: { value: LimitScope; label: string; target: string }[] = [
  { value: "global", label: "Tudo", target: "" },
  { value: "agent", label: "Um agente", target: "id do agente" },
  { value: "channel", label: "Um canal", target: "server, desktop, telegram…" },
  { value: "user", label: "Um usuário", target: "canal:id, ex. telegram:12345" },
  { value: "person", label: "Uma pessoa", target: "usuário, ex. ana (em todos os canais)" },
];

type ProviderDraft = Keyed<{ originalId?: string; id: string; kind: ProviderKind; baseUrl: string; model: string; apiKey: SecretDraft; node: string }>;

type LimitsMode = "default" | "custom" | "off";

interface Draft {
  providers: ProviderDraft[];
  activeProvider: string;
  combos: Keyed<Combo>[];
  agents: Keyed<AgentSettings>[];
  tavilyKey: SecretDraft;
  whisperKey: SecretDraft;
  limitsMode: LimitsMode;
  limits: Keyed<LimitSettings>[];
  prices: Keyed<PriceSettings>[];
  gitRemoteUrl: string;
  /** As loaded: an untouched git section isn't sent, so a remote set by hand in the file (a local
   * path, which the web can't save) never blocks saving the rest. */
  gitRemoteLoaded: string;
  gitToken: SecretDraft;
  /** P118 — the lists are edited as text, one entry per line. */
  botsLearningEnabled: boolean;
  botsLearningProvider: string;
  botsMaxPerDay: string;
  botsLearningChats: string;
  botsTelegramUsers: string;
  botsWhatsappChats: string;
  botsTelegramPairing: boolean;
  botsWhatsappPairing: boolean;
  /** P119 — the Telegram bot's token, a secret like the keys above. */
  telegramToken: SecretDraft;
  /** P119 — delegation ceilings and TruthID. Sent only when they changed from `advancedBase` (as loaded). */
  advanced: AdvancedDraft;
  advancedBase: AdvancedSettings;
  /** P119 — what reaches the hub's machine. Sent only when it changed from `machineBase`, so a hub that
   * doesn't allow editing it (or a connection that isn't encrypted) never receives it by accident. */
  machine: MachineDraft;
  machineBase: MachineEdit;
}

function toDraft(s: HubSettings): Draft {
  return {
    providers: s.providers.map((p) => keyed({ originalId: p.id, id: p.id, kind: p.kind, baseUrl: p.baseUrl, model: p.model, apiKey: { saved: p.apiKey, edit: KEEP }, node: p.node ?? "" })),
    activeProvider: s.activeProvider,
    combos: (s.combos ?? []).map((c) => keyed({ ...c })),
    agents: s.agents.map((a) => keyed({ ...a, originalId: a.id })),
    tavilyKey: { saved: s.tavilyKey, edit: KEEP },
    whisperKey: { saved: s.whisperKey, edit: KEEP },
    limitsMode: s.limits === null ? "default" : s.limits.length === 0 ? "off" : "custom",
    limits: (s.limits ?? []).map(keyed),
    prices: s.prices.map(keyed),
    gitRemoteUrl: s.gitSync.remoteUrl,
    gitRemoteLoaded: s.gitSync.remoteUrl,
    gitToken: { saved: s.gitSync.token, edit: KEEP },
    botsLearningEnabled: s.bots.learningEnabled,
    botsLearningProvider: s.bots.learningProvider,
    botsMaxPerDay: String(s.bots.learningMaxPerDay),
    botsLearningChats: s.bots.learningBotChats.join("\n"),
    botsTelegramUsers: s.bots.telegramAllowedUsers.join("\n"),
    botsWhatsappChats: s.bots.whatsappAllowedChats.join("\n"),
    botsTelegramPairing: s.bots.telegramPairing,
    botsWhatsappPairing: s.bots.whatsappPairing,
    telegramToken: { saved: s.telegramToken, edit: KEEP },
    advanced: toAdvancedDraft(s.advanced),
    advancedBase: s.advanced,
    machine: toMachineDraft(s.machine),
    machineBase: toMachineEdit(toMachineDraft(s.machine)),
  };
}

/** One entry per line (a comma works too), blanks dropped. */
function lines(text: string): string[] {
  return text
    .split(/[\n,]/)
    .map((l) => l.trim())
    .filter((l) => l !== "");
}

function toBots(d: Draft): BotsSettings {
  return {
    learningEnabled: d.botsLearningEnabled,
    learningProvider: d.botsLearningProvider,
    learningMaxPerDay: Number(d.botsMaxPerDay),
    learningBotChats: lines(d.botsLearningChats),
    telegramAllowedUsers: lines(d.botsTelegramUsers).map(Number),
    whatsappAllowedChats: lines(d.botsWhatsappChats),
    telegramPairing: d.botsTelegramPairing,
    whatsappPairing: d.botsWhatsappPairing,
  };
}

/** What stops the bots block from being saved, in the words the screen shows; `null` when it's fine. */
function botsError(d: Draft): string | null {
  const max = Number(d.botsMaxPerDay);
  if (!Number.isInteger(max) || max < 1) return "Sugestões por dia: um número inteiro, de 1 para cima.";
  const bad = lines(d.botsTelegramUsers).find((l) => !/^[1-9][0-9]*$/.test(l));
  if (bad) return `“${bad}” não é um id do Telegram: use o número (o @userinfobot mostra o seu).`;
  const chat = lines(d.botsLearningChats).find((l) => !/^(telegram|whatsapp):\S+$/.test(l));
  if (chat) return `“${chat}” não é um chat: escreva telegram:<id> ou whatsapp:<id>.`;
  const wa = lines(d.botsWhatsappChats).find((l) => /\s/.test(l));
  if (wa) return `“${wa}” tem espaços: escreva só o número ou o id inteiro.`;
  return null;
}

function toUpdate(d: Draft): HubSettingsUpdate {
  return {
    providers: d.providers.map((p) => ({ originalId: p.originalId, id: p.id, kind: p.kind, baseUrl: p.baseUrl, model: p.model, apiKey: p.apiKey.edit, node: p.kind === "node" ? p.node : "" })),
    activeProvider: d.activeProvider,
    combos: d.combos.map(strip),
    agents: d.agents.map(strip),
    tavilyKey: d.tavilyKey.edit,
    whisperKey: d.whisperKey.edit,
    limits: d.limitsMode === "default" ? null : d.limitsMode === "off" ? [] : d.limits.map(strip),
    prices: d.prices.map(strip),
    ...((d.gitRemoteUrl !== d.gitRemoteLoaded || d.gitToken.edit.action !== "keep") && {
      gitSync: { remoteUrl: d.gitRemoteUrl, token: d.gitRemoteUrl.trim() === "" ? { action: "clear" } : d.gitToken.edit },
    }),
    bots: toBots(d),
    telegramToken: d.telegramToken.edit,
    ...(machineChanged(d) && { machine: toMachineEdit(d.machine) }),
    ...(advancedChanged(d) && { advanced: toAdvanced(d.advanced) }),
  };
}

function machineChanged(d: Draft): boolean {
  return JSON.stringify(toMachineEdit(d.machine)) !== JSON.stringify(d.machineBase);
}

function advancedChanged(d: Draft): boolean {
  return JSON.stringify(toAdvanced(d.advanced)) !== JSON.stringify(d.advancedBase);
}

/** `list` with the item at `index` moved one place up (`-1`) or down (`1`). */
function moved(list: string[], index: number, delta: number): string[] {
  const next = [...list];
  const [item] = next.splice(index, 1);
  next.splice(index + delta, 0, item);
  return next;
}

function message(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

function numberOrNull(value: string): number | null {
  return value.trim() === "" ? null : Number(value);
}

/** P117 — people who wrote to a bot and were given a code. The owner approves (they join the bot's list) or
 * denies each one, with the pairing key. Not part of the draft: it acts on its own, so it's off while the
 * form has unsaved edits (an approval reloads the settings, and the lists in them change). */
function BotPairings({ conn, disabled, onResolved }: { conn: ServerConnection | null; disabled: boolean; onResolved: () => void }) {
  const [pairings, setPairings] = useState<BotPairing[]>([]);
  const [ask, setAsk] = useState<{ code: string; approve: boolean } | null>(null);
  const [key, setKey] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(() => {
    if (!conn) return;
    conn.listBotPairings().then(setPairings, () => setPairings([]));
  }, [conn]);
  useEffect(refresh, [refresh]);

  async function resolve() {
    if (!conn || !ask) return;
    setBusy(true);
    setError(null);
    try {
      setPairings(await conn.resolveBotPairing(key, ask.code, ask.approve));
      const approved = ask.approve;
      setAsk(null);
      setKey("");
      if (approved) onResolved();
    } catch (err) {
      setError(err instanceof UserError && err.authRejected ? "Chave de pareamento errada." : message(err));
    } finally {
      setBusy(false);
    }
  }

  const minutes = (p: BotPairing) => Math.max(0, Math.ceil((p.expiresAt * 1000 - Date.now()) / 60000));

  return (
    <div className="settings-field">
      <strong>Esperando a sua aprovação</strong>
      {pairings.length === 0 ? (
        <p className="skills-hint">Ninguém esperando. Com o pedido de acesso ligado acima, quem escrever para um bot aparece aqui.</p>
      ) : (
        <ul className="skills-list">
          {pairings.map((p) => (
            <li key={`${p.channel}:${p.sender}`} className="skills-item settings-card">
              <p>
                <code>{p.code}</code> {p.channel === "telegram" ? "Telegram" : "WhatsApp"} {p.sender}
                {p.label && ` (${p.label})`} — expira em {minutes(p)} min
              </p>
              <div className="skills-actions">
                <button type="button" className="primary-button" disabled={disabled || !conn} onClick={() => setAsk({ code: p.code, approve: true })}>
                  Aprovar
                </button>
                <button type="button" className="link-button skills-danger" disabled={disabled || !conn} onClick={() => setAsk({ code: p.code, approve: false })}>
                  Recusar
                </button>
              </div>
            </li>
          ))}
        </ul>
      )}
      {disabled && pairings.length > 0 && <p className="skills-hint">Salve ou descarte as mudanças acima antes: aprovar atualiza as listas.</p>}
      {ask && (
        <form
          className="settings-confirm"
          onSubmit={(e) => {
            e.preventDefault();
            void resolve();
          }}
        >
          <Field label={`Chave de pareamento do hub, para ${ask.approve ? "aprovar" : "recusar"} ${ask.code}`}>
            <input type="password" autoComplete="current-password" autoFocus value={key} onChange={(e) => setKey(e.target.value)} />
          </Field>
          {error && <p className="error-banner">{error}</p>}
          <div className="skills-actions">
            <button type="submit" className="primary-button" disabled={busy || key.trim() === ""}>
              Confirmar
            </button>
            <button
              type="button"
              className="link-button"
              disabled={busy}
              onClick={() => {
                setAsk(null);
                setKey("");
                setError(null);
              }}
            >
              Cancelar
            </button>
          </div>
        </form>
      )}
      <button type="button" className="link-button" onClick={refresh}>
        Atualizar a lista
      </button>
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
  /** P119 — a save that changes what reaches the hub's machine is read back to the person first. */
  const [confirmingMachine, setConfirmingMachine] = useState(false);
  const [machineAck, setMachineAck] = useState(false);
  /** P84 — the workspace's members, for sharing agents with them. */
  const [people, setPeople] = useState<UserInfo[]>([]);

  const load = useCallback(() => {
    if (!conn) return;
    conn
      .listUsers()
      .then(({ users }) => setPeople(users))
      .catch(() => setPeople([]));
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
  const botsProblem = botsError(draft);
  // The machine and advanced blocks are only questioned when they changed: a file edited by hand into
  // something the screen wouldn't write must not stop the rest of it from saving.
  const machineIsChanged = machineChanged(draft);
  const machineProblem = machineIsChanged ? machineError(draft.machine, draft.machineBase) : null;
  const advancedProblem = advancedChanged(draft) ? advancedError(draft.advanced, draft.advancedBase) : null;
  const agentIds = draft.agents.map((a) => a.id.trim()).filter((id) => id !== "");
  const modelIds = [...draft.providers.map((p) => p.id), ...draft.combos.map((c) => c.id)].filter((id) => id.trim() !== "");
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
          combos: d.combos.map((c) => ({ ...c, providers: c.providers.map((id) => (id === before.id ? patch.id! : id)) })),
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
        // A combo loses it too (the hub refuses an empty combo, so the screen says so before saving).
        combos: d.combos.map((c) => ({ ...c, providers: c.providers.filter((id) => id !== removed?.id) })),
      };
    });
  }

  function patchCombo(key: number, patch: Partial<Combo>) {
    update((d) => {
      const before = d.combos.find((c) => c.key === key);
      const combos = d.combos.map((c) => (c.key === key ? { ...c, ...patch } : c));
      // A renamed combo stays the active model and every agent's default, like a provider.
      if (before && patch.id !== undefined && patch.id !== before.id) {
        return {
          ...d,
          combos,
          activeProvider: d.activeProvider === before.id ? patch.id : d.activeProvider,
          agents: d.agents.map((a) => (a.providerId === before.id ? { ...a, providerId: patch.id! } : a)),
        };
      }
      return { ...d, combos };
    });
  }

  function removeCombo(key: number) {
    update((d) => {
      const removed = d.combos.find((c) => c.key === key);
      return {
        ...d,
        combos: d.combos.filter((c) => c.key !== key),
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
      setConfirmingMachine(false);
      setMachineAck(false);
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
  const comboIds = draft.combos.map((c) => c.id.trim()).filter((id) => id !== "");

  return (
    <div className="usage-view settings-view">
      <div className="skills-toolbar">
        <span className="skills-hint">
          A configuração do hub. O que alcança a máquina dele (shell, MCP, SSH, pastas) só muda daqui se o hub foi iniciado com --allow-machine-settings e a conexão é cifrada; senão, no desktop ou no config.toml.
        </span>
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
                providers: [...d.providers, keyed({ id: "", kind: "gemini" as ProviderKind, baseUrl: "", model: "", apiKey: { saved: { set: false }, edit: KEEP }, node: "" })],
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
                <Field label={p.kind === "node" ? "Provedor no nó" : "Modelo"} hint={p.kind === "node" ? "O id dele no config.toml do nó, o mesmo do --model" : undefined}>
                  <input
                    value={p.model}
                    placeholder={p.kind === "node" ? "ollama" : (settings.defaultModels[p.kind] ?? "obrigatório para este tipo")}
                    onChange={(e) => patchProvider(p.key, { model: e.target.value })}
                  />
                </Field>
                {p.kind === "node" && (
                  <Field label="Id do nó" hint="Só responde com o nó online, aprovado e liberado (aba Aparelhos). Num combo, cai no próximo quando ele está fora.">
                    <input value={p.node} placeholder="node-casa-pc-1a2b3c4d" onChange={(e) => patchProvider(p.key, { node: e.target.value })} />
                  </Field>
                )}
                {p.kind === "openai_compatible" && (
                  <Field label="URL base" hint="Ex.: http://localhost:11434/v1">
                    <input value={p.baseUrl} onChange={(e) => patchProvider(p.key, { baseUrl: e.target.value })} />
                  </Field>
                )}
                {p.kind !== "node" && (
                  <SecretField
                    label="Chave de API"
                    value={p.apiKey}
                    writable={secretsWritable}
                    onChange={(edit) => patchProvider(p.key, { apiKey: { ...p.apiKey, edit } })}
                  />
                )}
              </div>
            </li>
          ))}
        </ul>
      </Section>

      <Section
        title="Combos"
        hint="Um combo se escolhe como um provedor (modelo ativo, padrão de um agente) e tenta os provedores dele na ordem: se um cair (ocupado, limite de uso ou sem conexão), o próximo responde, e o chat avisa acima da resposta. Chave recusada ou pedido inválido nunca trocam."
        action={
          <button type="button" className="link-button" onClick={() => update((d) => ({ ...d, combos: [...d.combos, keyed({ id: "", providers: [] })] }))}>
            + Combo
          </button>
        }
      >
        {draft.combos.length === 0 && <p className="skills-hint">Nenhum combo.</p>}
        <ul className="skills-list">
          {draft.combos.map((c) => (
            <li key={c.key} className="skills-item settings-card">
              <div className="skills-item-header">
                <label className="settings-radio">
                  <input
                    type="radio"
                    name="active-provider"
                    checked={c.id.trim() !== "" && draft.activeProvider === c.id}
                    disabled={c.id.trim() === ""}
                    onChange={() => update((d) => ({ ...d, activeProvider: c.id }))}
                  />
                  {draft.activeProvider === c.id && c.id.trim() !== "" ? "Ativo" : "Usar este"}
                </label>
                <button type="button" className="link-button skills-danger" onClick={() => removeCombo(c.key)}>
                  Remover
                </button>
              </div>
              <Field label="Nome">
                <input value={c.id} onChange={(e) => patchCombo(c.key, { id: e.target.value })} />
              </Field>
              {c.providers.length === 0 && <p className="error-banner">Um combo precisa de pelo menos um provedor.</p>}
              {c.providers.length > 0 && (
                <ol className="fallback-list">
                  {c.providers.map((id, index) => (
                    <li key={id} className="fallback-row">
                      <span className="fallback-name">{id}</span>
                      <button type="button" className="link-button" disabled={index === 0} onClick={() => patchCombo(c.key, { providers: moved(c.providers, index, -1) })}>
                        Subir
                      </button>
                      <button
                        type="button"
                        className="link-button"
                        disabled={index === c.providers.length - 1}
                        onClick={() => patchCombo(c.key, { providers: moved(c.providers, index, 1) })}
                      >
                        Descer
                      </button>
                      <button type="button" className="link-button skills-danger" onClick={() => patchCombo(c.key, { providers: c.providers.filter((v) => v !== id) })}>
                        Remover
                      </button>
                    </li>
                  ))}
                </ol>
              )}
              {providerIds.some((id) => !c.providers.includes(id)) && (
                <Field label="Adicionar provedor">
                  <select
                    value=""
                    onChange={(e) => {
                      const id = e.target.value;
                      if (id) patchCombo(c.key, { providers: [...c.providers, id] });
                    }}
                  >
                    <option value="">Escolha um provedor…</option>
                    {providerIds
                      .filter((id) => !c.providers.includes(id))
                      .map((id) => (
                        <option key={id} value={id}>
                          {id}
                        </option>
                      ))}
                  </select>
                </Field>
              )}
            </li>
          ))}
        </ul>
      </Section>

      <Section
        title="Agentes"
        hint="Personas que uma conversa pode escolher (aqui no chat, no desktop e nas delegações entre agentes)."
        action={
          <button
            type="button"
            className="link-button"
            onClick={() =>
              update((d) => ({
                ...d,
                agents: [
                  ...d.agents,
                  keyed({ id: "", persona: "", providerId: "", canDelegateToAgents: false, canManageAgents: false, canMessageAgents: false, canManageTasks: false, allowedTools: null }),
                ],
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
                    {comboIds.map((id) => (
                      <option key={id} value={id}>
                        {id} (combo)
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
                  <input type="checkbox" checked={a.canMessageAgents} onChange={(e) => patchAgent(a.key, { canMessageAgents: e.target.checked })} />
                  Pode deixar recados para outros agentes (numa conversa que você vê)
                </label>
                <label className="settings-check">
                  <input type="checkbox" checked={a.canManageTasks} onChange={(e) => patchAgent(a.key, { canManageTasks: e.target.checked })} />
                  Pode criar e editar tarefas agendadas (sempre com a sua aprovação)
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
              {people.length > 0 && (
                <fieldset className="settings-tools">
                  <legend className="skills-hint">Compartilhar com (a pessoa usa o agente com a memória e as ferramentas dela):</legend>
                  <label className="settings-check">
                    <input
                      type="checkbox"
                      checked={(a.sharedWith ?? []).includes("*")}
                      onChange={(e) => patchAgent(a.key, { sharedWith: e.target.checked ? ["*"] : [] })}
                    />
                    Todas as pessoas
                  </label>
                  {!(a.sharedWith ?? []).includes("*") &&
                    people.map((p) => (
                      <label key={p.id} className="settings-check">
                        <input
                          type="checkbox"
                          checked={(a.sharedWith ?? []).includes(p.id)}
                          onChange={(e) =>
                            patchAgent(a.key, {
                              sharedWith: e.target.checked ? [...(a.sharedWith ?? []), p.id] : (a.sharedWith ?? []).filter((s) => s !== p.id),
                            })
                          }
                        />
                        {p.name} <code>{p.id}</code>
                      </label>
                    ))}
                </fieldset>
              )}
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

      <Section
        title="Sincronização (git)"
        hint="Um repositório seu (Gitea, GitHub…) por onde este hub troca o vault e as configurações com os outros aparelhos, a cada 5 minutos. Tudo vai cifrado. Deixe a URL vazia para não usar git. O andamento fica na aba Sync."
      >
        <div className="settings-grid">
          <Field label="URL do repositório" hint="Só https://.">
            <input
              value={draft.gitRemoteUrl}
              placeholder="https://git.exemplo.com/voce/vault.git"
              onChange={(e) => {
                const gitRemoteUrl = e.target.value;
                update((d) => ({ ...d, gitRemoteUrl }));
              }}
            />
          </Field>
          <SecretField
            label="Token de acesso"
            value={draft.gitToken}
            writable={secretsWritable}
            onChange={(edit) => update((d) => ({ ...d, gitToken: { ...d.gitToken, edit } }))}
          />
        </div>
      </Section>

      <Section
        title="Aprendizado e bots"
        hint="Quem pode falar com o Telegram e o WhatsApp, e se o assistente sugere o que aprender das conversas. Uma lista vazia significa ninguém."
      >
        {botsProblem && <p className="error-banner">{botsProblem}</p>}
        <div className="settings-checks">
          <label className="settings-check">
            <input type="checkbox" checked={draft.botsLearningEnabled} onChange={(e) => update((d) => ({ ...d, botsLearningEnabled: e.target.checked }))} />
            Aprender com as conversas (cada olhada é uma chamada curta ao modelo, e gasta do seu limite)
          </label>
        </div>
        <div className="settings-checks">
          <label className="settings-check">
            <input type="checkbox" checked={draft.botsTelegramPairing} onChange={(e) => update((d) => ({ ...d, botsTelegramPairing: e.target.checked }))} />
            Deixar desconhecidos pedirem acesso no Telegram (recebem um código de 1 hora para você aprovar; desligado, não recebem resposta)
          </label>
          <label className="settings-check">
            <input type="checkbox" checked={draft.botsWhatsappPairing} onChange={(e) => update((d) => ({ ...d, botsWhatsappPairing: e.target.checked }))} />
            O mesmo no WhatsApp (só conversa privada)
          </label>
        </div>
        <BotPairings conn={conn} disabled={dirty} onResolved={load} />
        <div className="settings-grid">
          <SecretField
            label="Token do bot do Telegram (do @BotFather)"
            placeholder="Cole o token"
            value={draft.telegramToken}
            writable={secretsWritable}
            onChange={(edit) => update((d) => ({ ...d, telegramToken: { ...d.telegramToken, edit } }))}
          />
          <p className="field-hint settings-field--wide">O bot do Telegram lê o token quando inicia: depois de trocar, reinicie o bot.</p>
          <Field label="Modelo do aprendizado" hint="Um modelo barato resolve. Vazio usa o modelo ativo. Cada membro pode ter o seu na aba Pessoas.">
            <select value={draft.botsLearningProvider} onChange={(e) => update((d) => ({ ...d, botsLearningProvider: e.target.value }))}>
              <option value="">O modelo ativo</option>
              {modelIds.map((id) => (
                <option key={id} value={id}>
                  {id}
                </option>
              ))}
            </select>
          </Field>
          <Field label="Sugestões por dia" hint="O teto de cada pessoa em 24 horas.">
            <input type="number" min={1} step={1} value={draft.botsMaxPerDay} onChange={(e) => update((d) => ({ ...d, botsMaxPerDay: e.target.value }))} />
          </Field>
          <Field label="Quem pode falar com o bot do Telegram" hint="Ids numéricos, um por linha. Só conversa privada. Vazio: o bot não responde a ninguém." wide>
            <textarea rows={3} value={draft.botsTelegramUsers} onChange={(e) => update((d) => ({ ...d, botsTelegramUsers: e.target.value }))} />
          </Field>
          <Field label="Quem pode falar com o bot do WhatsApp" hint="Número (5511999999999) ou id inteiro, um por linha. Só conversa privada. Vazio: ninguém." wide>
            <textarea rows={3} value={draft.botsWhatsappChats} onChange={(e) => update((d) => ({ ...d, botsWhatsappChats: e.target.value }))} />
          </Field>
          <Field label="Conversas dos bots de que o assistente pode aprender" hint="telegram:<id> ou whatsapp:<id>, um por linha. Só vale com o aprendizado ligado. Vazio: os bots não aprendem." wide>
            <textarea rows={3} value={draft.botsLearningChats} onChange={(e) => update((d) => ({ ...d, botsLearningChats: e.target.value }))} />
          </Field>
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

      <AdvancedSection draft={draft.advanced} error={advancedProblem} onChange={(change) => update((d) => ({ ...d, advanced: { ...d.advanced, ...change } }))} />

      <MachineSection
        draft={draft.machine}
        settings={settings.machine}
        agentIds={agentIds}
        secretsWritable={secretsWritable}
        error={machineProblem}
        onChange={(change) => update((d) => ({ ...d, machine: change(d.machine) }))}
      />

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
        {confirmingMachine ? (
          <div className="settings-confirm settings-confirm--danger" role="alertdialog" aria-labelledby="machine-confirm-title">
            <h3 id="machine-confirm-title" className="settings-subheading">
              Esta mudança altera o que o hub executa na máquina dele
            </h3>
            <ul className="settings-changes">
              {machineSummary(draft.machineBase, toMachineEdit(draft.machine)).map((line) => (
                <li key={line}>{line}</li>
              ))}
            </ul>
            <p className="field-hint">Valores novos de variáveis de ambiente e cabeçalhos seguem para o hub, mas não são mostrados aqui. O hub registra esta mudança no log dele.</p>
            <label className="settings-check">
              <input type="checkbox" checked={machineAck} onChange={(e) => setMachineAck(e.target.checked)} />
              Entendo que isso muda o que o hub pode executar na máquina dele
            </label>
            <div className="skills-actions">
              <button
                type="button"
                className="primary-button"
                disabled={!machineAck}
                onClick={() => {
                  setConfirmingMachine(false);
                  setAsking(true);
                }}
              >
                Continuar
              </button>
              <button
                type="button"
                className="link-button"
                onClick={() => {
                  setConfirmingMachine(false);
                  setMachineAck(false);
                }}
              >
                Cancelar
              </button>
            </div>
          </div>
        ) : asking ? (
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
            <button
              type="button"
              className="primary-button"
              disabled={!dirty || !conn || botsProblem !== null || machineProblem !== null || advancedProblem !== null}
              onClick={() => {
                // What reaches the machine is read back first; the pairing key comes after.
                if (machineIsChanged) setConfirmingMachine(true);
                else setAsking(true);
              }}
            >
              Salvar
            </button>
            <button type="button" className="link-button" disabled={!dirty} onClick={() => setDraft(toDraft(settings))}>
              Descartar mudanças
            </button>
          </div>
        )}
      </div>

      {/* P12 — saved on its own, not with the form above. */}
      <ApiKeysSection conn={conn} />
    </div>
  );
}
