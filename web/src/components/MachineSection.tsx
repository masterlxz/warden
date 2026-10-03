import type { MachineSettings } from "../hub/messages";
import { newEntry, newMcp, newSsh, type McpDraft, type McpEntryDraft, type MachineDraft, type SshDraft } from "./machineDraft";
import { Field, SecretField, Section } from "./settingsParts";

// "Máquina do hub" (P119): what reaches the computer the hub runs on. Every control sits in one disabled
// <fieldset> while the hub doesn't let a save change it, so a locked screen can't even look editable.

interface Props {
  draft: MachineDraft;
  /** What the hub sent, for the lock and its reason. */
  settings: MachineSettings;
  agentIds: string[];
  secretsWritable: boolean;
  error: string | null;
  onChange: (change: (m: MachineDraft) => MachineDraft) => void;
}

/** The names and values of an MCP server's env entries or headers. */
function EntryList({
  title,
  entries,
  secretsWritable,
  onChange,
}: {
  title: string;
  entries: McpEntryDraft[];
  secretsWritable: boolean;
  onChange: (entries: McpEntryDraft[]) => void;
}) {
  const patch = (key: number, change: Partial<McpEntryDraft>) => onChange(entries.map((e) => (e.key === key ? { ...e, ...change } : e)));
  return (
    <div className="settings-field settings-field--wide">
      <span className="settings-secret-label">{title}</span>
      {entries.length === 0 && <span className="field-hint">Nenhum.</span>}
      {entries.map((e) => (
        <div key={e.key} className="settings-card">
          <Field label="Nome">
            <input value={e.name} readOnly={e.secret.saved.set} onChange={(ev) => patch(e.key, { name: ev.target.value })} />
          </Field>
          <SecretField label="Valor" noun="valor" placeholder="Cole o valor" value={e.secret} writable={secretsWritable} onChange={(edit) => patch(e.key, { secret: { ...e.secret, edit } })} />
          {!e.secret.saved.set && (
            <button type="button" className="link-button skills-danger" onClick={() => onChange(entries.filter((x) => x.key !== e.key))}>
              Descartar
            </button>
          )}
        </div>
      ))}
      <button type="button" className="link-button settings-add" onClick={() => onChange([...entries, newEntry()])}>
        + Adicionar
      </button>
    </div>
  );
}

function McpCard({ server, secretsWritable, onChange, onRemove }: { server: McpDraft; secretsWritable: boolean; onChange: (change: Partial<McpDraft>) => void; onRemove: () => void }) {
  return (
    <li className="skills-item settings-card">
      <div className="settings-grid">
        <Field label="Nome">
          <input value={server.name} onChange={(e) => onChange({ name: e.target.value })} />
        </Field>
        <Field label="Tipo">
          <select value={server.kind} onChange={(e) => onChange({ kind: e.target.value as McpDraft["kind"] })}>
            <option value="stdio">Processo local (stdio)</option>
            <option value="http">Servidor remoto (http)</option>
          </select>
        </Field>
        {server.kind === "stdio" ? (
          <>
            <Field label="Comando" hint="Roda na máquina do hub, como o usuário do hub." wide>
              <input value={server.command} placeholder="npx" onChange={(e) => onChange({ command: e.target.value })} />
            </Field>
            <Field label="Argumentos" hint="Um por linha." wide>
              <textarea rows={2} value={server.args} onChange={(e) => onChange({ args: e.target.value })} />
            </Field>
            <EntryList title="Variáveis de ambiente" entries={server.env} secretsWritable={secretsWritable} onChange={(env) => onChange({ env })} />
          </>
        ) : (
          <>
            <Field label="Endereço" hint="http:// ou https://" wide>
              <input value={server.url} placeholder="https://mcp.exemplo.com/mcp" onChange={(e) => onChange({ url: e.target.value })} />
            </Field>
            {server.oauth ? (
              <p className="field-hint settings-field--wide">Este servidor entra com OAuth, que só o desktop consegue fazer: o endereço dá para mudar aqui, o login não.</p>
            ) : (
              <EntryList title="Cabeçalhos (ex.: Authorization)" entries={server.headers} secretsWritable={secretsWritable} onChange={(headers) => onChange({ headers })} />
            )}
          </>
        )}
      </div>
      <div className="skills-actions">
        <button type="button" className="link-button skills-danger" onClick={onRemove}>
          Remover servidor MCP
        </button>
      </div>
    </li>
  );
}

