import { useState } from "react";

interface Props {
  /** The code, as the hub made it (`ABCD-EFGH-…`). It's only ever shown here, once. */
  code: string;
  /** `true` when it replaces an earlier code, which no longer works. */
  replacing?: boolean;
  onDone: () => void;
}

/** P84 fatia 4: the recovery code of a member's encrypted data, shown once. It's the only way back to
 * the data if the password is lost or the owner resets it, so the screen doesn't go away until the
 * person says they wrote it down. */
export default function RecoveryCodeView({ code, replacing = false, onDone }: Props) {
  const [saved, setSaved] = useState(false);
  const [copied, setCopied] = useState(false);

  async function copy() {
    try {
      await navigator.clipboard.writeText(code);
      setCopied(true);
    } catch {
      setCopied(false); // no clipboard here: the code is on screen to write down
    }
  }

  return (
    <div className="login">
      <div className="login-card">
        <div className="login-brand">
          <img src="/favicon.svg" alt="" width={40} height={40} />
          <h1>Warden</h1>
        </div>
        <p className="login-hint">
          {replacing ? "Este é o seu novo código de recuperação. O anterior deixou de funcionar." : "Os seus dados neste hub agora ficam criptografados."} Guarde este código: ele é
          a única forma de recuperar tudo se você esquecer a senha, ou se quem administra o hub redefinir a sua senha.
        </p>
        <p className="recovery-code" aria-label="Código de recuperação">
          <code>{code}</code>
        </p>
        <button type="button" className="link-button" onClick={() => void copy()}>
          {copied ? "Copiado" : "Copiar"}
        </button>
        <p className="login-hint">
          O código aparece só desta vez, e ninguém no hub consegue vê-lo depois — nem quem o administra. Anote num lugar seguro, fora deste aparelho.
        </p>
        <label className="checkbox-row">
          <input type="checkbox" checked={saved} onChange={(e) => setSaved(e.target.checked)} />
          Guardei o código num lugar seguro
        </label>
        <button type="button" className="primary-button" disabled={!saved} onClick={onDone}>
          Continuar
        </button>
      </div>
    </div>
  );
}
