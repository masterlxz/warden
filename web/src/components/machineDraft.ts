import type { AdvancedSettings, EmbeddedServer, MachineEdit, MachineSettings, McpServerEdit } from "../hub/messages";
import { KEEP, keyed, type Keyed, type SecretDraft } from "./settingsParts";

// The drafts behind the "Máquina do hub" and "Avançado" sections (P119): what the screen edits, how it
// turns into what the hub is sent, what it refuses before sending, and what it tells the person a save
// will change. The hub checks everything again; these checks only spare a round trip and say it in Portuguese.

/** One env entry or header of an MCP server: its name, and a value the page never sees. */
export type McpEntryDraft = Keyed<{ name: string; secret: SecretDraft }>;

export type McpDraft = Keyed<{
  originalName?: string;
  name: string;
  kind: "stdio" | "http";
  command: string;
  /** One argument per line. */
  args: string;
  url: string;
  env: McpEntryDraft[];
  headers: McpEntryDraft[];
  /** Signs in with OAuth: only the desktop can run that, so the page leaves it as it is. */
  oauth: boolean;
}>;

export type SshDraft = Keyed<{ id: string; host: string; user: string; port: string; identityFile: string; enabled: boolean; agents: string[]; requireApproval: boolean }>;

export interface EmbeddedDraft {
  enabled: boolean;
  port: string;
  listenHost: string;
  serverName: string;
  tailscaleCert: boolean;
  tlsCert: string;
  tlsKey: string;
  tlsHost: string;
  webUi: boolean;
}

export interface MachineDraft {
  enableShell: boolean;
  vaultPath: string;
  generatedPath: string;
  mcp: McpDraft[];
  ssh: SshDraft[];
  /** `null`: the hub has none (it is set up on the desktop), so there is nothing to edit. */
  embedded: EmbeddedDraft | null;
}

function splitLines(text: string): string[] {
  return text
    .split("\n")
    .map((l) => l.trim())
    .filter((l) => l !== "");
}

function entryDraft(name: string): McpEntryDraft {
  return keyed({ name, secret: { saved: { set: true }, edit: KEEP } });
}

/** An entry the person just added: it has no saved value to keep, so it starts on "set". */
export function newEntry(): McpEntryDraft {
  return keyed({ name: "", secret: { saved: { set: false }, edit: { action: "set", value: "" } } });
}

export function newMcp(): McpDraft {
  return keyed({ name: "", kind: "stdio" as const, command: "", args: "", url: "", env: [], headers: [], oauth: false });
}

export function newSsh(): SshDraft {
  // Switched off until the person turns it on, like the desktop's.
  return keyed({ id: "", host: "", user: "", port: "22", identityFile: "", enabled: false, agents: [], requireApproval: false });
}

function embeddedDraft(e: EmbeddedServer): EmbeddedDraft {
  return { ...e, port: String(e.port) };
}

export function toMachineDraft(m: MachineSettings): MachineDraft {
  return {
    enableShell: m.enableShell,
    vaultPath: m.vaultPath,
    generatedPath: m.generatedPath,
    mcp: m.mcpServers.map((s) =>
      keyed({ originalName: s.name, name: s.name, kind: s.kind, command: s.command, args: s.args.join("\n"), url: s.url, env: s.envKeys.map(entryDraft), headers: s.headerKeys.map(entryDraft), oauth: s.oauth }),
    ),
    ssh: m.sshHosts.map((h) => keyed({ ...h, port: String(h.port) })),
    embedded: m.embeddedServer ? embeddedDraft(m.embeddedServer) : null,
  };
}

/** What a save sends for the machine slice. Also how two drafts are compared: only what would be sent counts. */
export function toMachineEdit(d: MachineDraft): MachineEdit {
  const mcpServers: McpServerEdit[] = d.mcp.map((s) => ({
    ...(s.originalName !== undefined && { originalName: s.originalName }),
    name: s.name.trim(),
    kind: s.kind,
    command: s.kind === "stdio" ? s.command : "",
    args: s.kind === "stdio" ? splitLines(s.args) : [],
    env: s.kind === "stdio" ? s.env.map((e) => ({ key: e.name.trim(), value: e.secret.edit })) : [],
    url: s.kind === "http" ? s.url.trim() : "",
    headers: s.kind === "http" ? s.headers.map((e) => ({ key: e.name.trim(), value: e.secret.edit })) : [],
  }));
  return {
    enableShell: d.enableShell,
    vaultPath: d.vaultPath.trim(),
    generatedPath: d.generatedPath.trim(),
    mcpServers,
    sshHosts: d.ssh.map((h) => ({ id: h.id.trim(), host: h.host.trim(), user: h.user.trim(), port: Number(h.port), identityFile: h.identityFile.trim(), enabled: h.enabled, agents: h.agents, requireApproval: h.requireApproval })),
    ...(d.embedded && {
      embeddedServer: {
        enabled: d.embedded.enabled,
        port: Number(d.embedded.port),
        listenHost: d.embedded.listenHost.trim(),
        serverName: d.embedded.serverName.trim(),
        tailscaleCert: d.embedded.tailscaleCert,
        tlsCert: d.embedded.tlsCert.trim(),
        tlsKey: d.embedded.tlsKey.trim(),
        tlsHost: d.embedded.tlsHost.trim(),
        webUi: d.embedded.webUi,
      },
    }),
  };
}