function SshCard({ host, agentIds, onChange, onRemove }: { host: SshDraft; agentIds: string[]; onChange: (change: Partial<SshDraft>) => void; onRemove: () => void }) {
  return (
    <li className="skills-item settings-card">
      <div className="settings-grid">
        <Field label="Nome" hint="É como o modelo escolhe o servidor.">
          <input value={host.id} onChange={(e) => onChange({ id: e.target.value })} />
        </Field>
        <Field label="Endereço">
          <input value={host.host} placeholder="servidor.exemplo.com" onChange={(e) => onChange({ host: e.target.value })} />
        </Field>
        <Field label="Usuário">
          <input value={host.user} onChange={(e) => onChange({ user: e.target.value })} />
        </Field>
        <Field label="Porta">
          <input type="number" min={1} max={65535} value={host.port} onChange={(e) => onChange({ port: e.target.value })} />
        </Field>
        <Field label="Arquivo da chave" hint="Só o caminho, no hub. Chave com senha precisa estar no ssh-agent." wide>
          <input value={host.identityFile} placeholder="vazio: ssh-agent e ~/.ssh/config" onChange={(e) => onChange({ identityFile: e.target.value })} />
        </Field>
      </div>
      <div className="settings-checks">
        <label className="settings-check">
          <input type="checkbox" checked={host.enabled} onChange={(e) => onChange({ enabled: e.target.checked })} />
          Ligado (desligado, o modelo nem sabe que existe)
        </label>
        <label className="settings-check">
          <input type="checkbox" checked={host.requireApproval} onChange={(e) => onChange({ requireApproval: e.target.checked })} />
          Pedir aprovação a cada comando (só o desktop e o terminal conseguem perguntar; os outros canais recusam)
        </label>
      </div>
      <div className="settings-field settings-field--wide">
        <span className="settings-secret-label">Quem pode usar</span>
        <span className="field-hint">Nenhum marcado vale para todos os agentes e para toda conversa sem agente (Telegram, WhatsApp, celular).</span>
        <div className="settings-checks">
          {agentIds.map((id) => (
            <label key={id} className="settings-check">
              <input type="checkbox" checked={host.agents.includes(id)} onChange={(e) => onChange({ agents: e.target.checked ? [...host.agents, id] : host.agents.filter((a) => a !== id) })} />
              {id}
            </label>
          ))}
        </div>
      </div>
      <div className="skills-actions">
        <button type="button" className="link-button skills-danger" onClick={onRemove}>
          Remover servidor SSH
        </button>
      </div>
    </li>
  );
}

