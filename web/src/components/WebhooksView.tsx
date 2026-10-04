import { useCallback, useEffect, useState } from "react";
import { WebhookError, type ServerConnection } from "../hub/connection";
import type { Webhook, WebhookAuth, WebhookInfo } from "../hub/messages";

// Incoming webhooks (P105): a prompt an agent runs when something calls `POST /hooks/<id>` on this hub with the
// webhook's credential. The same list, create, edit, pause, remove, credential and revoke as `warden-server webhooks`.
// Every change asks for the pairing key, like the tasks and the API keys: a webhook lets a stranger's call run an agent,
// with its tools and spend. A new credential is shown once, here, and never again. Each call lands in the webhook's
// conversation (`task-hook-<id>`), which "Abrir conversa" opens in the chat.

interface EditorState {
  /** The id being edited; absent for a new webhook. */
  originalId?: string;
  /** What the webhook wanted before this edit, to warn that changing it drops the credential. */
  originalAuth?: WebhookAuth;
  webhook: Webhook;
}

type Change =
  | { kind: "save"; editor: EditorState }
  | { kind: "toggle"; hook: WebhookInfo }
  | { kind: "delete"; hook: WebhookInfo }
  | { kind: "credential"; hook: WebhookInfo }
  | { kind: "revoke"; hook: WebhookInfo };

/** A credential just made. */
interface Created {
  id: string;
  credential: string;
  kind: WebhookAuth;
}

const dateFormatter = new Intl.DateTimeFormat("pt-BR", { dateStyle: "short", timeStyle: "short" });

const authNames: Record<WebhookAuth, string> = { token: "token", hmac: "assinatura HMAC" };
const credentialNames: Record<WebhookAuth, string> = { token: "token", hmac: "segredo de assinatura" };