const ABSOLUTE_PATH = /^(\/|[A-Za-z]:[\\/]|\\\\)/;
const PARENT_SEGMENT = /(^|[\\/])\.\.([\\/]|$)/;

/** What stops the machine slice from being sent, in the words the screen shows; `null` when it's fine.
 * `base` is the slice as it was loaded: a folder that was already there is not asked to be absolute again. */
export function machineError(d: MachineDraft, base: MachineEdit): string | null {
  for (const [label, value, was] of [
    ["A pasta do cofre", d.vaultPath.trim(), base.vaultPath],
    ["A pasta dos arquivos gerados", d.generatedPath.trim(), base.generatedPath],
  ] as const) {
    if (value !== "" && value !== was) {
      if (!ABSOLUTE_PATH.test(value)) return `${label} precisa ser um caminho absoluto (é uma pasta na máquina do hub).`;
      if (PARENT_SEGMENT.test(value)) return `${label} não pode ter “..”.`;
    }
  }
  const mcpNames = new Set<string>();
  for (const s of d.mcp) {
    const name = s.name.trim();
    if (name === "") return "Todo servidor MCP precisa de um nome.";
    if (mcpNames.has(name)) return `Dois servidores MCP se chamam “${name}”.`;
    mcpNames.add(name);
    if (s.kind === "stdio" && s.command.trim() === "") return `O servidor MCP “${name}” precisa de um comando.`;
    if (s.kind === "http" && !/^https?:\/\//.test(s.url.trim())) return `O servidor MCP “${name}” precisa de um endereço http:// ou https://.`;
    const entries = s.kind === "stdio" ? s.env : s.headers;
    const what = s.kind === "stdio" ? "variável de ambiente" : "cabeçalho";
    const seen = new Set<string>();
    for (const e of entries) {
      const key = e.name.trim();
      if (key === "") return `O servidor MCP “${name}” tem ${what} sem nome.`;
      if (seen.has(key)) return `O servidor MCP “${name}” repete o ${what} “${key}”.`;
      seen.add(key);
      if (e.secret.edit.action === "set" && e.secret.edit.value.trim() === "") return `Dê um valor ao ${what} “${key}” do servidor MCP “${name}”, ou remova.`;
    }
  }
  const sshIds = new Set<string>();
  for (const h of d.ssh) {
    const id = h.id.trim();
    if (id === "") return "Todo servidor SSH precisa de um nome.";
    if (sshIds.has(id)) return `Dois servidores SSH se chamam “${id}”.`;
    sshIds.add(id);
    if (h.host.trim() === "" || h.user.trim() === "") return `O servidor SSH “${id}” precisa de endereço e usuário.`;
    const port = Number(h.port);
    if (!Number.isInteger(port) || port < 1 || port > 65535) return `A porta do servidor SSH “${id}” vai de 1 a 65535.`;
  }
  if (d.embedded) {
    const port = Number(d.embedded.port);
    if (!Number.isInteger(port) || port < 1 || port > 65535) return "A porta do hub embutido vai de 1 a 65535.";
    if ((d.embedded.tlsCert.trim() === "") !== (d.embedded.tlsKey.trim() === "")) return "No hub embutido, informe o certificado e a chave privada juntos.";
  }
  return null;
}

/** A line per thing a save would change, for the confirmation: what it lets the hub do, never a secret value. */
export function machineSummary(base: MachineEdit, now: MachineEdit): string[] {
  const out: string[] = [];
  if (base.enableShell !== now.enableShell) {
    out.push(now.enableShell ? "Ligar o shell: o modelo poderá rodar qualquer comando na máquina do hub, sem isolamento." : "Desligar o shell.");
  }
  if (base.vaultPath !== now.vaultPath) out.push(`Pasta do cofre: de ${base.vaultPath || "o padrão"} para ${now.vaultPath || "o padrão"}. Os arquivos não são movidos.`);
  if (base.generatedPath !== now.generatedPath) out.push(`Pasta dos arquivos gerados: de ${base.generatedPath || "o padrão"} para ${now.generatedPath || "o padrão"}.`);

  const before = new Map(base.mcpServers.map((s) => [s.originalName ?? s.name, s]));
  for (const s of now.mcpServers) {
    const was = s.originalName !== undefined ? before.get(s.originalName) : undefined;
    const what = s.kind === "stdio" ? `inicia o processo “${s.command}” na máquina do hub` : `conecta em ${s.url}`;
    if (!was) out.push(`Servidor MCP novo “${s.name}”: ${what}.`);
    else if (JSON.stringify(was) !== JSON.stringify(s)) out.push(`Servidor MCP “${s.name}” alterado: ${what}.`);
  }
  const keptMcp = new Set(now.mcpServers.map((s) => s.originalName));
  for (const s of base.mcpServers) if (!keptMcp.has(s.originalName ?? s.name)) out.push(`Servidor MCP “${s.name}” removido.`);

  const sshBefore = new Map(base.sshHosts.map((h) => [h.id, h]));
  for (const h of now.sshHosts) {
    const was = sshBefore.get(h.id);
    const where = `${h.user}@${h.host}:${h.port}${h.enabled ? "" : " (desligado)"}`;
    if (!was) out.push(`Servidor SSH novo “${h.id}”: ${where}. Usa as chaves SSH do hub.`);
    else if (JSON.stringify(was) !== JSON.stringify(h)) out.push(`Servidor SSH “${h.id}” alterado: ${where}.`);
  }
  const sshNow = new Set(now.sshHosts.map((h) => h.id));
  for (const h of base.sshHosts) if (!sshNow.has(h.id)) out.push(`Servidor SSH “${h.id}” removido.`);

  if (JSON.stringify(base.embeddedServer) !== JSON.stringify(now.embeddedServer)) out.push("Hub embutido alterado: vale na próxima vez que o desktop iniciar o hub.");
  return out;
}

export interface AdvancedDraft {
  depth: string;
  calls: string;
  jobs: string;
  network: string;
  rpcUrl: string;
  publicUrl: string;
}

/** The most the web accepts for each delegation setting. The hub enforces the same numbers (`machine_settings.rs`);
 * a value already in the file passes unchanged, and more than this is set in the file by hand. */
export const CEILINGS = { depth: 5, calls: 300, jobs: 10 } as const;

function text(n: number | null): string {
  return n === null ? "" : String(n);
}

export function toAdvancedDraft(a: AdvancedSettings): AdvancedDraft {
  return { depth: text(a.delegateMaxDepth), calls: text(a.maxDelegatedCalls), jobs: text(a.maxParallelJobs), network: a.truthidNetwork, rpcUrl: a.truthidRpcUrl, publicUrl: a.truthidPublicUrl };
}

function numberOrNull(value: string): number | null {
  return value.trim() === "" ? null : Number(value);
}

export function toAdvanced(d: AdvancedDraft): AdvancedSettings {
  return {
    delegateMaxDepth: numberOrNull(d.depth),
    maxDelegatedCalls: numberOrNull(d.calls),
    maxParallelJobs: numberOrNull(d.jobs),
    truthidNetwork: d.network,
    truthidRpcUrl: d.rpcUrl.trim(),
    truthidPublicUrl: d.publicUrl.trim(),
  };
}

/** What stops the advanced block from being sent; `null` when it's fine. A value that didn't change is never questioned. */
export function advancedError(d: AdvancedDraft, base: AdvancedSettings): string | null {
  const now = toAdvanced(d);
  const check = (label: string, value: number | null, was: number | null, min: number, max: number, extra: string): string | null => {
    if (value === null || value === was) return null;
    if (!Number.isInteger(value) || value < min || value > max) return `${label}: um número inteiro de ${min} a ${max}${extra}.`;
    return null;
  };
  return (
    check("Profundidade da delegação", now.delegateMaxDepth, base.delegateMaxDepth, 0, CEILINGS.depth, " (mais que isso, só no config.toml)") ??
    check("Chamadas delegadas por turno", now.maxDelegatedCalls, base.maxDelegatedCalls, 1, CEILINGS.calls, " (0 e mais que isso, só no config.toml)") ??
    check("Jobs em paralelo", now.maxParallelJobs, base.maxParallelJobs, 0, CEILINGS.jobs, " (mais que isso, só no config.toml)") ??
    (now.truthidRpcUrl !== "" && now.truthidRpcUrl !== base.truthidRpcUrl && !/^https?:\/\//.test(now.truthidRpcUrl) ? "O endereço RPC do TruthID começa com http:// ou https://." : null) ??
    (now.truthidPublicUrl !== "" && now.truthidPublicUrl !== base.truthidPublicUrl && !/^https:\/\//.test(now.truthidPublicUrl) ? "O endereço público do hub (TruthID) precisa ser https://." : null)
  );
}
