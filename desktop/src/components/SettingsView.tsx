import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import type { McpServer, ProviderEntry, ProviderKind, Settings } from "../types";

const emptySettings: Settings = {
  providers: [],
  activeProvider: "",
  vaultPath: "",
  tavilyKey: "",
  enableShell: false,
  defaultModels: {},
  mcpServers: [],
};

const PROVIDER_KIND_OPTIONS: { value: ProviderKind; label: string }[] = [
  { value: "gemini", label: "Gemini" },
  { value: "openai", label: "OpenAI" },
  { value: "anthropic", label: "Anthropic" },
  { value: "openai_compatible", label: "OpenAI-compatible (Ollama, OpenRouter, Groq, ...)" },
];

/** Known-good starting points for popular integrations — researched 2026-08-29 (Sessão 35; see
 * ARCHITECTURE.md for why GitHub goes through Docker and Slack isn't offered at all here). Env
 * values are left blank on purpose — the user fills in their own secret after picking a preset. */
const MCP_PRESETS: { label: string; server: McpServer }[] = [
  { label: "Filesystem", server: { name: "filesystem", command: "npx", args: ["-y", "@modelcontextprotocol/server-filesystem", "/path/to/allowed/dir"], env: {} } },
  { label: "Google Workspace", server: { name: "google_workspace", command: "npx", args: ["-y", "@aaronsb/google-workspace-mcp"], env: { GOOGLE_CLIENT_ID: "", GOOGLE_CLIENT_SECRET: "" } } },
  { label: "Notion", server: { name: "notion", command: "npx", args: ["-y", "@notionhq/notion-mcp-server"], env: { NOTION_TOKEN: "" } } },
  {
    label: "GitHub (via Docker)",
    server: { name: "github", command: "docker", args: ["run", "-i", "--rm", "-e", "GITHUB_PERSONAL_ACCESS_TOKEN", "ghcr.io/github/github-mcp-server"], env: { GITHUB_PERSONAL_ACCESS_TOKEN: "" } },
  },
];

function ApiKeyField({
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

      <ApiKeyField
        label={isOpenAiCompatible ? "API key (optional — most local servers don't need one)" : "API key"}
        value={provider.apiKey}
        onChange={(v) => onChange({ ...provider, apiKey: v })}
      />

      <label className="settings-field">
        <span className="settings-label">Model</span>
        <input
          className="settings-input"
          type="text"
          placeholder={defaultModel ? `Default: ${defaultModel}` : isOpenAiCompatible ? "Required, e.g. llama3.1" : ""}
          value={provider.model}
          onChange={(e) => onChange({ ...provider, model: e.currentTarget.value })}
        />
      </label>
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
  const envEntries = Object.entries(server.env);

  function updateArgs(text: string) {
    const args = text
      .split("\n")
      .map((s) => s.trim())
      .filter((s) => s.length > 0);
    onChange({ ...server, args });
  }

  function updateEnvEntry(index: number, key: string, value: string) {
    const entries = envEntries.map((entry, i) => (i === index ? ([key, value] as [string, string]) : entry));
    onChange({ ...server, env: Object.fromEntries(entries) });
  }

  function addEnvVar() {
    // A blank key is a valid (if temporary) React list item — it's the value being edited that
    // matters; a truly empty key just gets dropped on save the same way an empty provider name
    // would be, so nothing needs deduping here.
    onChange({ ...server, env: { ...server.env, "": "" } });
  }

  function removeEnvVar(index: number) {
    onChange({ ...server, env: Object.fromEntries(envEntries.filter((_, i) => i !== index)) });
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

      <div className="settings-field">
        <span className="settings-label">Environment variables</span>
        {envEntries.map(([key, value], i) => (
          <div className="env-var-row" key={i}>
            <input
              className="settings-input"
              type="text"
              placeholder="KEY"
              value={key}
              onChange={(e) => updateEnvEntry(i, e.currentTarget.value, value)}
            />
            <input
              className="settings-input"
              type="password"
              placeholder="value"
              value={value}
              onChange={(e) => updateEnvEntry(i, key, e.currentTarget.value)}
            />
            <button type="button" className="provider-delete-btn" onClick={() => removeEnvVar(i)} aria-label={`Remove ${key || "this variable"}`}>
              ✕
            </button>
          </div>
        ))}
        <button type="button" className="settings-browse-btn" onClick={addEnvVar}>
          + Add variable
        </button>
      </div>
    </div>
  );
}

function SettingsView() {
  const [form, setForm] = useState<Settings>(emptySettings);
  const [isLoading, setIsLoading] = useState(true);
  const [isSaving, setIsSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [savedAt, setSavedAt] = useState<number | null>(null);

  useEffect(() => {
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

  function addProvider() {
    setForm((f) => {
      const id = nextProviderId(f.providers);
      const entry: ProviderEntry = { id, kind: "gemini", apiKey: "", baseUrl: "", model: "" };
      return { ...f, providers: [...f.providers, entry], activeProvider: f.activeProvider || id };
    });
  }

  function updateProvider(index: number, next: ProviderEntry) {
    setForm((f) => ({ ...f, providers: f.providers.map((p, i) => (i === index ? next : p)) }));
  }

  function deleteProvider(index: number) {
    setForm((f) => {
      const removed = f.providers[index];
      const providers = f.providers.filter((_, i) => i !== index);
      const activeProvider = f.activeProvider === removed.id ? (providers[0]?.id ?? "") : f.activeProvider;
      return { ...f, providers, activeProvider };
    });
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
    const server: McpServer = { ...preset, args: [...preset.args], env: { ...preset.env } };
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
    const commandlessServer = form.mcpServers.find((s) => s.command.trim() === "");
    if (commandlessServer) {
      setError(`MCP server '${commandlessServer.name}' needs a command.`);
      return;
    }

    setIsSaving(true);
    try {
      await invoke("save_settings", {
        payload: {
          providers: form.providers,
          active_provider: form.activeProvider,
          vault_path: form.vaultPath,
          tavily_key: form.tavilyKey,
          enable_shell: form.enableShell,
          mcp_servers: form.mcpServers,
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

        <ApiKeyField
          label="Tavily API key (web search)"
          value={form.tavilyKey}
          onChange={(v) => setForm((f) => ({ ...f, tavilyKey: v }))}
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
    </div>
  );
}

export default SettingsView;
