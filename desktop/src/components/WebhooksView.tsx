import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { AgentEntry } from "../types";

/** How a webhook's caller proves itself: a bearer token, or an HMAC signature of the body (GitHub, Stripe style). */
type WebhookAuth = "token" | "hmac";

/** Mirrors `WebhookDto` (P105): one incoming webhook, as `[[webhooks]]` keeps it. */
interface Webhook {
  id: string;
  agentId?: string;
  prompt: string;
  enabled: boolean;
  auth: WebhookAuth;
}

/** Mirrors `WebhookInfoDto`: a webhook and what this machine knows about its credential. */
interface WebhookInfo extends Webhook {
  /** What it has: absent means no credential, so it takes no calls. */
  credential?: WebhookAuth;
  shown?: string;
  createdAtMs?: number;
  lastUsedAtMs?: number;
  conversation: string;
}

/** Mirrors `webhook_cmds::WebhookListPayload`. */
interface WebhookList {
  webhooks: WebhookInfo[];
  hubRunning: boolean;
  hubUrl?: string;
}

/** Mirrors `webhook_cmds::WebhookCreatedPayload`: the credential is shown once, here. */
interface WebhookCreated {
  id: string;
  credential: string;
  kind: WebhookAuth;
  list: WebhookList;
}

interface WebhookMessage {
  role: "user" | "assistant";
  content: string;
  createdAt: number;
}

interface EditorState {
  originalId?: string;
  /** What the webhook wanted before this edit, to warn that changing it drops the credential. */
  originalAuth?: WebhookAuth;
  webhook: Webhook;
}

const dateFormatter = new Intl.DateTimeFormat(undefined, { dateStyle: "short", timeStyle: "short" });

const authNames: Record<WebhookAuth, string> = { token: "token", hmac: "HMAC signature" };
const credentialNames: Record<WebhookAuth, string> = { token: "token", hmac: "signing secret" };

function editorFor(hook: WebhookInfo): EditorState {
  return { originalId: hook.id, originalAuth: hook.auth, webhook: { id: hook.id, agentId: hook.agentId, prompt: hook.prompt, enabled: hook.enabled, auth: hook.auth } };
}

function statusLine(hook: WebhookInfo): string {
  const parts: string[] = [];
  if (!hook.credential) parts.push("no credential: takes no calls");
  else if (hook.credential !== hook.auth) parts.push(`the credential is a ${credentialNames[hook.credential]} but the webhook wants a ${authNames[hook.auth]}: make a new one`);
  else {
    parts.push(`${credentialNames[hook.credential]} ${hook.shown ?? ""}…`);
    if (hook.createdAtMs) parts.push(`created ${dateFormatter.format(hook.createdAtMs)}`);
    parts.push(hook.lastUsedAtMs ? `last used ${dateFormatter.format(hook.lastUsedAtMs)}` : "never used");
  }
  if (!hook.enabled) parts.push("paused");
  return parts.join(" · ");
}

/** A webhook's conversation on this machine, read-only: the last answer, and the rest on request. */
function WebhookHistory({ id, refreshKey }: { id: string; refreshKey: number }) {
  const [messages, setMessages] = useState<WebhookMessage[] | null>(null);
  const [all, setAll] = useState(false);

  useEffect(() => {
    invoke<WebhookMessage[]>("webhook_history", { id })
      .then(setMessages)
      .catch(() => setMessages([]));
  }, [id, refreshKey]);

  if (messages === null) return <p className="settings-hint">Loading…</p>;
  if (messages.length === 0) return <p className="settings-hint">No calls on this computer yet. If another hub takes it, open its conversation from the web or the phone.</p>;
  const shown = all ? messages : messages.filter((m) => m.role === "assistant").slice(-1);
  return (
    <div className="task-history">
      {shown.map((m, i) => (
        <div key={i} className={`task-history-message task-history-message--${m.role}`}>
          <span className="settings-hint">
            {m.role === "user" ? "Received" : "Answer"} · {dateFormatter.format(m.createdAt)}
          </span>
          <p className="task-history-content">{m.content}</p>
        </div>
      ))}
      {messages.length > 1 && (
        <button type="button" className="settings-browse-btn" onClick={() => setAll(!all)}>
          {all ? "Only the last answer" : `Whole history (${messages.length} messages)`}
        </button>
      )}
    </div>
  );
}

/**
 * P105 — incoming webhooks: a prompt an agent runs when something calls `POST /hooks/<id>` on this computer's embedded
 * hub with the webhook's credential. The same list, create, edit, pause, remove, credential and revoke as
 * `warden-server webhooks` and the web's Webhooks tab, on this machine's `config.toml` and credentials file. A new
 * credential is shown once, here, and never again.
 */
