import type { ReactNode } from "react";
import type { SecretEdit, SecretStatus } from "../hub/messages";

// The pieces the settings screen is built from, shared by its sections (P78, P119).

export type Keyed<T> = T & { key: number };

/** A saved secret as the draft holds it: whether one is saved, and what the next save does to it. */
export interface SecretDraft {
  saved: SecretStatus;
  edit: SecretEdit;
}

let nextKey = 1;
export function keyed<T>(value: T): Keyed<T> {
  return { ...value, key: nextKey++ };
}

export function strip<T>({ key: _key, ...rest }: Keyed<T>): T {
  return rest as T;
}

export const KEEP: SecretEdit = { action: "keep" };

export function Section({ title, hint, action, children }: { title: string; hint?: string; action?: ReactNode; children: ReactNode }) {
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

export function Field({ label, hint, children, wide }: { label: string; hint?: string; children: ReactNode; wide?: boolean }) {
  return (
    <label className={wide ? "settings-field settings-field--wide" : "settings-field"}>
      {label}
      {children}
      {hint && <span className="field-hint">{hint}</span>}
    </label>
  );
}

/** A saved secret the page never sees: shows whether one is there and lets it be replaced or removed. */
export function SecretField({
  label,
  value,
  writable,
  onChange,
  placeholder = "Cole a chave",
  noun = "chave",
}: {
  label: string;
  value: SecretDraft;
  writable: boolean;
  onChange: (edit: SecretEdit) => void;
  placeholder?: string;
  /** What the secret is called in the status line: an API key is a "chave", an MCP entry a "valor". */
  noun?: "chave" | "valor";
}) {
  const { saved, edit } = value;
  const [fresh, removed, savedText, none] = noun === "chave" ? ["Nova chave (ainda não salva)", "Será removida ao salvar", "Salva", "Nenhuma"] : ["Novo valor (ainda não salvo)", "Será removido ao salvar", "Salvo", "Nenhum"];
  let status: string;
  if (edit.action === "set") status = fresh;
  else if (edit.action === "clear") status = removed;
  else if (saved.set) status = saved.hint ? `${savedText}, termina em …${saved.hint}` : savedText;
  else status = none;

  return (
    <div className="settings-field settings-field--wide">
      <span className="settings-secret-label">{label}</span>
      {edit.action === "set" ? (
        <input type="password" autoComplete="new-password" placeholder={placeholder} value={edit.value} onChange={(e) => onChange({ action: "set", value: e.target.value })} />
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
