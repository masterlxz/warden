import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { isMcpServerHttp } from "../types";
import type { AgentEntry, Combo, GitSyncConfig, McpServer, ProviderEntry, ProviderKind, Settings, SshHostEntry } from "../types";
import ApiKeysSection from "./ApiKeysSection";
import BotsSection from "./BotsSection";
import SpendingSection, { validateSpending } from "./SpendingSection";
import { APPROVAL_CATEGORIES } from "../lib/approvalCategories";
import { descendantsOf, removeFromOrg, renameInReports } from "../lib/org";

const emptySettings: Settings = {
  providers: [],
  activeProvider: "",
  combos: [],
  vaultPath: "",
  generatedPath: "",
  tavilyKey: "",
  whisperKey: "",
  enableShell: false,
  defaultModels: {},
  mcpServers: [],
  agents: [],
  sshHosts: [],
  gitSync: null,
  limits: null,
  defaultLimits: [],
  limitsDisabledByEnv: false,
  prices: [],
  version: "",
};

/** A combo's providers (P90), in the order they're tried when one is down (429, 5xx, no
 * connection). Each entry is a provider id; the picker only offers ones not already listed. */
function ProviderOrderEditor({
  value,
  providers,
  onChange,
}: {
  value: string[];
  providers: ProviderEntry[];
  onChange: (next: string[]) => void;
}) {
  const available = providers.map((p) => p.id).filter((id) => id !== "" && !value.includes(id));

  function move(index: number, delta: number) {
    const next = [...value];
    const [item] = next.splice(index, 1);
    next.splice(index + delta, 0, item);
    onChange(next);
  }

  return (
    <div className="fallback-list">
      {value.length === 0 && <p className="settings-hint">No providers yet — a combo needs at least one.</p>}
      {value.map((id, index) => (
        <div key={id} className="fallback-row">
          <span className="fallback-order">{index + 1}.</span>
          <span className="fallback-name">{id}</span>
          <button type="button" className="provider-delete-btn" disabled={index === 0} onClick={() => move(index, -1)} title="Move up" aria-label={`Move ${id} up`}>
            ↑
          </button>
          <button
            type="button"
            className="provider-delete-btn"
            disabled={index === value.length - 1}
            onClick={() => move(index, 1)}
            title="Move down"
            aria-label={`Move ${id} down`}
          >
            ↓
          </button>
          <button type="button" className="provider-delete-btn" onClick={() => onChange(value.filter((v) => v !== id))} title="Remove" aria-label={`Remove ${id}`}>
            ✕
          </button>
        </div>
      ))}
      {available.length > 0 && (
        <select
          className="settings-select"
          value=""
          onChange={(e) => {
            if (e.currentTarget.value) onChange([...value, e.currentTarget.value]);
          }}
        >
          <option value="">+ Add a provider…</option>
          {available.map((id) => (
            <option key={id} value={id}>
              {id}
            </option>
          ))}
        </select>
      )}
    </div>
  );
}

const emptyGitSync: GitSyncConfig = { remoteUrl: "", token: "" };

/** Connection form for the git sync backend (P63/P71 v2) — a self-hosted/remote git repo as an
 * alternative to Arweave for the Sync screen's push/pull (and the auto-sync loop). The vault itself
 * always lives on this machine (P61). `value` is `null` until the user starts typing; `onChange`
 * always passes a complete object back so partial edits never get lost between keystrokes. */
function GitSyncForm({ value, onChange }: { value: GitSyncConfig | null; onChange: (next: GitSyncConfig) => void }) {
  const current = value ?? emptyGitSync;

  function set<K extends keyof GitSyncConfig>(key: K, v: GitSyncConfig[K]) {
    onChange({ ...current, [key]: v });
  }

  return (
    <div className="git-sync-form">
      <label className="settings-field">
        <span className="settings-label">Remote URL</span>
        <input
          className="settings-input"
          type="text"
          placeholder="https://gitea.example.com/user/vault.git"
          value={current.remoteUrl}
          onChange={(e) => set("remoteUrl", e.currentTarget.value)}
        />
      </label>
      <ApiKeyField label="Token" value={current.token} onChange={(v) => set("token", v)} />
    </div>
  );
}

const PROVIDER_KIND_OPTIONS: { value: ProviderKind; label: string }[] = [
  { value: "gemini", label: "Gemini" },
  { value: "openai", label: "OpenAI" },
  { value: "anthropic", label: "Anthropic" },
  { value: "openai_compatible", label: "OpenAI-compatible (Ollama, OpenRouter, Groq, ...)" },
  { value: "node", label: "A node's model (another machine)" },
];

/** Known-good starting points for popular integrations — researched 2026-08-29 (Sessão 35; see
 * ARCHITECTURE.md for why GitHub goes through Docker). Env/header values are left blank on
 * purpose — the user fills in their own secret after picking a preset. */
const MCP_PRESETS: { label: string; server: McpServer }[] = [
  { label: "Filesystem", server: { name: "filesystem", command: "npx", args: ["-y", "@modelcontextprotocol/server-filesystem", "/path/to/allowed/dir"], env: {} } },
  { label: "Google Workspace", server: { name: "google_workspace", command: "npx", args: ["-y", "@aaronsb/google-workspace-mcp"], env: { GOOGLE_CLIENT_ID: "", GOOGLE_CLIENT_SECRET: "" } } },
  { label: "Notion", server: { name: "notion", command: "npx", args: ["-y", "@notionhq/notion-mcp-server"], env: { NOTION_TOKEN: "" } } },
  {
    label: "GitHub (via Docker)",
    server: { name: "github", command: "docker", args: ["run", "-i", "--rm", "-e", "GITHUB_PERSONAL_ACCESS_TOKEN", "ghcr.io/github/github-mcp-server"], env: { GITHUB_PERSONAL_ACCESS_TOKEN: "" } },
  },
  {
    // Verified endpoint (2026-08-31, Sessão 36 / P25): https://mcp.slack.com/mcp, Streamable
    // HTTP, real OAuth (discovery + Dynamic Client Registration + browser consent — see
    // PENDING.md P26). Click "Connect" on this card after saving to authorize it.
    label: "Slack (hosted — OAuth)",
    server: { name: "slack", url: "https://mcp.slack.com/mcp", headers: {}, oauth: true },
  },
];

/** Reused outside this file too (e.g. `WorkspaceView.tsx`'s embedded-server auth key) — same
 * reveal/hide affordance for any secret the desktop shows in an editable field. */
export function ApiKeyField({
  label,
  value,
  onChange,
}: {
  label: string;
  value: string;
  onChange: (value: string) => void;
}) {
  const [revealed, setRevealed] = useState(false);

  return (
    <label className="settings-field">
      <span className="settings-label">{label}</span>
      <div className="settings-key-field">
        <input
          className="settings-input"
          type={revealed ? "text" : "password"}
          placeholder="Not set"
          value={value}
          onChange={(e) => onChange(e.currentTarget.value)}
          autoComplete="off"
        />
        <button
          type="button"
          className="settings-key-toggle"
          aria-label={revealed ? `Hide ${label}` : `Reveal ${label}`}
          onClick={() => setRevealed((r) => !r)}
        >
          {revealed ? "🙈" : "👁"}
        </button>
      </div>
    </label>
  );
}