function WebhooksView({ agents }: { agents: AgentEntry[] }) {
  const [list, setList] = useState<WebhookList | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [editor, setEditor] = useState<EditorState | null>(null);
  const [saving, setSaving] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);
  const [expanded, setExpanded] = useState<string | null>(null);
  const [refreshKey, setRefreshKey] = useState(0);
  const [created, setCreated] = useState<{ id: string; credential: string; kind: WebhookAuth } | null>(null);
  const [copied, setCopied] = useState(false);

  const load = useCallback(() => {
    invoke<WebhookList>("list_webhooks")
      .then((next) => {
        setList(next);
        setRefreshKey((k) => k + 1);
      })
      .catch((err) => setError(String(err)));
  }, []);

  useEffect(load, [load]);

  // Calls come from outside, so look again now and then: the last use and the last answer change by themselves.
  useEffect(() => {
    const timer = window.setInterval(load, 5000);
    return () => window.clearInterval(timer);
  }, [load]);

  async function act(command: string, args: Record<string, unknown>): Promise<boolean> {
    setError(null);
    try {
      setList(await invoke<WebhookList>(command, args));
      setRefreshKey((k) => k + 1);
      return true;
    } catch (err) {
      setError(String(err));
      return false;
    }
  }

  async function makeCredential(hook: WebhookInfo) {
    setError(null);
    try {
      const made = await invoke<WebhookCreated>("create_webhook_credential", { id: hook.id });
      setList(made.list);
      setRefreshKey((k) => k + 1);
      setCreated({ id: made.id, credential: made.credential, kind: made.kind });
      setCopied(false);
    } catch (err) {
      setError(String(err));
    }
  }

  async function handleSave() {
    if (!editor) return;
    setSaving(true);
    const saved = await act("save_webhook", { originalId: editor.originalId ?? null, webhook: editor.webhook });
    setSaving(false);
    if (saved) setEditor(null);
  }

  async function copy(text: string) {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
    } catch {
      setCopied(false);
    }
  }

  function updateHook(patch: Partial<Webhook>) {
    setEditor((current) => (current ? { ...current, webhook: { ...current.webhook, ...patch } } : current));
  }

  /** Where a caller reaches a webhook: the embedded hub's address while it runs, else only the path. */
  function addressOf(id: string): string {
    return list?.hubUrl ? `${list.hubUrl}/hooks/${id}` : `/hooks/${id}`;
  }

  const dropsCredential = editor?.originalAuth !== undefined && editor.originalAuth !== editor.webhook.auth;

  return (
    <div className="settings-view">
      <h2 className="settings-title">Webhooks</h2>
      <p className="settings-hint">
        Prompts an agent runs when a service calls the webhook's address. Each call lands in the webhook's conversation, which every device connected to this hub
        can open.
      </p>
      {list && !list.hubRunning && (
        <p className="settings-hint">
          Calls only arrive while this computer's embedded hub is on (Workspace). Until then these are just the requests kept in the configuration.
        </p>
      )}
      {error && <p className="settings-error-banner">{error}</p>}

      {created && (
        <div className="api-key-created">
          <p className="settings-hint">
            {created.kind === "token" ? "Token" : "Signing secret"} for <strong>{created.id}</strong> created. Copy it now — it isn't shown again.
          </p>
          <code className="api-key-value">{created.credential}</code>
          {created.kind === "token" ? (
            <p className="settings-hint">
              The service calls <code>{addressOf(created.id)}</code> with <code>Authorization: Bearer {"<token>"}</code> (or <code>X-Warden-Token</code>), for example:{" "}
              <code className="api-key-value">{`curl -X POST -H "Authorization: Bearer ${created.credential}" --data-binary @body.json ${addressOf(created.id)}`}</code>
            </p>
          ) : (
            <p className="settings-hint">
              Paste it as the <em>Secret</em> of the webhook on GitHub (content type application/json) with the address <code>{addressOf(created.id)}</code>, as the
              endpoint's signing secret on Stripe, or as the app's Signing Secret on Slack. A plain token doesn't open this webhook. This computer keeps the secret in the clear, in{" "}
              <code>webhook_tokens.json</code> (mode 0600): checking a signature means making one.
            </p>
          )}
          <div className="api-key-actions">
            <button type="button" className="settings-browse-btn" onClick={() => void copy(created.credential)}>
              {copied ? "Copied" : "Copy"}
            </button>
            <button type="button" className="settings-browse-btn" onClick={() => setCreated(null)}>
              Done
            </button>
          </div>
        </div>
      )}

      {editor && (
        <div className="provider-card skill-editor">
          <span className="settings-section-title">{editor.originalId ? `Edit ${editor.originalId}` : "New webhook"}</span>

          <label className="settings-field">
            <span className="settings-label">Name</span>
            <input className="settings-input" type="text" placeholder="build-failed" value={editor.webhook.id} onChange={(e) => updateHook({ id: e.currentTarget.value })} />
            <span className="settings-hint">Letters, digits, - and _ (up to 54). It ends the address, /hooks/&lt;name&gt;, and its conversation is task-hook-&lt;name&gt;.</span>
          </label>

          <label className="settings-field">
            <span className="settings-label">Agent</span>
            <select className="settings-select" value={editor.webhook.agentId ?? ""} onChange={(e) => updateHook({ agentId: e.currentTarget.value || undefined })}>
              <option value="">(none)</option>
              {agents.map((a) => (
                <option key={a.id} value={a.id}>
                  {a.id}
                </option>
              ))}
            </select>
            <span className="settings-hint">The body of a call comes from outside: give this webhook an agent with few tools.</span>
          </label>

          <label className="settings-field">
            <span className="settings-label">What to ask</span>
            <textarea
              className="settings-input settings-textarea"
              rows={6}
              placeholder="Say why the build failed and what to do."
              value={editor.webhook.prompt}
              onChange={(e) => updateHook({ prompt: e.currentTarget.value })}
            />
            <span className="settings-hint">The body of the call is handed to the agent after this, as data — never as instructions.</span>
          </label>

          <label className="settings-field">
            <span className="settings-label">How the service identifies itself</span>
            <select className="settings-select" value={editor.webhook.auth} onChange={(e) => updateHook({ auth: e.currentTarget.value as WebhookAuth })}>
              <option value="token">Token (Authorization: Bearer)</option>
              <option value="hmac">HMAC signature (GitHub, Gitea, Stripe, Slack)</option>
            </select>
            <span className="settings-hint">
              {editor.webhook.auth === "token"
                ? "The service sends the token in Authorization: Bearer (or X-Warden-Token). Only a hash of it is kept."
                : "For services that sign what they send: X-Hub-Signature-256 (GitHub, Gitea, Forgejo) , Stripe-Signature or X-Slack-Signature. The secret is kept in the clear (file mode 0600): checking a signature means making one."}
            </span>
          </label>
          {dropsCredential && <p className="settings-hint">Changing the type deletes the current credential: the webhook takes no calls until you make a new one.</p>}

          <div className="skill-editor-actions">
            <button type="button" className="settings-save-btn" onClick={() => void handleSave()} disabled={saving}>
              {saving ? "Saving…" : "Save webhook"}
            </button>
            <button type="button" className="settings-browse-btn" onClick={() => setEditor(null)} disabled={saving}>
              Cancel
            </button>
          </div>
        </div>
      )}

      <section className="settings-section">
        <div className="settings-section-header">
          <h3 className="settings-section-title">Your webhooks</h3>
          <button type="button" className="settings-browse-btn" onClick={() => setEditor({ webhook: { id: "", prompt: "", enabled: true, auth: "token" } })}>
            + New webhook
          </button>
        </div>

        {list === null ? (
          <p className="settings-hint">Loading webhooks…</p>
        ) : list.webhooks.length === 0 ? (
          <p className="settings-hint">No webhooks yet.</p>
        ) : (
          <div className="provider-list">
            {list.webhooks.map((hook) => (
              <div className="provider-card skill-card" key={hook.id}>
                <div className="skill-card-header">
                  <span className="skill-card-name">{hook.id}</span>
                  <div className="skill-card-actions">
                    {confirmDelete === hook.id ? (
                      <>
                        <span className="settings-hint">Delete this webhook and its credential? Its conversation stays.</span>
                        <button type="button" className="provider-delete-btn" onClick={() => void act("delete_webhook", { id: hook.id }).then(() => setConfirmDelete(null))}>
                          Delete
                        </button>
                        <button type="button" className="settings-browse-btn" onClick={() => setConfirmDelete(null)}>
                          Keep
                        </button>
                      </>
                    ) : (
                      <>
                        <button type="button" className="settings-browse-btn" onClick={() => setExpanded(expanded === hook.id ? null : hook.id)}>
                          {expanded === hook.id ? "Hide result" : "Last result"}
                        </button>
                        <button type="button" className="settings-browse-btn" onClick={() => void makeCredential(hook)} title={hook.credential ? "The new one replaces the old one at once" : undefined}>
                          {hook.credential === hook.auth ? (hook.auth === "token" ? "Replace token" : "Replace secret") : hook.auth === "token" ? "Make token" : "Make secret"}
                        </button>
                        {hook.credential && (
                          <button type="button" className="settings-browse-btn" onClick={() => void act("revoke_webhook_credential", { id: hook.id })}>
                            Revoke
                          </button>
                        )}
                        <button type="button" className="settings-browse-btn" onClick={() => void act("set_webhook_enabled_cmd", { id: hook.id, enabled: !hook.enabled })}>
                          {hook.enabled ? "Pause" : "Resume"}
                        </button>
                        <button type="button" className="settings-browse-btn" onClick={() => setEditor(editorFor(hook))}>
                          Edit
                        </button>
                        <button
                          type="button"
                          className="provider-delete-btn"
                          onClick={() => setConfirmDelete(hook.id)}
                          aria-label={`Delete ${hook.id}`}
                          title="Delete this webhook"
                        >
                          🗑
                        </button>
                      </>
                    )}
                  </div>
                </div>
                <p className="skill-card-description">
                  POST {addressOf(hook.id)} · {authNames[hook.auth]} · {hook.agentId ? `agent ${hook.agentId}` : "no agent"}
                </p>
                <p className="settings-hint">{statusLine(hook)}</p>
                {expanded === hook.id && <WebhookHistory id={hook.id} refreshKey={refreshKey} />}
              </div>
            ))}
          </div>
        )}
      </section>
    </div>
  );
}

export default WebhooksView;
