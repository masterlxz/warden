import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import type { ModelProvider, Settings } from "../types";

const emptySettings: Settings = {
  provider: "gemini",
  model: "",
  vaultPath: "",
  defaultModelGemini: "",
  defaultModelOpenai: "",
  geminiKey: "",
  openaiKey: "",
  tavilyKey: "",
  enableShell: false,
};

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

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    setSavedAt(null);
    setIsSaving(true);
    try {
      await invoke("save_settings", {
        payload: {
          provider: form.provider,
          model: form.model,
          vault_path: form.vaultPath,
          gemini_key: form.geminiKey,
          openai_key: form.openaiKey,
          tavily_key: form.tavilyKey,
          enable_shell: form.enableShell,
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

  const modelPlaceholder = form.provider === "gemini" ? form.defaultModelGemini : form.defaultModelOpenai;

  return (
    <div className="settings-view">
      <h2 className="settings-title">Settings</h2>
      <form className="settings-form" onSubmit={handleSubmit}>
        <label className="settings-field">
          <span className="settings-label">Provider</span>
          <select
            className="settings-select"
            value={form.provider}
            onChange={(e) => setForm((f) => ({ ...f, provider: e.currentTarget.value as ModelProvider }))}
          >
            <option value="gemini">Gemini</option>
            <option value="openai">OpenAI</option>
          </select>
        </label>

        <label className="settings-field">
          <span className="settings-label">Model</span>
          <input
            className="settings-input"
            type="text"
            placeholder={modelPlaceholder ? `Default: ${modelPlaceholder}` : ""}
            value={form.model}
            onChange={(e) => setForm((f) => ({ ...f, model: e.currentTarget.value }))}
          />
        </label>

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
          label="Gemini API key"
          value={form.geminiKey}
          onChange={(v) => setForm((f) => ({ ...f, geminiKey: v }))}
        />
        <ApiKeyField
          label="OpenAI API key"
          value={form.openaiKey}
          onChange={(v) => setForm((f) => ({ ...f, openaiKey: v }))}
        />
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