/** First name not already used by another provider — "provider-2", "provider-3", etc. */
function nextProviderId(existing: ProviderEntry[]): string {
  let n = existing.length + 1;
  while (existing.some((p) => p.id === `provider-${n}`)) {
    n += 1;
  }
  return `provider-${n}`;
}

function ProviderCard({
  provider,
  isActive,
  defaultModel,
  onChange,
  onDelete,
  onSetActive,
}: {
  provider: ProviderEntry;
  isActive: boolean;
  defaultModel: string | undefined;
  onChange: (next: ProviderEntry) => void;
  onDelete: () => void;
  onSetActive: () => void;
}) {
  const isOpenAiCompatible = provider.kind === "openai_compatible";
  const isNode = provider.kind === "node";
  /** P10 — "Test key": asks the provider for its model list with the key as typed (saved or not). */
  const [testing, setTesting] = useState(false);
  const [keyTest, setKeyTest] = useState<{ ok: boolean; kind: string; message: string } | null>(null);

  async function testKey() {
    setTesting(true);
    setKeyTest(null);
    try {
      setKeyTest(await invoke<{ ok: boolean; kind: string; message: string }>("test_provider_key", { provider }));
    } catch (err) {
      setKeyTest({ ok: false, kind: "unreachable", message: String(err) });
    } finally {
      setTesting(false);
    }
  }

  return (
    <div className={`provider-card${isActive ? " provider-card-active" : ""}`}>
      <div className="provider-card-header">
        <label className="provider-active-toggle" title="Use this provider for new messages">
          <input type="radio" name="active-provider" checked={isActive} onChange={onSetActive} />
          <span>Active</span>
        </label>
        <input
          className="settings-input provider-name-input"
          type="text"
          placeholder="Name (e.g. ollama-local)"
          value={provider.id}
          onChange={(e) => onChange({ ...provider, id: e.currentTarget.value })}
        />
        <button
          type="button"
          className="provider-delete-btn"
          onClick={onDelete}
          aria-label={`Delete ${provider.id || "this provider"}`}
          title="Delete this provider"
        >
          🗑
        </button>
      </div>

      <label className="settings-field">
        <span className="settings-label">Provider type</span>
        <select
          className="settings-select"
          value={provider.kind}
          onChange={(e) => onChange({ ...provider, kind: e.currentTarget.value as ProviderKind })}
        >
          {PROVIDER_KIND_OPTIONS.map((opt) => (
            <option key={opt.value} value={opt.value}>
              {opt.label}
            </option>
          ))}
        </select>
      </label>

      {isOpenAiCompatible && (
        <label className="settings-field">
          <span className="settings-label">Base URL</span>
          <input
            className="settings-input"
            type="text"
            placeholder="http://localhost:11434/v1"
            value={provider.baseUrl}
            onChange={(e) => onChange({ ...provider, baseUrl: e.currentTarget.value })}
          />
        </label>
      )}

      {isNode && (
        <label className="settings-field">
          <span className="settings-label">Node device id</span>
          <input
            className="settings-input"
            type="text"
            placeholder="node-home-pc-1a2b3c4d"
            value={provider.node ?? ""}
            onChange={(e) => onChange({ ...provider, node: e.currentTarget.value })}
          />
          <span className="settings-hint">
            Answers only through a hub with that node online, approved and allowed (Workspace → Nodes). In a combo, the
            next provider answers while it's out.
          </span>
        </label>
      )}

      {!isNode && (
        <ApiKeyField
          label={isOpenAiCompatible ? "API key (optional — most local servers don't need one)" : "API key"}
          value={provider.apiKey}
          onChange={(v) => onChange({ ...provider, apiKey: v })}
        />
      )}

      {!isNode && (
        <div className="settings-field">
          <button type="button" className="settings-browse-btn" disabled={testing} onClick={() => void testKey()}>
            {testing ? "Testing…" : "Test key"}
          </button>
          <span className="settings-hint">Asks the provider for its model list: it costs no tokens and counts against no limit.</span>
          {keyTest && (
            <div
              role="status"
              className={keyTest.ok ? "settings-success-banner" : keyTest.kind === "unverifiable" || keyTest.kind === "rate_limited" ? "settings-hint" : "settings-error-banner"}
            >
              {keyTest.ok ? "✓ " : keyTest.kind === "unverifiable" || keyTest.kind === "rate_limited" ? "⚠ " : "✗ "}
              {keyTest.message}
            </div>
          )}
        </div>
      )}

      <label className="settings-field">
        <span className="settings-label">{isNode ? "Provider on the node" : "Model"}</span>
        <input
          className="settings-input"
          type="text"
          placeholder={isNode ? "Its id in the node's config.toml, as in --model (e.g. ollama)" : defaultModel ? `Default: ${defaultModel}` : isOpenAiCompatible ? "Required, e.g. llama3.1" : ""}
          value={provider.model}
          onChange={(e) => onChange({ ...provider, model: e.currentTarget.value })}
        />
      </label>
    </div>
  );
}

/** First name not already used by another agent — "agent-2", "agent-3", etc., same scheme as
 * `nextProviderId`. */
function nextAgentId(existing: AgentEntry[]): string {
  let n = existing.length + 1;
  while (existing.some((a) => a.id === `agent-${n}`)) {
    n += 1;
  }
  return `agent-${n}`;
}

function nextSshHostId(existing: SshHostEntry[]): string {
  let n = existing.length + 1;
  while (existing.some((h) => h.id === `server-${n}`)) {
    n += 1;
  }
  return `server-${n}`;
}

