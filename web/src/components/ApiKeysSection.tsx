import { useCallback, useEffect, useState } from "react";
import { ApiKeyError, type ServerConnection } from "../hub/connection";
import type { ApiKey } from "../hub/messages";

// The Warden API (P12): this hub's agent behind the OpenAI chat-completions format, on the same
// address as this page, for scripts, n8n or any client that talks to OpenAI. The same list,
// create and revoke as the desktop's Settings and `warden-server api-keys`. Creating or revoking
// asks for the pairing key each time; a new key is shown once, here, and never again.

const dateFormatter = new Intl.DateTimeFormat("pt-BR", { dateStyle: "short", timeStyle: "short" });

function message(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

type Asking = { kind: "create"; name: string } | { kind: "revoke"; key: ApiKey };

export default function ApiKeysSection({ conn }: { conn: ServerConnection | null }) {
  const [keys, setKeys] = useState<ApiKey[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [name, setName] = useState("");
  const [asking, setAsking] = useState<Asking | null>(null);
  const [pairingKey, setPairingKey] = useState("");
  const [keyError, setKeyError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [created, setCreated] = useState<{ name: string; key: string } | null>(null);
  const [copied, setCopied] = useState(false);

  const baseUrl = `${window.location.origin}/v1`;

  const load = useCallback(async () => {
    if (!conn) return;
    setError(null);
    try {
      setKeys(await conn.listApiKeys());
    } catch (err) {
      setError(message(err));
    }
  }, [conn]);

  useEffect(() => {
    void load();
  }, [load]);

  function cancel() {
    setAsking(null);
    setPairingKey("");
    setKeyError(null);
  }

  async function confirm() {
    if (!conn || !asking) return;
    setBusy(true);
    setKeyError(null);
    try {
      if (asking.kind === "create") {
        const result = await conn.createApiKey(pairingKey, asking.name);
        setKeys(result.keys);
        setCreated({ name: asking.name.trim(), key: result.key });
        setCopied(false);
        setName("");
      } else {
        setKeys(await conn.revokeApiKey(pairingKey, asking.key.id));
      }
      cancel();
    } catch (err) {
      if (err instanceof ApiKeyError && err.authRejected) {
        setKeyError("Chave de pareamento errada.");
      } else {
        cancel();
        setError(message(err));
      }
    } finally {
      setBusy(false);
    }
  }

  async function copy(text: string) {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
    } catch {
      setCopied(false);
    }
  }

  const keyPrompt = asking && (
    <form
      className="settings-confirm devices-confirm"
      onSubmit={(e) => {
        e.preventDefault();
        void confirm();
      }}
    >
      <label className="settings-field">
        Chave de pareamento do hub
        <input type="password" autoComplete="current-password" autoFocus value={pairingKey} onChange={(e) => setPairingKey(e.target.value)} />
        <span className="field-hint">A mesma do primeiro login. É pedida a cada mudança.</span>
      </label>
      {keyError && <p className="error-banner">{keyError}</p>}
      <div className="skills-actions">
        <button type="submit" className="primary-button" disabled={busy || pairingKey.trim() === "" || !conn}>
          {busy ? "Aguarde…" : asking.kind === "create" ? "Criar a chave" : "Revogar"}
        </button>
        <button type="button" className="link-button" disabled={busy} onClick={cancel}>
          Cancelar
        </button>
      </div>
    </form>
  );

  return (
    <section className="usage-section settings-section">
      <div className="settings-section-header">
        <h2 className="usage-heading">Warden API</h2>
        <button type="button" className="link-button" onClick={() => void load()} disabled={!conn || busy}>
          Recarregar
        </button>
      </div>
      <p className="skills-hint">
        O agente deste hub no formato da API da OpenAI, para scripts, n8n ou qualquer cliente que fale com a OpenAI. Endereço base{" "}
        <code>{baseUrl}</code>, modelo <code>warden</code> ou <code>warden/&lt;agente&gt;</code>. As tools mandadas pelo cliente são
        ignoradas, e nada vira conversa; o gasto entra no canal <code>api</code>.
      </p>

      {error && <p className="error-banner">{error}</p>}

      {created && (
        <div className="settings-card api-key-created">
          <p className="skills-item-description">
            Chave <strong>{created.name}</strong> criada. Copie agora: ela não aparece de novo.
          </p>
          <p>
            <code className="api-key-value">{created.key}</code>
          </p>
          <div className="skills-actions">
            <button type="button" className="primary-button" onClick={() => void copy(created.key)}>
              {copied ? "Copiada" : "Copiar"}
            </button>
            <button type="button" className="link-button" onClick={() => setCreated(null)}>
              Pronto
            </button>
          </div>
        </div>
      )}

      {keys && keys.length === 0 && <p className="skills-hint">Nenhuma chave ainda.</p>}
      {keys && keys.length > 0 && (
        <ul className="skills-list">
          {keys.map((key) => (
            <li key={key.id} className="skills-item">
              <div className="skills-item-header">
                <span className="skills-item-name">{key.name}</span>
                <code>{key.shown}…</code>
              </div>
              <p className="skills-item-description">
                Criada em {dateFormatter.format(new Date(key.createdAtMs))} ·{" "}
                {key.lastUsedAtMs ? `usada por último em ${dateFormatter.format(new Date(key.lastUsedAtMs))}` : "nunca usada"}
              </p>
              {asking?.kind === "revoke" && asking.key.id === key.id ? (
                keyPrompt
              ) : (
                <div className="skills-actions">
                  <button type="button" className="link-button skills-danger" disabled={!conn || asking !== null} onClick={() => setAsking({ kind: "revoke", key })}>
                    Revogar
                  </button>
                </div>
              )}
            </li>
          ))}
        </ul>
      )}

      {asking?.kind === "create" ? (
        keyPrompt
      ) : (
        <form
          className="skills-actions"
          onSubmit={(e) => {
            e.preventDefault();
            setAsking({ kind: "create", name });
          }}
        >
          <input value={name} onChange={(e) => setName(e.target.value)} placeholder="Nome da chave (ex.: n8n)" maxLength={60} aria-label="Nome da chave" />
          <button type="submit" className="primary-button" disabled={!conn || asking !== null || name.trim() === ""}>
            Criar chave
          </button>
        </form>
      )}
    </section>
  );
}
