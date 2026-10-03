import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

/** Mirrors `warden_server_protocol::BotsSettingsDto`. */
interface BotsSettings {
  learningEnabled: boolean;
  /** A provider or combo id; empty is the active model. */
  learningProvider: string;
  learningMaxPerDay: number;
  /** `telegram:<id>` / `whatsapp:<id>`. */
  learningBotChats: string[];
  telegramAllowedUsers: number[];
  whatsappAllowedChats: string[];
}

/** Mirrors `bot_cmds::BotsPayload` — never the token, only whether one is saved. */
interface BotsPayload {
  bots: BotsSettings;
  telegramToken: { set: boolean; hint: string | null };
  modelIds: string[];
}

/** The lists are edited as text, one entry per line. */
interface Draft {
  learningEnabled: boolean;
  learningProvider: string;
  maxPerDay: string;
  learningChats: string;
  telegramUsers: string;
  whatsappChats: string;
  /** `null` keeps the saved token, "" removes it, anything else replaces it. */
  telegramToken: string | null;
}

function toDraft(bots: BotsSettings): Draft {
  return {
    learningEnabled: bots.learningEnabled,
    learningProvider: bots.learningProvider,
    maxPerDay: String(bots.learningMaxPerDay),
    learningChats: bots.learningBotChats.join("\n"),
    telegramUsers: bots.telegramAllowedUsers.join("\n"),
    whatsappChats: bots.whatsappAllowedChats.join("\n"),
    telegramToken: null,
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
    learningEnabled: d.learningEnabled,
    learningProvider: d.learningProvider,
    learningMaxPerDay: Number(d.maxPerDay),
    learningBotChats: lines(d.learningChats),
    telegramAllowedUsers: lines(d.telegramUsers).map(Number),
    whatsappAllowedChats: lines(d.whatsappChats),
  };
}

/** What stops a save, in the words the screen shows; `null` when the draft is fine. */
function problem(d: Draft): string | null {
  const max = Number(d.maxPerDay);
  if (!Number.isInteger(max) || max < 1) return "Suggestions per day: a whole number, 1 or more.";
  const bad = lines(d.telegramUsers).find((l) => !/^[1-9][0-9]*$/.test(l));
  if (bad) return `"${bad}" is not a Telegram id: use the number (@userinfobot shows yours).`;
  const chat = lines(d.learningChats).find((l) => !/^(telegram|whatsapp):\S+$/.test(l));
  if (chat) return `"${chat}" is not a chat: write telegram:<id> or whatsapp:<id>.`;
  const wa = lines(d.whatsappChats).find((l) => /\s/.test(l));
  if (wa) return `"${wa}" has spaces: write just the number or the whole id.`;
  return null;
}

/**
 * P118 — learning and the Telegram/WhatsApp bots: who may talk to them, whether the assistant
 * suggests what to learn, and the Telegram token. Lives outside Settings' form: it saves on its own
 * (`bot_cmds.rs` rereads the file and changes only this slice). The bots reread the lists on the
 * fly; the token and `[learning]` are read when they start.
 */