function SshHostCard({
  host,
  agents,
  onChange,
  onDelete,
}: {
  host: SshHostEntry;
  agents: AgentEntry[];
  onChange: (next: SshHostEntry) => void;
  onDelete: () => void;
}) {
  const [testing, setTesting] = useState(false);
  const [testResult, setTestResult] = useState<{ ok: boolean; message: string } | null>(null);

  async function testConnection() {
    setTesting(true);
    setTestResult(null);
    try {
      setTestResult(await invoke<{ ok: boolean; message: string }>("test_ssh_host", { host }));
    } catch (err) {
      setTestResult({ ok: false, message: String(err) });
    } finally {
      setTesting(false);
    }
  }

  function toggleAgent(id: string, checked: boolean) {
    onChange({ ...host, agents: checked ? [...host.agents.filter((a) => a !== id), id] : host.agents.filter((a) => a !== id) });
  }

  return (
    <div className="provider-card">
      <div className="provider-card-header">
        <input
          className="settings-input provider-name-input"
          type="text"
          placeholder="Name (e.g. my-vps)"
          value={host.id}
          onChange={(e) => onChange({ ...host, id: e.currentTarget.value })}
        />
        <button
          type="button"
          className="provider-delete-btn"
          onClick={onDelete}
          aria-label={`Delete ${host.id || "this server"}`}
          title="Delete this server"
        >
          🗑
        </button>
      </div>

      <label className="settings-field">
        <span className="settings-label">Host</span>
        <input
          className="settings-input"
          type="text"
          placeholder="203.0.113.7 or server.example.com"
          value={host.host}
          onChange={(e) => onChange({ ...host, host: e.currentTarget.value })}
        />
      </label>

      <label className="settings-field">
        <span className="settings-label">User</span>
        <input
          className="settings-input"
          type="text"
          placeholder="deploy"
          value={host.user}
          onChange={(e) => onChange({ ...host, user: e.currentTarget.value })}
        />
      </label>

      <label className="settings-field">
        <span className="settings-label">Port</span>
        <input
          className="settings-input"
          type="number"
          min={1}
          max={65535}
          value={host.port}
          onChange={(e) => onChange({ ...host, port: Number(e.currentTarget.value) })}
        />
      </label>

      <label className="settings-field">
        <span className="settings-label">Private key file (optional)</span>
        <input
          className="settings-input"
          type="text"
          placeholder="/home/you/.ssh/id_ed25519"
          value={host.identityFile}
          onChange={(e) => onChange({ ...host, identityFile: e.currentTarget.value })}
        />
        <span className="settings-hint">
          Only the path is stored — Warden never reads or copies the key. Leave empty to use ssh-agent and your
          ~/.ssh/config. A key with a passphrase must be loaded in ssh-agent; Warden can't type a passphrase.
        </span>
      </label>

      <div className="settings-field">
        <span className="settings-label">Available to</span>
        {agents.length === 0 ? (
          <span className="settings-hint">No agents configured, so every conversation can use this server.</span>
        ) : (
          <div className="skill-agent-list">
            {agents.map((a) => (
              <label className="skill-agent-option" key={a.id}>
                <input type="checkbox" checked={host.agents.includes(a.id)} onChange={(e) => toggleAgent(a.id, e.currentTarget.checked)} />
                {a.id}
              </label>
            ))}
          </div>
        )}
        <span className="settings-hint">
          None ticked means every agent <strong>and every chat without an agent</strong> (Telegram, WhatsApp, mobile)
          can run commands here. Tick agents to restrict it to them.
        </span>
      </div>

      <label className="settings-field settings-checkbox-field">
        <span className="settings-checkbox-row">
          <input type="checkbox" checked={host.enabled} onChange={(e) => onChange({ ...host, enabled: e.currentTarget.checked })} />
          <span className="settings-label">Let the AI use this server</span>
        </span>
        <span className="settings-hint">
          The AI can run any command as this user, and send or fetch files, with no sandbox — same trust as the
          local shell. Off keeps the server saved but invisible to it. Every call is logged to
          ~/.config/warden/ssh_audit.jsonl.
        </span>
      </label>

      <label className="settings-field settings-checkbox-field">
        <span className="settings-checkbox-row">
          <input
            type="checkbox"
            checked={host.requireApproval}
            onChange={(e) => onChange({ ...host, requireApproval: e.currentTarget.checked })}
          />
          <span className="settings-label">Ask me before every command or file transfer</span>
        </span>
        <span className="settings-hint">
          A prompt shows the exact command or paths and waits for your yes. Only this app and the interactive CLI can
          ask — on Telegram, WhatsApp, mobile and sub-agents the AI is refused on this server instead.
        </span>
      </label>

      <div className="settings-field">
        <button type="button" className="settings-browse-btn" onClick={testConnection} disabled={testing}>
          {testing ? "Testing…" : "Test connection"}
        </button>
        {testResult && (
          <div className={testResult.ok ? "settings-success-banner" : "settings-error-banner"} role="status">
            {testResult.ok ? "✓ " : "✗ "}
            {testResult.message}
          </div>
        )}
      </div>
    </div>
  );
}

/** One combo (P90): its name (also its id), whether it's the active model, and its providers. */
function ComboCard({
  combo,
  providers,
  isActive,
  onChange,
  onDelete,
  onSetActive,
}: {
  combo: Combo;
  providers: ProviderEntry[];
  isActive: boolean;
  onChange: (next: Combo) => void;
  onDelete: () => void;
  onSetActive: () => void;
}) {
  return (
    <div className="provider-card">
      <div className="provider-card-header">
        <label className="provider-active-toggle" title="Use this combo for new messages">
          <input type="radio" name="active-provider" checked={isActive} onChange={onSetActive} />
          <span>Active</span>
        </label>
        <input
          className="settings-input provider-name-input"
          type="text"
          value={combo.id}
          placeholder="combo name"
          aria-label="Combo name"
          onChange={(e) => onChange({ ...combo, id: e.currentTarget.value })}
        />
        <button type="button" className="provider-delete-btn" onClick={onDelete} aria-label={`Delete ${combo.id || "this combo"}`} title="Delete this combo">
          ✕
        </button>
      </div>
      <ProviderOrderEditor value={combo.providers} providers={providers} onChange={(members) => onChange({ ...combo, providers: members })} />
    </div>
  );
}