function message(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

function newEditor(): EditorState {
  return { webhook: { id: "", prompt: "", enabled: true, auth: "token" } };
}

function editorFor(hook: WebhookInfo): EditorState {
  return { originalId: hook.id, originalAuth: hook.auth, webhook: { id: hook.id, agentId: hook.agentId, prompt: hook.prompt, enabled: hook.enabled, auth: hook.auth } };
}

/** Where the hub is called, from the address this page was opened at. */
function hookUrl(id: string): string {
  return `${window.location.origin}/hooks/${id}`;
}

function status(hook: WebhookInfo): string {
  const parts: string[] = [];
  if (!hook.credential) parts.push("sem credencial: não recebe chamadas");
  else if (hook.credential !== hook.auth) parts.push(`a credencial é um ${credentialNames[hook.credential]}, mas o webhook quer ${authNames[hook.auth]}: gere uma nova`);
  else {
    parts.push(`${credentialNames[hook.credential]} ${hook.shown ?? ""}…`);
    if (hook.createdAtMs) parts.push(`criado em ${dateFormatter.format(hook.createdAtMs)}`);
    parts.push(hook.lastUsedAtMs ? `último uso: ${dateFormatter.format(hook.lastUsedAtMs)}` : "nunca usado");
  }
  if (!hook.enabled) parts.push("pausado");
  return parts.join(" · ");
}

export default function WebhooksView({ conn, onOpenConversation }: { conn: ServerConnection | null; onOpenConversation: (id: string) => void }) {
  const [hooks, setHooks] = useState<WebhookInfo[] | null>(null);
  const [servesHere, setServesHere] = useState(true);
  const [agentIds, setAgentIds] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [editor, setEditor] = useState<EditorState | null>(null);
  const [asking, setAsking] = useState<Change | null>(null);
  const [pairingKey, setPairingKey] = useState("");
  const [keyError, setKeyError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);
  const [created, setCreated] = useState<Created | null>(null);
  const [copied, setCopied] = useState(false);

  const refresh = useCallback(async () => {
    if (!conn) return;
    try {
      const list = await conn.listWebhooks();
      setHooks(list.webhooks);
      setServesHere(list.servesHere);
      setError(null);
    } catch (err) {
      setHooks((current) => current ?? []);
      setError(`falha ao listar os webhooks: ${message(err)}`);
    }
  }, [conn]);

  useEffect(() => {
    void refresh();
    if (!conn) return;
    conn
      .requestSettings()
      .then(({ settings }) => setAgentIds(settings.agents.map((a) => a.id)))
      .catch(() => setAgentIds([]));
    // A call finished: when the credential was last used changed.
    return conn.onConversationsChanged((id) => {
      if (id.startsWith("task-hook-")) void refresh();
    });
  }, [conn, refresh]);

  function cancelKey() {
    setAsking(null);
    setPairingKey("");
    setKeyError(null);
  }

  async function confirm() {
    if (!conn || !asking) return;
    setBusy(true);
    setKeyError(null);
    try {
      if (asking.kind === "credential") {
        const made = await conn.createWebhookCredential(pairingKey, asking.hook.id);
        setHooks(made.webhooks);
        setServesHere(made.servesHere);
        setCreated({ id: made.id, credential: made.credential, kind: made.kind });
        setCopied(false);
      } else {
        const list =
          asking.kind === "save"
            ? await conn.saveWebhook(pairingKey, asking.editor.webhook, asking.editor.originalId)
            : asking.kind === "toggle"
              ? await conn.setWebhookEnabled(pairingKey, asking.hook.id, !asking.hook.enabled)
              : asking.kind === "delete"
                ? await conn.deleteWebhook(pairingKey, asking.hook.id)
                : await conn.revokeWebhookCredential(pairingKey, asking.hook.id);
        setHooks(list.webhooks);
        setServesHere(list.servesHere);
        if (asking.kind === "save") setEditor(null);
        // What was shown once is only worth keeping on screen while it can still be copied from the right webhook.
        if (asking.kind === "delete" || asking.kind === "revoke") setCreated((current) => (current && current.id === asking.hook.id ? null : current));
      }
      setConfirmDelete(null);
      setError(null);
      cancelKey();
    } catch (err) {
      if (err instanceof WebhookError && err.authRejected) {
        setKeyError("Chave de pareamento errada.");
      } else {
        cancelKey();
        setError(message(err));
      }
    } finally {
      setBusy(false);
    }
  }

  function ask(change: Change) {
    setError(null);
    setKeyError(null);
    setAsking(change);
  }

  async function copy(text: string) {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
    } catch {
      setCopied(false);
    }
  }

  const labels: Record<Change["kind"], string> = { save: "Salvar o webhook", toggle: "Confirmar", delete: "Apagar", credential: "Gerar", revoke: "Revogar" };
  const keyPrompt = asking && (
    <form
      className="settings-confirm devices-confirm"
      onSubmit={(e) => {
        e.preventDefault();
        void confirm();
      }}
    >
      {asking.kind === "credential" && asking.hook.credential && (
        <p className="skills-hint">Já existe uma credencial: a nova a substitui, e a antiga para de funcionar na hora.</p>
      )}
      <label className="settings-field">
        Chave de pareamento do hub
        <input type="password" autoComplete="current-password" autoFocus value={pairingKey} onChange={(e) => setPairingKey(e.target.value)} />
        <span className="field-hint">A mesma do primeiro login. É pedida a cada mudança.</span>
      </label>
      {keyError && <p className="error-banner">{keyError}</p>}
      <div className="skills-actions">
        <button type="submit" className="primary-button" disabled={busy || pairingKey.trim() === "" || !conn}>
          {busy ? "Aguarde…" : labels[asking.kind]}
        </button>
        <button type="button" className="link-button" disabled={busy} onClick={cancelKey}>
          Cancelar
        </button>
      </div>
    </form>
  );

  function updateHook(patch: Partial<Webhook>) {
    setEditor((current) => (current ? { ...current, webhook: { ...current.webhook, ...patch } } : current));
  }

  if (editor) {
    const dropsCredential = editor.originalAuth !== undefined && editor.originalAuth !== editor.webhook.auth;
    return (
      <form
        className="skills-editor"
        onSubmit={(e) => {
          e.preventDefault();
          ask({ kind: "save", editor });
        }}
      >
        <strong>{editor.originalId ? `Editar ${editor.originalId}` : "Novo webhook"}</strong>
        <label>
          Nome
          <input value={editor.webhook.id} onChange={(e) => updateHook({ id: e.target.value })} placeholder="build-falhou" required />
          <span className="skills-hint">Letras, números, - e _ (até 54). Vira o fim do endereço, /hooks/&lt;nome&gt;, e a conversa se chama task-hook-&lt;nome&gt;.</span>
        </label>
        <label>
          Agente
          <select value={editor.webhook.agentId ?? ""} onChange={(e) => updateHook({ agentId: e.target.value || undefined })}>
            <option value="">(nenhum)</option>
            {agentIds.map((id) => (
              <option key={id} value={id}>
                {id}
              </option>
            ))}
          </select>
          <span className="skills-hint">O corpo da chamada vem de fora: dê a este webhook um agente com poucas tools.</span>
        </label>
        <label>
          O que pedir
          <textarea
            value={editor.webhook.prompt}
            onChange={(e) => updateHook({ prompt: e.target.value })}
            rows={6}
            placeholder="Diga por que o build falhou e o que fazer."
            required
          />
          <span className="skills-hint">O corpo da chamada é entregue ao agente depois disso, como dado (nunca como instrução).</span>
        </label>
        <label>
          Como o serviço se identifica
          <select value={editor.webhook.auth} onChange={(e) => updateHook({ auth: e.target.value as WebhookAuth })}>
            <option value="token">Token (Authorization: Bearer)</option>
            <option value="hmac">Assinatura HMAC (GitHub, Gitea, Stripe)</option>
          </select>
          <span className="skills-hint">
            {editor.webhook.auth === "token"
              ? "O serviço manda o token em Authorization: Bearer (ou X-Warden-Token). O hub guarda só um hash dele."
              : "Para serviços que assinam o que enviam: X-Hub-Signature-256 (GitHub, Gitea, Forgejo) ou Stripe-Signature. O hub guarda o segredo em texto puro (arquivo 0600): conferir uma assinatura exige refazê-la."}
          </span>
        </label>
        {dropsCredential && <p className="skills-hint">Mudar o tipo apaga a credencial atual: o webhook para de receber chamadas até você gerar uma nova.</p>}
        {error && <p className="error-banner">{error}</p>}
        {asking ? (
          keyPrompt
        ) : (
          <div className="skills-actions">
            <button type="submit" className="primary-button" disabled={!conn}>
              Salvar
            </button>
            <button
              type="button"
              className="link-button"
              onClick={() => {
                setEditor(null);
                setError(null);
              }}
            >
              Cancelar
            </button>
          </div>
        )}
      </form>
    );
  }

  const createdHook = created && hooks?.find((h) => h.id === created.id);

  return (
    <div className="skills-view">
      <div className="skills-toolbar">
        <span className="skills-hint">Pedidos que um agente faz quando um serviço chama o endereço do webhook. Cada chamada cai na conversa dele.</span>
        <button
          type="button"
          className="primary-button"
          onClick={() => {
            setError(null);
            setEditor(newEditor());
          }}
        >
          + Novo
        </button>
      </div>
      {!servesHere && <p className="skills-hint">Este hub não está recebendo chamadas de webhook, então estes são só os pedidos guardados na configuração.</p>}
      {error && <p className="error-banner">{error}</p>}
      {asking && keyPrompt}

      {created && (
        <div className="settings-card api-key-created">
          <p className="skills-item-description">
            {created.kind === "token" ? "Token" : "Segredo de assinatura"} de <strong>{created.id}</strong> criado. Copie agora: ele não aparece de novo.
          </p>
          <p>
            <code className="api-key-value">{created.credential}</code>
          </p>
          {created.kind === "token" ? (
            <p className="skills-hint">
              O serviço chama <code>{hookUrl(created.id)}</code> com <code>Authorization: Bearer {"<token>"}</code> (ou <code>X-Warden-Token</code>), por exemplo:{" "}
              <code className="api-key-value">{`curl -X POST -H "Authorization: Bearer ${created.credential}" --data-binary @corpo.json ${hookUrl(created.id)}`}</code>
            </p>
          ) : (
            <p className="skills-hint">
              Cole como o <em>Secret</em> do webhook no GitHub (Content type: application/json), com o endereço <code>{hookUrl(created.id)}</code>, ou como o segredo
              de assinatura do endpoint no Stripe. Um token comum não abre este webhook. O hub guarda o segredo em texto puro, no arquivo <code>webhook_tokens.json</code>{" "}
              (modo 0600).
            </p>
          )}
          {createdHook && !createdHook.enabled && <p className="skills-hint">O webhook está pausado: retome para ele receber chamadas.</p>}
          <div className="skills-actions">
            <button type="button" className="primary-button" onClick={() => void copy(created.credential)}>
              {copied ? "Copiado" : "Copiar"}
            </button>
            <button type="button" className="link-button" onClick={() => setCreated(null)}>
              Pronto
            </button>
          </div>
        </div>
      )}

      {hooks === null ? (
        <p className="skills-hint">Carregando…</p>
      ) : hooks.length === 0 ? (
        <p className="skills-hint">Nenhum webhook ainda.</p>
      ) : (
        <ul className="skills-list">
          {hooks.map((hook) => (
            <li key={hook.id} className="skills-item">
              <div className="skills-item-header">
                <span className="skills-item-name">{hook.id}</span>
                {confirmDelete === hook.id ? (
                  <span className="skills-actions">
                    <button type="button" className="link-button skills-danger" onClick={() => ask({ kind: "delete", hook })}>
                      Apagar
                    </button>
                    <button type="button" className="link-button" onClick={() => setConfirmDelete(null)}>
                      Manter
                    </button>
                  </span>
                ) : (
                  <span className="skills-actions">
                    <button type="button" className="link-button" onClick={() => onOpenConversation(hook.conversation)}>
                      Abrir conversa
                    </button>
                    <button type="button" className="link-button" onClick={() => ask({ kind: "credential", hook })}>
                      {hook.credential === hook.auth ? (hook.auth === "token" ? "Trocar token" : "Trocar segredo") : hook.auth === "token" ? "Gerar token" : "Gerar segredo"}
                    </button>
                    {hook.credential && (
                      <button type="button" className="link-button" onClick={() => ask({ kind: "revoke", hook })}>
                        Revogar
                      </button>
                    )}
                    <button type="button" className="link-button" onClick={() => ask({ kind: "toggle", hook })}>
                      {hook.enabled ? "Pausar" : "Retomar"}
                    </button>
                    <button
                      type="button"
                      className="link-button"
                      onClick={() => {
                        setError(null);
                        setEditor(editorFor(hook));
                      }}
                    >
                      Editar
                    </button>
                    <button type="button" className="link-button" onClick={() => setConfirmDelete(hook.id)}>
                      Apagar
                    </button>
                  </span>
                )}
              </div>
              <p className="skills-item-description">
                POST {hookUrl(hook.id)} · {authNames[hook.auth]} · {hook.agentId ? `agente ${hook.agentId}` : "sem agente"}
              </p>
              <p className="skills-hint">{status(hook)}</p>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
