import { CEILINGS, type AdvancedDraft } from "./machineDraft";
import { Field, Section } from "./settingsParts";

// "Avançado" (P119): how far the assistant's sub-agents may go, and the TruthID sign-in. None of it
// reaches the machine or holds a secret, so it saves like the rest of the screen.

export default function AdvancedSection({ draft, error, onChange }: { draft: AdvancedDraft; error: string | null; onChange: (change: Partial<AdvancedDraft>) => void }) {
  return (
    <Section
      title="Avançado"
      hint="Até onde os sub-agentes vão, e o login por TruthID. Campo vazio usa o padrão do hub (profundidade 2, 30 chamadas por turno, 3 jobs em paralelo)."
    >
      {error && <p className="error-banner">{error}</p>}
      <p className="banner settings-note">
        Cada nível de delegação e cada chamada a mais é uma chamada ao modelo, e gasta do seu limite. Por isso a web aceita até {CEILINGS.depth} níveis, {CEILINGS.calls} chamadas por turno e{" "}
        {CEILINGS.jobs} jobs em paralelo; mais que isso, ou desligar o teto de chamadas (0), só editando o config.toml. Uma variável de ambiente do hub vence o que está salvo aqui.
      </p>
      <div className="settings-grid">
        <Field label="Profundidade da delegação" hint={`Quantos níveis um sub-agente pode delegar adiante, de 0 a ${CEILINGS.depth}.`}>
          <input type="number" min={0} max={CEILINGS.depth} step={1} value={draft.depth} placeholder="padrão" onChange={(e) => onChange({ depth: e.target.value })} />
        </Field>
        <Field label="Chamadas delegadas por turno" hint={`O teto de chamadas ao modelo dos sub-agentes num turno, de 1 a ${CEILINGS.calls}.`}>
          <input type="number" min={1} max={CEILINGS.calls} step={1} value={draft.calls} placeholder="padrão" onChange={(e) => onChange({ calls: e.target.value })} />
        </Field>
        <Field label="Jobs em paralelo" hint={`Tarefas em segundo plano ao mesmo tempo num turno, de 0 (uma de cada vez) a ${CEILINGS.jobs}.`}>
          <input type="number" min={0} max={CEILINGS.jobs} step={1} value={draft.jobs} placeholder="padrão" onChange={(e) => onChange({ jobs: e.target.value })} />
        </Field>
        <Field label="Rede do TruthID">
          <select value={draft.network} onChange={(e) => onChange({ network: e.target.value })}>
            <option value="base-mainnet">Base (principal)</option>
            <option value="base-sepolia">Base Sepolia (testes)</option>
          </select>
        </Field>
        <Field label="Endereço RPC do TruthID" hint="http:// ou https://. Vazio usa o público da rede." wide>
          <input value={draft.rpcUrl} onChange={(e) => onChange({ rpcUrl: e.target.value })} />
        </Field>
        <Field label="Endereço público deste hub" hint="https://. É para onde o login por TruthID manda a resposta do celular. Vazio desliga esse login." wide>
          <input value={draft.publicUrl} placeholder="https://hub.exemplo.com" onChange={(e) => onChange({ publicUrl: e.target.value })} />
        </Field>
      </div>
    </Section>
  );
}
