import { useState, type ReactNode } from "react";

/** Pede a chave de pareamento do hub para uma mudança (P120, P123): ela é pedida a cada mudança e não fica guardada em lugar nenhum,
 * nem aqui, nem no background. `error` é o recado de uma chave errada. */
export default function PairingKeyForm({
  children,
  busy,
  error,
  onConfirm,
  onCancel,
}: {
  children: ReactNode;
  busy: boolean;
  error: string | null;
  onConfirm: (pairingKey: string) => void;
  onCancel: () => void;
}) {
  const [pairingKey, setPairingKey] = useState("");
  return (
    <form
      className="connection-form pairing-key-form"
      onSubmit={(e) => {
        e.preventDefault();
        onConfirm(pairingKey);
      }}
    >
      <p className="skills-hint">{children}</p>
      <label>
        Chave de pareamento do hub
        <input type="password" autoComplete="current-password" autoFocus value={pairingKey} onChange={(e) => setPairingKey(e.target.value)} />
      </label>
      {error && <p className="error-banner">{error}</p>}
      <div className="skills-actions">
        <button type="submit" disabled={busy || pairingKey.trim() === ""}>
          {busy ? "Aguarde…" : "Confirmar"}
        </button>
        <button type="button" className="link-button" disabled={busy} onClick={onCancel}>
          Cancelar
        </button>
      </div>
    </form>
  );
}