function BotsSection() {
  const [loaded, setLoaded] = useState<BotsPayload | null>(null);
  const [draft, setDraft] = useState<Draft | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);

  useEffect(() => {
    invoke<BotsPayload>("get_bots_settings")
      .then((payload) => {
        setLoaded(payload);
        setDraft(toDraft(payload.bots));
      })
      .catch((err) => setError(String(err)));
  }, []);

  const dirty = useMemo(() => {
    if (!loaded || !draft) return false;
    return JSON.stringify(draft) !== JSON.stringify(toDraft(loaded.bots));
  }, [loaded, draft]);

  if (!loaded || !draft) {
    return (
      <section className="settings-section">
        <h3 className="settings-section-title">Learning and bots</h3>
        {error ? <p className="settings-error-banner">{error}</p> : <p className="settings-hint">Loading…</p>}
      </section>
    );
  }

  const blocked = problem(draft);
  const change = (patch: Partial<Draft>) => {
    setDraft({ ...draft, ...patch });
    setSaved(false);
  };

  async function save() {
    if (!draft) return;
    setSaving(true);
    setError(null);
    try {
      const payload = await invoke<BotsPayload>("save_bots_settings", { update: { bots: toBots(draft), telegramToken: draft.telegramToken } });
      setLoaded(payload);
      setDraft(toDraft(payload.bots));
      setSaved(true);
    } catch (err) {
      setError(String(err));
    } finally {
      setSaving(false);
    }
  }

  const token = loaded.telegramToken;
  const tokenStatus =
    draft.telegramToken === null ? (token.set ? (token.hint ? `Saved, ends in …${token.hint}` : "Saved") : "Not set") : draft.telegramToken === "" ? "Will be removed on save" : "New token (not saved yet)";

  return (
    <section className="settings-section">
      <div className="settings-section-header">
        <h3 className="settings-section-title">Learning and bots</h3>
      </div>
      <p className="settings-hint">
        Who may talk to the Telegram and WhatsApp bots, and whether the assistant suggests what to learn from conversations.
        An empty list means nobody. The lists apply as soon as you save; a new token or learning setting is read when the bots
        start.
      </p>

      {error && <p className="settings-error-banner">{error}</p>}
      {blocked && <p className="settings-error-banner">{blocked}</p>}

      <label className="settings-field settings-checkbox-field">
        <span className="settings-checkbox-row">
          <input type="checkbox" checked={draft.learningEnabled} onChange={(e) => change({ learningEnabled: e.currentTarget.checked })} />
          <span className="settings-label">Learn from conversations</span>
        </span>
        <span className="settings-hint">Every look is a short model call and counts against your spending limit. Suggestions wait for your approval.</span>
      </label>

      <label className="settings-field">
        <span className="settings-label">Learning model</span>
        <select className="settings-select" value={draft.learningProvider} onChange={(e) => change({ learningProvider: e.currentTarget.value })}>
          <option value="">The active model</option>
          {loaded.modelIds.map((id) => (
            <option key={id} value={id}>
              {id}
            </option>
          ))}
        </select>
        <span className="settings-hint">A cheap model does the job. Members can have their own in the Workspace's People section.</span>
      </label>

      <label className="settings-field">
        <span className="settings-label">Suggestions per day</span>
        <input className="settings-input" type="number" min={1} step={1} value={draft.maxPerDay} onChange={(e) => change({ maxPerDay: e.currentTarget.value })} />
        <span className="settings-hint">Each person's cap in 24 hours.</span>
      </label>

      <label className="settings-field">
        <span className="settings-label">Telegram bot token</span>
        <input
          className="settings-input"
          type="password"
          autoComplete="off"
          placeholder={tokenStatus}
          value={draft.telegramToken ?? ""}
          onChange={(e) => change({ telegramToken: e.currentTarget.value })}
        />
        <span className="settings-hint">
          {tokenStatus}.{" "}
          {token.set && draft.telegramToken === null && (
            <button type="button" className="settings-browse-btn" onClick={() => change({ telegramToken: "" })}>
              Remove
            </button>
          )}
          {draft.telegramToken !== null && (
            <button type="button" className="settings-browse-btn" onClick={() => change({ telegramToken: null })}>
              Undo
            </button>
          )}
        </span>
      </label>

      <label className="settings-field">
        <span className="settings-label">Who may talk to the Telegram bot</span>
        <textarea className="settings-input settings-textarea" rows={3} value={draft.telegramUsers} onChange={(e) => change({ telegramUsers: e.currentTarget.value })} />
        <span className="settings-hint">Numeric user ids, one per line. Private chats only. Empty: the bot answers nobody.</span>
      </label>

      <label className="settings-field">
        <span className="settings-label">Who may talk to the WhatsApp bot</span>
        <textarea className="settings-input settings-textarea" rows={3} value={draft.whatsappChats} onChange={(e) => change({ whatsappChats: e.currentTarget.value })} />
        <span className="settings-hint">A number (5511999999999) or the whole id, one per line. Private chats only. Empty: nobody.</span>
      </label>

      <label className="settings-field">
        <span className="settings-label">Bot chats the assistant may learn from</span>
        <textarea className="settings-input settings-textarea" rows={3} value={draft.learningChats} onChange={(e) => change({ learningChats: e.currentTarget.value })} />
        <span className="settings-hint">telegram:&lt;id&gt; or whatsapp:&lt;id&gt;, one per line. Only counts with learning on. Empty: the bots don't learn.</span>
      </label>

      <div className="api-key-actions">
        <button type="button" className="settings-save-btn" disabled={!dirty || blocked !== null || saving} onClick={() => void save()}>
          {saving ? "Saving…" : "Save learning and bots"}
        </button>
        {dirty && (
          <button type="button" className="settings-browse-btn" onClick={() => setDraft(toDraft(loaded.bots))}>
            Discard
          </button>
        )}
        {saved && !dirty && <span className="settings-hint">Saved.</span>}
      </div>
    </section>
  );
}

export default BotsSection;
