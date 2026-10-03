import { useState } from "react";
import { UserError, type ServerConnection } from "../hub/connection";
import type { ProviderEdit } from "../hub/messages";
import { Field } from "./settingsParts";

// "Testar chave" (P10): asks the hub to check a provider's key without spending a conversation. The hub asks the
// provider for its model list, with the key saved for it (`keep`) or the one just typed (`set`), and answers a word and
// a sentence, never the key or what the provider said. It needs the pairing key like a save does, so the screen asks
// for it here, on the spot, and never keeps it.

/** What each answer is shown as: a mark, and whether it is plainly good, plainly wrong, or neither (the key may be fine). */
const MARKS: Record<string, { mark: string; tone: "good" | "bad" | "unsure" }> = {
  ok: { mark: "✓", tone: "good" },
  unverifiable: { mark: "⚠", tone: "unsure" },
  rate_limited: { mark: "⚠", tone: "unsure" },
  rejected: { mark: "✗", tone: "bad" },
  provider_down: { mark: "✗", tone: "bad" },
  unreachable: { mark: "✗", tone: "bad" },
  unsupported: { mark: "⚠", tone: "unsure" },
};

function message(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

export default function ProviderKeyTest({ conn, provider }: { conn: ServerConnection | null; provider: ProviderEdit }) {
  const [asking, setAsking] = useState(false);
  const [pairingKey, setPairingKey] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<{ ok: boolean; kind: string; message: string } | null>(null);

  async function run() {
    if (!conn) return;
    setBusy(true);
    setError(null);
    setResult(null);
    try {
      setResult(await conn.testProvider(pairingKey, provider));
      setAsking(false);
      setPairingKey("");
    } catch (err) {
      setError(err instanceof UserError && err.authRejected ? "Chave de pareamento errada." : message(err));
    } finally {
      setBusy(false);
    }
  }

  const shown = result ? (MARKS[result.kind] ?? MARKS.unreachable) : null;

  return (
    <div className="settings-field settings-field--wide">
      {asking ? (
        <form
          className="settings-confirm"
          onSubmit={(e) => {
            e.preventDefault();
            void run();
          }}
        >
          <Field label="Chave de pareamento do hub, para testar a chave" hint="O teste só pede a lista de modelos ao provedor: não gasta tokens nem conta nos limites.">
            <input type="password" autoComplete="current-password" autoFocus value={pairingKey} onChange={(e) => setPairingKey(e.target.value)} />
          </Field>
          {error && <p className="error-banner">{error}</p>}
          <div className="skills-actions">
            <button type="submit" className="primary-button" disabled={busy || pairingKey.trim() === "" || !conn}>
              {busy ? "Testando…" : "Testar"}
            </button>
            <button
              type="button"
              className="link-button"
              disabled={busy}
              onClick={() => {
                setAsking(false);
                setPairingKey("");
                setError(null);
              }}
            >
              Cancelar
            </button>
          </div>
        </form>
      ) : (
        <span className="skills-actions">
          <button
            type="button"
            className="link-button"
            disabled={!conn}
            onClick={() => {
              setAsking(true);
              setResult(null);
            }}
          >
            Testar chave
          </button>
        </span>
      )}
      {result && shown && (
        <p role="status" className={shown.tone === "bad" ? "error-banner" : shown.tone === "unsure" ? "banner settings-note" : "settings-saved"}>
          {shown.mark} {result.message}
        </p>
      )}
    </div>
  );
}