function AgentCard({
  agent,
  providers,
  combos,
  toolNames,
  people,
  allAgents,
  onChange,
  onDelete,
}: {
  agent: AgentEntry;
  /** Every agent of the owner's, for the "Reports to" list. */
  allAgents: AgentEntry[];
  providers: ProviderEntry[];
  combos: Combo[];
  /** Every tool the running app has, for the "Restrict tools" list. */
  toolNames: string[];
  /** P84 — the workspace's members, to share this agent with. */
  people: { id: string; name: string }[];
  onChange: (next: AgentEntry) => void;
  onDelete: () => void;
}) {
  return (
    <div className="provider-card">
      <div className="provider-card-header">
        <input
          className="settings-input provider-name-input"
          type="text"
          placeholder="Name (e.g. pirate)"
          value={agent.id}
          onChange={(e) => onChange({ ...agent, id: e.currentTarget.value })}
        />
        <button
          type="button"
          className="provider-delete-btn"
          onClick={onDelete}
          aria-label={`Delete ${agent.id || "this agent"}`}
          title="Delete this agent"
        >
          🗑
        </button>
      </div>

      <label className="settings-field">
        <span className="settings-label">Role</span>
        <input
          className="settings-input"
          type="text"
          placeholder="Optional, e.g. Head of engineering"
          value={agent.role ?? ""}
          onChange={(e) => onChange({ ...agent, role: e.currentTarget.value })}
        />
      </label>

      <label className="settings-field">
        <span className="settings-label">Reports to</span>
        <select
          className="settings-select"
          value={agent.reportsTo ?? ""}
          onChange={(e) => onChange({ ...agent, reportsTo: e.currentTarget.value || null })}
        >
          <option value="">(nobody)</option>
          {allAgents
            .filter((other) => other.id !== agent.id && !descendantsOf(allAgents, agent.id).has(other.id))
            .map((other) => (
              <option key={other.id} value={other.id}>
                {other.id}
              </option>
            ))}
        </select>
        <span className="settings-hint">Shown in the Organization view. It doesn't change what the agent may do.</span>
      </label>

      <label className="settings-field">
        <span className="settings-label">Personality</span>
        <textarea
          className="settings-input settings-textarea"
          placeholder="Describe how this agent should behave, e.g. 'You are a terse, no-nonsense assistant who always answers in bullet points.'"
          rows={3}
          value={agent.persona}
          onChange={(e) => onChange({ ...agent, persona: e.currentTarget.value })}
        />
      </label>

      <label className="settings-field">
        <span className="settings-label">Default model</span>
        <select
          className="settings-select"
          value={agent.providerId}
          onChange={(e) => onChange({ ...agent, providerId: e.currentTarget.value })}
        >
          <option value="">(use the conversation's active provider)</option>
          {providers.map((p) => (
            <option key={p.id} value={p.id}>
              {p.id}
            </option>
          ))}
          {combos.map((c) => (
            <option key={c.id} value={c.id}>
              {c.id} (combo)
            </option>
          ))}
        </select>
      </label>

      <label className="settings-field settings-checkbox-field">
        <span className="settings-checkbox-row">
          <input
            type="checkbox"
            checked={agent.canDelegateToAgents}
            onChange={(e) => onChange({ ...agent, canDelegateToAgents: e.currentTarget.checked })}
          />
          <span className="settings-label">Can delegate to other agents</span>
        </span>
        <span className="settings-hint">
          Lets this agent hand off part of a conversation to any other configured agent by name.
        </span>
      </label>

      <label className="settings-field settings-checkbox-field">
        <span className="settings-checkbox-row">
          <input
            type="checkbox"
            checked={agent.canManageAgents}
            onChange={(e) => onChange({ ...agent, canManageAgents: e.currentTarget.checked })}
          />
          <span className="settings-label">Can create and edit other agents</span>
        </span>
        <span className="settings-hint">
          Lets this agent write new agents (persona and model) when you ask. Every creation or edit is shown to you
          first and only happens if you approve it. It can't give any agent this power or the one above — only these
          checkboxes can — and it can't edit an agent that has either.
        </span>
      </label>

      <label className="settings-field settings-checkbox-field">
        <span className="settings-checkbox-row">
          <input
            type="checkbox"
            checked={agent.canMessageAgents}
            onChange={(e) => onChange({ ...agent, canMessageAgents: e.currentTarget.checked })}
          />
          <span className="settings-label">Can leave messages for other agents</span>
        </span>
        <span className="settings-hint">
          Lets this agent write to another agent as a colleague. The message and the answer land in a conversation
          ("this agent → the other") that shows up in your sidebar, where you can also talk to the other agent.
        </span>
      </label>

      <label className="settings-field settings-checkbox-field">
        <span className="settings-checkbox-row">
          <input
            type="checkbox"
            checked={agent.canManageTasks}
            onChange={(e) => onChange({ ...agent, canManageTasks: e.currentTarget.checked })}
          />
          <span className="settings-label">Can create and edit scheduled tasks</span>
        </span>
        <span className="settings-hint">
          Lets this agent turn "every weekday at 8, summarize the news" into a task (see Tasks). Every creation or edit
          is shown to you first and only happens if you approve it. An agent limited to some tools can only schedule
          agents whose tools fit in its own.
        </span>
      </label>

      <label className="settings-field">
        <span className="settings-label">Autonomy</span>
        <select
          className="settings-input"
          value={agent.autonomy}
          onChange={(e) => onChange({ ...agent, autonomy: Number(e.currentTarget.value) })}
        >
          <option value={1}>1 — Only answers (no tools)</option>
          <option value={2}>2 — Suggests (never changes anything itself)</option>
          <option value={3}>3 — Asks before every change</option>
          <option value={4}>4 — Acts on its own</option>
        </select>
        <span className="settings-hint">
          Reading tools always work at levels 2 and 3. A delegate never gets more autonomy than the agent that called
          it. An agent made by another agent starts at 3.
        </span>
      </label>

      <div className="settings-field" role="group" aria-label={`Actions ${agent.id || "this agent"} must get approved`}>
        <span className="settings-label">Always ask me before</span>
        {APPROVAL_CATEGORIES.map((category) => (
          <span key={category.id} className="settings-checkbox-row">
            <input
              type="checkbox"
              id={`approval-${agent.id}-${category.id}`}
              checked={agent.approvalRequired.includes(category.id)}
              onChange={(e) =>
                onChange({
                  ...agent,
                  approvalRequired: e.currentTarget.checked
                    ? [...agent.approvalRequired, category.id]
                    : agent.approvalRequired.filter((id) => id !== category.id),
                })
              }
            />
            <label htmlFor={`approval-${agent.id}-${category.id}`} title={category.hint}>
              {category.label}
            </label>
          </span>
        ))}
        <span className="settings-hint">
          Even at autonomy 4, a call in a ticked kind waits for your yes. Which tool belongs to which kind is built in;
          add your own in config.toml with [[tool_categories]].
        </span>
      </div>

      <label className="settings-field settings-checkbox-field">
        <span className="settings-checkbox-row">
          <input
            type="checkbox"
            checked={agent.allowedTools !== null}
            onChange={(e) => onChange({ ...agent, allowedTools: e.currentTarget.checked ? [] : null })}
          />
          <span className="settings-label">Restrict tools</span>
        </span>
        <span className="settings-hint">
          Off: this agent can use every tool the app has. On: only the ones ticked below (also when another agent
          delegates to it).
        </span>
      </label>
      {agent.allowedTools !== null && (
        <div className="settings-field agent-tool-list" role="group" aria-label={`Tools ${agent.id || "this agent"} may use`}>
          {[...new Set([...toolNames, ...agent.allowedTools])].map((tool) => (
            <span key={tool} className="settings-checkbox-row">
              <input
                type="checkbox"
                id={`tool-${agent.id}-${tool}`}
                checked={agent.allowedTools!.includes(tool)}
                onChange={(e) =>
                  onChange({
                    ...agent,
                    allowedTools: e.currentTarget.checked
                      ? [...agent.allowedTools!, tool]
                      : agent.allowedTools!.filter((t) => t !== tool),
                  })
                }
              />
              <label htmlFor={`tool-${agent.id}-${tool}`}>
                {tool}
                {!toolNames.includes(tool) && <span className="settings-hint"> (not available now)</span>}
              </label>
            </span>
          ))}
          {agent.allowedTools.length === 0 && (
            <span className="settings-hint">No tool ticked — this agent can only chat.</span>
          )}
        </div>
      )}
      {people.length > 0 && (
        <div className="settings-field agent-tool-list" role="group" aria-label={`People ${agent.id || "this agent"} is shared with`}>
          <span className="settings-label">Shared with</span>
          <span className="settings-hint">They use it with their own memory and only the tools you allow them (Workspace → People).</span>
          <span className="settings-checkbox-row">
            <input
              type="checkbox"
              id={`share-${agent.id}-all`}
              checked={(agent.sharedWith ?? []).includes("*")}
              onChange={(e) => onChange({ ...agent, sharedWith: e.currentTarget.checked ? ["*"] : [] })}
            />
            <label htmlFor={`share-${agent.id}-all`}>Everyone</label>
          </span>
          {!(agent.sharedWith ?? []).includes("*") &&
            people.map((p) => (
              <span key={p.id} className="settings-checkbox-row">
                <input
                  type="checkbox"
                  id={`share-${agent.id}-${p.id}`}
                  checked={(agent.sharedWith ?? []).includes(p.id)}
                  onChange={(e) =>
                    onChange({
                      ...agent,
                      sharedWith: e.currentTarget.checked ? [...(agent.sharedWith ?? []), p.id] : (agent.sharedWith ?? []).filter((s) => s !== p.id),
                    })
                  }
                />
                <label htmlFor={`share-${agent.id}-${p.id}`}>
                  {p.name} ({p.id})
                </label>
              </span>
            ))}
        </div>
      )}
    </div>
  );
}

/** Editor for a `Record<string, string>` shown as key/value rows — shared by env vars (stdio
 * servers) and headers (HTTP servers), the only two places this shape shows up. */
function KeyValueListField({
  label,
  addLabel,
  keyPlaceholder,
  entries,
  onChange,
}: {
  label: string;
  addLabel: string;
  keyPlaceholder: string;
  entries: Record<string, string>;
  onChange: (next: Record<string, string>) => void;
}) {
  const list = Object.entries(entries);

  function updateEntry(index: number, key: string, value: string) {
    const next = list.map((entry, i) => (i === index ? ([key, value] as [string, string]) : entry));
    onChange(Object.fromEntries(next));
  }

  function addEntry() {
    // A blank key is a valid (if temporary) React list item — it's the value being edited that
    // matters; a truly empty key just gets dropped on save the same way an empty provider name
    // would be, so nothing needs deduping here.
    onChange({ ...entries, "": "" });
  }

  function removeEntry(index: number) {
    onChange(Object.fromEntries(list.filter((_, i) => i !== index)));
  }

  return (
    <div className="settings-field">
      <span className="settings-label">{label}</span>
      {list.map(([key, value], i) => (
        <div className="env-var-row" key={i}>
          <input
            className="settings-input"
            type="text"
            placeholder={keyPlaceholder}
            value={key}
            onChange={(e) => updateEntry(i, e.currentTarget.value, value)}
          />
          <input
            className="settings-input"
            type="password"
            placeholder="value"
            value={value}
            onChange={(e) => updateEntry(i, key, e.currentTarget.value)}
          />
          <button type="button" className="provider-delete-btn" onClick={() => removeEntry(i)} aria-label={`Remove ${key || "this entry"}`}>
            ✕
          </button>
        </div>
      ))}
      <button type="button" className="settings-browse-btn" onClick={addEntry}>
        + {addLabel}
      </button>
    </div>
  );
}

/** Connect/Disconnect UI for an OAuth-authenticated MCP server (PENDING.md P26) — status comes
 * only from whether a token file exists locally (no network call, see `mcp_oauth_status`'s doc
 * comment); a token that's actually expired/unrefreshable only surfaces the next time the
 * orchestrator tries to use that server. Independent of the form's save state on purpose: a
 * "Connect" click persists the token immediately, whether or not this card's been saved yet — a
 * later Save (or the reconnect this panel already triggers) is what makes the orchestrator
 * actually pick the server up. */
function McpOAuthPanel({ name, url }: { name: string; url: string }) {
  const [connected, setConnected] = useState<boolean | null>(null);
  const [isBusy, setIsBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  function refreshStatus() {
    if (!name.trim()) {
      setConnected(false);
      return;
    }
    invoke<boolean>("mcp_oauth_status", { name })
      .then(setConnected)
      .catch(() => setConnected(false));
  }

  useEffect(refreshStatus, [name]);

  async function handleConnect() {
    setError(null);
    setIsBusy(true);
    try {
      await invoke("mcp_oauth_connect", { name, url });
      refreshStatus();
    } catch (err) {
      setError(String(err));
    } finally {
      setIsBusy(false);
    }
  }

  async function handleDisconnect() {
    setError(null);
    setIsBusy(true);
    try {
      await invoke("mcp_oauth_disconnect", { name });
      refreshStatus();
    } catch (err) {
      setError(String(err));
    } finally {
      setIsBusy(false);
    }
  }

  const canConnect = name.trim() !== "" && url.trim() !== "";

  return (
    <div className="settings-field">
      <span className="settings-label">Authorization</span>
      <div className="settings-key-field">
        <span className={`mcp-oauth-status${connected ? " mcp-oauth-status-connected" : ""}`}>
          {connected === null ? "Checking…" : connected ? "Connected" : "Not connected"}
        </span>
        {connected ? (
          <button type="button" className="settings-browse-btn" onClick={handleDisconnect} disabled={isBusy}>
            {isBusy ? "Disconnecting…" : "Disconnect"}
          </button>
        ) : (
          <button type="button" className="settings-browse-btn" onClick={handleConnect} disabled={isBusy || !canConnect}>
            {isBusy ? "Opening browser…" : "Connect"}
          </button>
        )}
      </div>
      {!canConnect && <span className="settings-hint">Name and URL are required before connecting.</span>}
      {error && <p className="settings-error-banner">{error}</p>}
    </div>
  );
}

function McpServerCard({
  server,
  onChange,
  onDelete,
}: {
  server: McpServer;
  onChange: (next: McpServer) => void;
  onDelete: () => void;
}) {
  const isHttp = isMcpServerHttp(server);

  function setTransport(next: "stdio" | "http") {
    if (next === (isHttp ? "http" : "stdio")) return;
    onChange(next === "http" ? { name: server.name, url: "", headers: {}, oauth: false } : { name: server.name, command: "", args: [], env: {} });
  }

  function updateArgs(text: string) {
    const args = text
      .split("\n")
      .map((s) => s.trim())
      .filter((s) => s.length > 0);
    onChange({ ...server, args } as McpServer);
  }

  return (
    <div className="provider-card">
      <div className="provider-card-header">
        <input
          className="settings-input provider-name-input"
          type="text"
          placeholder="Name (e.g. notion)"
          value={server.name}
          onChange={(e) => onChange({ ...server, name: e.currentTarget.value })}
        />
        <button
          type="button"
          className="provider-delete-btn"
          onClick={onDelete}
          aria-label={`Delete ${server.name || "this MCP server"}`}
          title="Delete this MCP server"
        >
          🗑
        </button>
      </div>

      <label className="settings-field">
        <span className="settings-label">Transport</span>
        <select className="settings-select" value={isHttp ? "http" : "stdio"} onChange={(e) => setTransport(e.currentTarget.value as "stdio" | "http")}>
          <option value="stdio">Local command (stdio)</option>
          <option value="http">Remote URL (HTTP)</option>
        </select>
      </label>

      {isHttp ? (
        <>
          <label className="settings-field">
            <span className="settings-label">URL</span>
            <input
              className="settings-input"
              type="text"
              placeholder="https://mcp.example.com/mcp"
              value={server.url}
              onChange={(e) => onChange({ ...server, url: e.currentTarget.value })}
            />
          </label>

          <label className="settings-field settings-checkbox-field">
            <span className="settings-checkbox-row">
              <input type="checkbox" checked={server.oauth} onChange={(e) => onChange({ ...server, oauth: e.currentTarget.checked })} />
              <span className="settings-label">Requires OAuth (e.g. Slack)</span>
            </span>
            <span className="settings-hint">
              Discovery, registration, and browser consent instead of a static token — connect below.
            </span>
          </label>

          {server.oauth ? (
            <McpOAuthPanel name={server.name} url={server.url} />
          ) : (
            <KeyValueListField
              label="Headers"
              addLabel="Add header"
              keyPlaceholder="Authorization"
              entries={server.headers}
              onChange={(headers) => onChange({ ...server, headers })}
            />
          )}
        </>
      ) : (
        <>
          <label className="settings-field">
            <span className="settings-label">Command</span>
            <input
              className="settings-input"
              type="text"
              placeholder="npx"
              value={server.command}
              onChange={(e) => onChange({ ...server, command: e.currentTarget.value })}
            />
          </label>

          <label className="settings-field">
            <span className="settings-label">Arguments (one per line)</span>
            <textarea
              className="settings-input settings-textarea"
              rows={3}
              placeholder={"-y\n@notionhq/notion-mcp-server"}
              value={server.args.join("\n")}
              onChange={(e) => updateArgs(e.currentTarget.value)}
            />
          </label>

          <KeyValueListField
            label="Environment variables"
            addLabel="Add variable"
            keyPlaceholder="KEY"
            entries={server.env}
            onChange={(env) => onChange({ ...server, env })}
          />
        </>
      )}
    </div>
  );
}

function SettingsView() {
  const [form, setForm] = useState<Settings>(emptySettings);
  const [isLoading, setIsLoading] = useState(true);
  const [isSaving, setIsSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [savedAt, setSavedAt] = useState<number | null>(null);
  const [toolNames, setToolNames] = useState<string[]>([]);
  const [people, setPeople] = useState<{ id: string; name: string }[]>([]);

  useEffect(() => {
    invoke<string[]>("list_tool_names")
      .then(setToolNames)
      .catch(() => setToolNames([]));
    invoke<{ users: { id: string; name: string }[] }>("list_people")
      .then((p) => setPeople(p.users))
      .catch(() => setPeople([]));
    invoke<Settings>("get_settings")
      .then(setForm)
      .catch((err) => setError(String(err)))
      .finally(() => setIsLoading(false));
  }, []);

  async function handleBrowseVaultPath() {
    const selected = await open({ directory: true, multiple: false });
    if (typeof selected === "string") {
      setForm((f) => ({ ...f, vaultPath: selected }));
    }
  }

  async function handleBrowseGeneratedPath() {
    const selected = await open({ directory: true, multiple: false });
    if (typeof selected === "string") {
      setForm((f) => ({ ...f, generatedPath: selected }));
    }
  }

  function addProvider() {
    setForm((f) => {
      const id = nextProviderId(f.providers);
      const entry: ProviderEntry = { id, kind: "gemini", apiKey: "", baseUrl: "", model: "" };
      return { ...f, providers: [...f.providers, entry], activeProvider: f.activeProvider || id };
    });
  }

  function updateProvider(index: number, next: ProviderEntry) {
    setForm((f) => {
      const prevId = f.providers[index]?.id;
      const providers = f.providers.map((p, i) => (i === index ? next : p));
      if (prevId === undefined || prevId === next.id) {
        return { ...f, providers };
      }
      // Renaming a provider's id (the "Name" field doubles as its id, same scheme as
      // nextProviderId) must carry the rename to whatever else in this form referenced the old
      // id — otherwise "Active" and any agent's default model silently point at a name that no
      // longer exists, and resolving it later fails with a raw "not found" error.
      const activeProvider = f.activeProvider === prevId ? next.id : f.activeProvider;
      const agents = f.agents.map((a) => (a.providerId === prevId ? { ...a, providerId: next.id } : a));
      const combos = f.combos.map((c) => ({ ...c, providers: c.providers.map((id) => (id === prevId ? next.id : id)) }));
      return { ...f, providers, activeProvider, agents, combos };
    });
  }

  function deleteProvider(index: number) {
    setForm((f) => {
      const removed = f.providers[index];
      const providers = f.providers.filter((_, i) => i !== index);
      const activeProvider = f.activeProvider === removed.id ? (providers[0]?.id ?? "") : f.activeProvider;
      // Same dangling-reference risk as the rename case in updateProvider: an agent whose
      // default model was this provider must fall back to "no default" instead of keeping a
      // providerId that no longer resolves to anything.
      const agents = f.agents.map((a) => (a.providerId === removed.id ? { ...a, providerId: "" } : a));
      // A combo loses it too; one left with nothing to try goes, with whatever pointed at it.
      const kept = f.combos.map((c) => ({ ...c, providers: c.providers.filter((id) => id !== removed.id) }));
      const emptied = kept.filter((c) => c.providers.length === 0).map((c) => c.id);
      return {
        ...f,
        providers,
        activeProvider: emptied.includes(activeProvider) ? (providers[0]?.id ?? "") : activeProvider,
        agents: agents.map((a) => (emptied.includes(a.providerId) ? { ...a, providerId: "" } : a)),
        combos: kept.filter((c) => c.providers.length > 0),
      };
    });
  }

  function addCombo() {
    setForm((f) => {
      let n = f.combos.length + 1;
      while (f.combos.some((c) => c.id === `combo-${n}`) || f.providers.some((p) => p.id === `combo-${n}`)) n += 1;
      return { ...f, combos: [...f.combos, { id: `combo-${n}`, providers: [] }] };
    });
  }

  function updateCombo(index: number, next: Combo) {
    setForm((f) => {
      const prevId = f.combos[index]?.id;
      const combos = f.combos.map((c, i) => (i === index ? next : c));
      if (prevId === undefined || prevId === next.id) {
        return { ...f, combos };
      }
      // Same rename cascade as a provider's: the active model and every agent that named it.
      const activeProvider = f.activeProvider === prevId ? next.id : f.activeProvider;
      const agents = f.agents.map((a) => (a.providerId === prevId ? { ...a, providerId: next.id } : a));
      return { ...f, combos, activeProvider, agents };
    });
  }

  function deleteCombo(index: number) {
    setForm((f) => {
      const removed = f.combos[index];
      return {
        ...f,
        combos: f.combos.filter((_, i) => i !== index),
        activeProvider: f.activeProvider === removed.id ? (f.providers[0]?.id ?? "") : f.activeProvider,
        agents: f.agents.map((a) => (a.providerId === removed.id ? { ...a, providerId: "" } : a)),
      };
    });
  }

  function addAgent() {
    setForm((f) => ({
      ...f,
      agents: [
        ...f.agents,
        { id: nextAgentId(f.agents), persona: "", providerId: "", canDelegateToAgents: false, canManageAgents: false, canMessageAgents: false, canManageTasks: false, allowedTools: null, autonomy: 4, approvalRequired: [] },
      ],
    }));
  }

  // An SSH host's "available to" list names agents by id, so renaming or deleting an agent must
  // carry through to it — same dangling-reference care as the provider rename/delete above.
  function updateAgent(index: number, next: AgentEntry) {
    setForm((f) => {
      const prevId = f.agents[index].id;
      const sshHosts =
        prevId === next.id ? f.sshHosts : f.sshHosts.map((h) => ({ ...h, agents: h.agents.map((a) => (a === prevId ? next.id : a)) }));
      // P120: whoever reported to a renamed agent follows its new name.
      const agents = renameInReports(
        f.agents.map((a, i) => (i === index ? next : a)),
        prevId,
        next.id,
      );
      return { ...f, agents, sshHosts };
    });
  }

  function deleteAgent(index: number) {
    setForm((f) => {
      const removed = f.agents[index].id;
      // A host restricted only to the deleted agent must not fall through to "empty list = every
      // agent" — that would silently widen access. Pruning it to nothing switches it off instead.
      const sshHosts = f.sshHosts.map((h) => {
        if (!h.agents.includes(removed)) return h;
        const agents = h.agents.filter((a) => a !== removed);
        return { ...h, agents, enabled: agents.length === 0 ? false : h.enabled };
      });
      // P120: the ones that reported to it report to its superior from now on.
      return { ...f, agents: removeFromOrg(f.agents, removed), sshHosts };
    });
  }

  function addSshHost() {
    setForm((f) => ({
      ...f,
      sshHosts: [...f.sshHosts, { id: nextSshHostId(f.sshHosts), host: "", user: "", port: 22, identityFile: "", enabled: false, agents: [], requireApproval: false }],
    }));
  }

  function updateSshHost(index: number, next: SshHostEntry) {
    setForm((f) => ({ ...f, sshHosts: f.sshHosts.map((h, i) => (i === index ? next : h)) }));
  }

  function deleteSshHost(index: number) {
    setForm((f) => ({ ...f, sshHosts: f.sshHosts.filter((_, i) => i !== index) }));
  }

  function addMcpServer(server: McpServer) {
    setForm((f) => ({ ...f, mcpServers: [...f.mcpServers, server] }));
  }

  function addBlankMcpServer() {
    addMcpServer({ name: "", command: "", args: [], env: {} });
  }

  function addMcpPreset(preset: McpServer) {
    // Deep-clone so editing one card never mutates the shared MCP_PRESETS constant, and dedupe
    // the name against whatever's already in the list (same spirit as nextProviderId).
    const server: McpServer = isMcpServerHttp(preset)
      ? { ...preset, headers: { ...preset.headers } }
      : { ...preset, args: [...preset.args], env: { ...preset.env } };
    let name = server.name;
    let n = 2;
    while (form.mcpServers.some((s) => s.name === name)) {
      name = `${server.name}-${n}`;
      n += 1;
    }
    addMcpServer({ ...server, name });
  }

  function updateMcpServer(index: number, next: McpServer) {
    setForm((f) => ({ ...f, mcpServers: f.mcpServers.map((s, i) => (i === index ? next : s)) }));
  }

  function deleteMcpServer(index: number) {
    setForm((f) => ({ ...f, mcpServers: f.mcpServers.filter((_, i) => i !== index) }));
  }

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    setSavedAt(null);

    const namelessProvider = form.providers.find((p) => p.id.trim() === "");
    if (namelessProvider) {
      setError("Every provider needs a name.");
      return;
    }
    const ids = form.providers.map((p) => p.id.trim());
    const duplicate = ids.find((id, i) => ids.indexOf(id) !== i);
    if (duplicate) {
      setError(`Duplicate provider name: ${duplicate}`);
      return;
    }

    const namelessServer = form.mcpServers.find((s) => s.name.trim() === "");
    if (namelessServer) {
      setError("Every MCP server needs a name.");
      return;
    }
    const incompleteServer = form.mcpServers.find((s) => (isMcpServerHttp(s) ? s.url.trim() === "" : s.command.trim() === ""));
    if (incompleteServer) {
      setError(`MCP server '${incompleteServer.name}' needs a ${isMcpServerHttp(incompleteServer) ? "URL" : "command"}.`);
      return;
    }

    const namelessAgent = form.agents.find((a) => a.id.trim() === "");
    if (namelessAgent) {
      setError("Every agent needs a name.");
      return;
    }
    const agentIds = form.agents.map((a) => a.id.trim());
    const duplicateAgent = agentIds.find((id, i) => agentIds.indexOf(id) !== i);
    if (duplicateAgent) {
      setError(`Duplicate agent name: ${duplicateAgent}`);
      return;
    }

    const spendingError = validateSpending(form.limits, form.prices);
    if (spendingError) {
      setError(spendingError);
      return;
    }

    // Mirrors save_settings's own all-or-nothing check for git_sync — catches it before the IPC
    // round-trip.
    const gitSyncFilled = form.gitSync ? [form.gitSync.remoteUrl, form.gitSync.token].filter((s) => s.trim() !== "").length : 0;
    if (gitSyncFilled === 1) {
      setError("Git sync fields (remote URL and token) must be filled in together, or left entirely blank.");
      return;
    }

    setIsSaving(true);
    try {
      await invoke("save_settings", {
        payload: {
          version: form.version,
          providers: form.providers,
          active_provider: form.activeProvider,
          combos: form.combos,
          vault_path: form.vaultPath,
          generated_path: form.generatedPath,
          tavily_key: form.tavilyKey,
          whisper_key: form.whisperKey,
          enable_shell: form.enableShell,
          mcp_servers: form.mcpServers,
          agents: form.agents,
          ssh_hosts: form.sshHosts,
          git_sync: gitSyncFilled === 2 ? form.gitSync : null,
          // `null` means "no limits written" (the safety net applies), `[]` means "all off" — sent as is.
          limits: form.limits,
          prices: form.prices,
        },
      });
      const refreshed = await invoke<Settings>("get_settings");
      setForm(refreshed);
      setSavedAt(Date.now());
    } catch (err) {
      setError(String(err));
    } finally {
      setIsSaving(false);
    }
  }

  if (isLoading) {
    return (
      <div className="settings-view">
        <p>Loading settings…</p>
      </div>
    );
  }

  return (
    <div className="settings-view">
      <h2 className="settings-title">Settings</h2>
      <form className="settings-form" onSubmit={handleSubmit}>
        <section className="settings-section">
          <div className="settings-section-header">
            <h3 className="settings-section-title">Model providers</h3>
            <button type="button" className="settings-browse-btn" onClick={addProvider}>
              + Add provider
            </button>
          </div>
          {form.providers.length === 0 && (
            <p className="settings-hint">No providers configured yet — add one to start chatting.</p>
          )}
          <div className="provider-list">
            {form.providers.map((p, i) => (
              <ProviderCard
                key={i}
                provider={p}
                isActive={form.activeProvider === p.id && p.id !== ""}
                defaultModel={form.defaultModels[p.kind]}
                onChange={(next) => updateProvider(i, next)}
                onDelete={() => deleteProvider(i)}
                onSetActive={() => setForm((f) => ({ ...f, activeProvider: p.id }))}
              />
            ))}
          </div>
        </section>

        <section className="settings-section">
          <div className="settings-section-header">
            <h3 className="settings-section-title">Combos</h3>
            <button type="button" className="settings-browse-btn" onClick={addCombo}>
              + Add combo
            </button>
          </div>
          <p className="settings-hint">
            A combo is picked like a provider — as the active model, an agent's default or a conversation's model — and
            tries its providers in order: when one is down (busy, rate-limited or unreachable), the next answers, and the
            chat says so above the answer. A rejected key or a bad request never switches.
          </p>
          {form.combos.length === 0 && <p className="settings-hint">No combos yet.</p>}
          <div className="provider-list">
            {form.combos.map((c, i) => (
              <ComboCard
                key={i}
                combo={c}
                providers={form.providers}
                isActive={form.activeProvider === c.id && c.id !== ""}
                onChange={(next) => updateCombo(i, next)}
                onDelete={() => deleteCombo(i)}
                onSetActive={() => setForm((f) => ({ ...f, activeProvider: c.id }))}
              />
            ))}
          </div>
        </section>

        <section className="settings-section">
          <div className="settings-section-header">
            <h3 className="settings-section-title">Agents</h3>
            <button type="button" className="settings-browse-btn" onClick={addAgent}>
              + Add agent
            </button>
          </div>
          <p className="settings-hint">
            Give the agent a name and a personality — every new conversation starts by picking one, alongside the
            model, and sticks with it for good.
          </p>
          {form.agents.length === 0 && (
            <p className="settings-hint">No agents configured yet — add one here before starting a new conversation.</p>
          )}
          <div className="provider-list">
            {form.agents.map((a, i) => (
              <AgentCard key={i} agent={a} allAgents={form.agents} providers={form.providers} combos={form.combos} toolNames={toolNames} people={people} onChange={(next) => updateAgent(i, next)} onDelete={() => deleteAgent(i)} />
            ))}
          </div>
        </section>

        <SpendingSection
          limits={form.limits}
          defaultLimits={form.defaultLimits}
          disabledByEnv={form.limitsDisabledByEnv}
          prices={form.prices}
          agents={form.agents}
          modelSuggestions={[...new Set([...form.providers.map((p) => p.model), ...Object.values(form.defaultModels)].filter((m) => m.trim() !== ""))]}
          onLimitsChange={(limits) => setForm((f) => ({ ...f, limits }))}
          onPricesChange={(prices) => setForm((f) => ({ ...f, prices }))}
        />

        <section className="settings-section">
          <div className="settings-section-header">
            <h3 className="settings-section-title">SSH servers</h3>
            <button type="button" className="settings-browse-btn" onClick={addSshHost}>
              + Add server
            </button>
          </div>
          <p className="settings-hint">
            Servers the AI can run commands on (a VPS, a home machine). Nothing is shared until you switch a server
            on. The server's host key must already be trusted in your ~/.ssh/known_hosts — connect once from a
            terminal first; Warden never accepts an unknown key on its own.
          </p>
          {form.sshHosts.length === 0 && <p className="settings-hint">No servers registered yet.</p>}
          <div className="provider-list">
            {form.sshHosts.map((h, i) => (
              <SshHostCard key={i} host={h} agents={form.agents} onChange={(next) => updateSshHost(i, next)} onDelete={() => deleteSshHost(i)} />
            ))}
          </div>
        </section>

        <section className="settings-section">
          <div className="settings-section-header">
            <h3 className="settings-section-title">MCP servers</h3>
          </div>
          <p className="settings-hint">
            Connect external MCP servers to give the agent access to more tools — pick a preset below to start from a
            known-good command, or add a custom one.
          </p>
          <div className="mcp-quick-add-row">
            {MCP_PRESETS.map((preset) => (
              <button key={preset.label} type="button" className="settings-browse-btn" onClick={() => addMcpPreset(preset.server)}>
                + {preset.label}
              </button>
            ))}
            <button type="button" className="settings-browse-btn" onClick={addBlankMcpServer}>
              + Custom
            </button>
          </div>
          {form.mcpServers.length === 0 && <p className="settings-hint">No MCP servers configured yet.</p>}
          <div className="provider-list">
            {form.mcpServers.map((s, i) => (
              <McpServerCard key={i} server={s} onChange={(next) => updateMcpServer(i, next)} onDelete={() => deleteMcpServer(i)} />
            ))}
          </div>
        </section>

        <section className="settings-section">
          <div className="settings-section-header">
            <h3 className="settings-section-title">Sync via Git</h3>
          </div>
          <p className="settings-hint">
            Your memory (the vault) always lives on this machine; sync keeps it the same on your other devices. A
            self-hosted/remote git repo (Gitea, GitHub, ...) is the alternative to Arweave for that — free, and fully
            automatable (no phone approval). Fill this in to unlock manual push/pull and automatic sync on the Sync
            screen; leaving it blank keeps Arweave (or nothing) as the sync backend.
          </p>
          <GitSyncForm value={form.gitSync} onChange={(gitSync) => setForm((f) => ({ ...f, gitSync }))} />
        </section>

        <label className="settings-field">
          <span className="settings-label">Vault path</span>
          <div className="settings-key-field">
            <input
              className="settings-input"
              type="text"
              placeholder="Default: ~/Warden/vault"
              value={form.vaultPath}
              onChange={(e) => setForm((f) => ({ ...f, vaultPath: e.currentTarget.value }))}
            />
            <button type="button" className="settings-browse-btn" onClick={handleBrowseVaultPath}>
              Browse…
            </button>
          </div>
        </label>

        <label className="settings-field">
          <span className="settings-label">Generated files path</span>
          <p className="settings-hint">
            Where files the model generates (documents, spreadsheets) or oversized media it fetches get saved. Kept
            separate from the vault — these files aren't synced or searched.
          </p>
          <div className="settings-key-field">
            <input
              className="settings-input"
              type="text"
              placeholder="Default: sibling of the vault path (…/generated)"
              value={form.generatedPath}
              onChange={(e) => setForm((f) => ({ ...f, generatedPath: e.currentTarget.value }))}
            />
            <button type="button" className="settings-browse-btn" onClick={handleBrowseGeneratedPath}>
              Browse…
            </button>
          </div>
        </label>

        <ApiKeyField
          label="Tavily API key (web search)"
          value={form.tavilyKey}
          onChange={(v) => setForm((f) => ({ ...f, tavilyKey: v }))}
        />

        <ApiKeyField
          label="OpenAI voice API key (speech-to-text + text-to-speech)"
          value={form.whisperKey}
          onChange={(v) => setForm((f) => ({ ...f, whisperKey: v }))}
        />

        <label className="settings-field settings-checkbox-field">
          <span className="settings-checkbox-row">
            <input
              type="checkbox"
              checked={form.enableShell}
              onChange={(e) => {
                const enableShell = e.currentTarget.checked;
                setForm((f) => ({ ...f, enableShell }));
              }}
            />
            <span className="settings-label">Enable shell tool</span>
          </span>
          <span className="settings-hint">
            Lets the model run any command on this machine, with no sandboxing. Off by default.
          </span>
        </label>

        {error && <p className="settings-error-banner">{error}</p>}
        {savedAt && !error && <p className="settings-success-banner">Settings saved.</p>}

        <button type="submit" className="settings-save-btn" disabled={isSaving}>
          {isSaving ? "Saving…" : "Save settings"}
        </button>
      </form>

      {/* P118 — saved on its own, outside the form above. */}
      <BotsSection />

      {/* P12 — saved on its own, outside the form above. */}
      <ApiKeysSection />
    </div>
  );
}

export default SettingsView;