export default function MachineSection({ draft, settings, agentIds, secretsWritable, error, onChange }: Props) {
  const writable = settings.writable;
  const set = (change: Partial<MachineDraft>) => onChange((m) => ({ ...m, ...change }));
  const patchMcp = (key: number, change: Partial<McpDraft>) => onChange((m) => ({ ...m, mcp: m.mcp.map((s) => (s.key === key ? { ...s, ...change } : s)) }));
  const patchSsh = (key: number, change: Partial<SshDraft>) => onChange((m) => ({ ...m, ssh: m.ssh.map((h) => (h.key === key ? { ...h, ...change } : h)) }));

  return (
    <Section
      title="Máquina do hub"
      hint="O que alcança o computador em que o hub roda: o shell, os servidores MCP que ele inicia, os servidores SSH que ele pode acessar, as pastas e o hub embutido do desktop."
    >
      <p className="banner settings-danger">
        Quem muda isto escolhe o que o modelo pode executar na máquina do hub, com as permissões do usuário do hub e sem isolamento. Só altere com a conexão cifrada (https://) e
        conferindo cada linha: cada mudança fica registrada no log do hub, e o hub pede confirmação antes de salvar.
      </p>
      {!writable && <p className="banner settings-note">Somente leitura: {settings.blockedReason}</p>}
      {error && <p className="error-banner">{error}</p>}

      <fieldset className="settings-machine" disabled={!writable}>
        <div className="settings-checks">
          <label className="settings-check">
            <input type="checkbox" checked={draft.enableShell} onChange={(e) => set({ enableShell: e.target.checked })} />
            Ligar o shell (o modelo roda qualquer comando na máquina do hub, sem isolamento)
          </label>
        </div>

        <div className="settings-grid">
          <Field label="Pasta do cofre" hint="Caminho absoluto no hub. Vazio usa a do hub. Mudar não move os arquivos que já existem." wide>
            <input value={draft.vaultPath} placeholder="vazio: a padrão" onChange={(e) => set({ vaultPath: e.target.value })} />
          </Field>
          <Field label="Pasta dos arquivos gerados" hint="Caminho absoluto no hub. Vazio usa uma pasta ao lado do cofre." wide>
            <input value={draft.generatedPath} placeholder="vazio: ao lado do cofre" onChange={(e) => set({ generatedPath: e.target.value })} />
          </Field>
        </div>

        <h3 className="settings-subheading">Servidores MCP</h3>
        {draft.mcp.length === 0 && <p className="skills-hint">Nenhum servidor MCP.</p>}
        <ul className="skills-list">
          {draft.mcp.map((s) => (
            <McpCard key={s.key} server={s} secretsWritable={secretsWritable} onChange={(change) => patchMcp(s.key, change)} onRemove={() => onChange((m) => ({ ...m, mcp: m.mcp.filter((x) => x.key !== s.key) }))} />
          ))}
        </ul>
        <button type="button" className="link-button settings-add" onClick={() => onChange((m) => ({ ...m, mcp: [...m.mcp, newMcp()] }))}>
          + Servidor MCP
        </button>

        <h3 className="settings-subheading">Servidores SSH</h3>
        <p className="skills-hint">Nada é compartilhado até você ligar um servidor. A chave do servidor precisa já estar no known_hosts do hub.</p>
        {draft.ssh.length === 0 && <p className="skills-hint">Nenhum servidor SSH.</p>}
        <ul className="skills-list">
          {draft.ssh.map((h) => (
            <SshCard key={h.key} host={h} agentIds={agentIds} onChange={(change) => patchSsh(h.key, change)} onRemove={() => onChange((m) => ({ ...m, ssh: m.ssh.filter((x) => x.key !== h.key) }))} />
          ))}
        </ul>
        <button type="button" className="link-button settings-add" onClick={() => onChange((m) => ({ ...m, ssh: [...m.ssh, newSsh()] }))}>
          + Servidor SSH
        </button>

        {draft.embedded && (
          <>
            <h3 className="settings-subheading">Hub embutido do desktop</h3>
            <p className="skills-hint">Vale na próxima vez que o desktop iniciar o hub: nada aqui recarrega um hub que já está rodando. A chave de pareamento dele só muda no desktop.</p>
            <div className="settings-checks">
              <label className="settings-check">
                <input type="checkbox" checked={draft.embedded.enabled} onChange={(e) => onChange((m) => ({ ...m, embedded: m.embedded && { ...m.embedded, enabled: e.target.checked } }))} />
                Iniciar junto com o desktop
              </label>
              <label className="settings-check">
                <input type="checkbox" checked={draft.embedded.webUi} onChange={(e) => onChange((m) => ({ ...m, embedded: m.embedded && { ...m.embedded, webUi: e.target.checked } }))} />
                Servir esta interface web
              </label>
            </div>
            <div className="settings-grid">
              <Field label="Porta">
                <input type="number" min={1} max={65535} value={draft.embedded.port} onChange={(e) => onChange((m) => ({ ...m, embedded: m.embedded && { ...m.embedded, port: e.target.value } }))} />
              </Field>
              <Field label="Nome do hub" hint="Vazio usa o nome da máquina.">
                <input value={draft.embedded.serverName} onChange={(e) => onChange((m) => ({ ...m, embedded: m.embedded && { ...m.embedded, serverName: e.target.value } }))} />
              </Field>
              <Field label="Endereço de escuta" hint="Vazio: todas as interfaces (0.0.0.0). 127.0.0.1: só esta máquina." wide>
                <input value={draft.embedded.listenHost} onChange={(e) => onChange((m) => ({ ...m, embedded: m.embedded && { ...m.embedded, listenHost: e.target.value } }))} />
              </Field>
            </div>
            <div className="settings-checks">
              <label className="settings-check">
                <input type="checkbox" checked={draft.embedded.tailscaleCert} onChange={(e) => onChange((m) => ({ ...m, embedded: m.embedded && { ...m.embedded, tailscaleCert: e.target.checked } }))} />
                HTTPS com o certificado do Tailscale
              </label>
            </div>
            <div className="settings-grid">
              <Field label="Certificado próprio (PEM)" hint="Caminho no hub. Vai junto com a chave privada.">
                <input value={draft.embedded.tlsCert} onChange={(e) => onChange((m) => ({ ...m, embedded: m.embedded && { ...m.embedded, tlsCert: e.target.value } }))} />
              </Field>
              <Field label="Chave privada do certificado" hint="Caminho no hub.">
                <input value={draft.embedded.tlsKey} onChange={(e) => onChange((m) => ({ ...m, embedded: m.embedded && { ...m.embedded, tlsKey: e.target.value } }))} />
              </Field>
              <Field label="Nome do certificado" hint="Só com certificado próprio.">
                <input value={draft.embedded.tlsHost} onChange={(e) => onChange((m) => ({ ...m, embedded: m.embedded && { ...m.embedded, tlsHost: e.target.value } }))} />
              </Field>
            </div>
          </>
        )}
      </fieldset>
    </Section>
  );
}
