/**
 * Mirrors `crates/warden-server-protocol/src/protocol.rs` — now including the tool-call variants
 * (Fase 8.3-8.6), since `Hello.tools` is no longer always empty (see `background/tools/index.ts`).
 * Wire shape: internally-tagged JSON with a `type` field, both the tag and every field name
 * camelCase (`#[serde(tag = "type", rename_all = "camelCase", rename_all_fields =
 * "camelCase")]` on the Rust side) — locked by `protocol.rs`'s own round-trip tests, not guessed.
 */

export interface Usage {
  promptTokens: number;
  completionTokens: number;
  totalTokens: number;
}

export interface Attachment {
  mimeType: string;
  data: string;
}

/** Mirrors `warden_core::tool::ToolSpec` — `parameters` is a raw JSON-Schema object, not a
 * `inputSchema` wrapper. */
export interface ToolSpec {
  name: string;
  description: string;
  parameters: unknown;
}

/** Mirrors `warden_server_protocol::protocol::SkillDto` (P72). `agents` is the agent restriction
 * (empty = every agent) — this client only displays it; an edit that sends `[]` keeps whatever the
 * server has stored. */
export interface SkillDto {
  name: string;
  description: string;
  body: string;
  agents: string[];
  /** P104 — a suggestion the assistant made, still pending. Sending a save without it accepts the skill. */
  proposed?: boolean;
  source?: string;
  proposedAt?: number;
  /** P115: the suggestion is a change to this existing skill; accepting applies it there. */
  revises?: string;
}

/** Mirrors `warden_server_protocol::protocol::ConversationSummary` (P78). */
export interface ConversationSummary {
  id: string;
  title: string;
  createdAt: number;
  updatedAt: number;
  /** The agent this conversation last spoke with (P46/P87), restored when it's opened. */
  agentId?: string;
  /** The folder the conversation works in (P102): a path of the hub's machine, or `node:<id>:<path>` on a node. */
  workdir?: string;
  /** P125 — set on a thread: the conversation and the message it came from. */
  parent?: ThreadParent;
  /** P125 — for a thread, how many messages the person sent in it. */
  replies?: number;
}

/** Mirrors `ThreadParentDto` (P125). */
export interface ThreadParent {
  conversationId: string;
  messageId: string;
}

/** Mirrors `DirEntryDto` (P102): a folder in the folder browser. */
export interface DirEntry {
  name: string;
  path: string;
}

/** What the folder browser shows (`dirList`): `path` is "" for a member's list of allowed folders, `parent` absent at the top. */
export interface DirListing {
  path: string;
  parent?: string;
  dirs: DirEntry[];
}

/** A tool in this device's turn needs the person's yes (P87) — mirrors `ServerMessage::ApprovalRequest`. */
export interface ApprovalPrompt {
  approvalId: number;
  target: string;
  action: string;
  detail: string;
  /** P122 — the kind of action this agent has to get approved (`critical_infra`...), when the ask comes from that rule. */
  category?: string;
}

/** Onde uma tarefa delegada está (P123). */
export type AgentTaskState = "pending" | "running" | "waiting" | "paused" | "done" | "failed" | "cancelled";

/** O que dá para fazer com uma tarefa em andamento, pela tela. */
export type AgentTaskAction = "pause" | "resume" | "cancel";

/** Mirrors `ActivityEventDto` (P121): algo que aconteceu entre os agentes, no feed de atividade. `kind`: `delegated`, `started`, `done`, `failed`,
 * `cancelled` (as tarefas, com `taskId`), `note`, `reply` (agentes escrevendo uns aos outros) ou `messaged_user` (o agente escrevendo primeiro para
 * a pessoa); os três últimos trazem a `conversationId`. `actor` vazio é o assistente que atende sem agente escolhido. */
export interface ActivityEvent {
  id: string;
  atMs: number;
  kind: string;
  actor: string;
  target?: string;
  text: string;
  taskId?: string;
  conversationId?: string;
}

/** Mirrors `AgentTaskDto` (P123): uma tarefa que um agente delegou em segundo plano. `group` é comum às tarefas que um turno começou. */
export interface AgentTask {
  id: string;
  group: string;
  /** O agente que delegou. */
  owner?: string | null;
  /** Quem faz o trabalho, ou o nome dado a um ajudante temporário. */
  assignee: string;
  /** A tarefa de que esta é subtarefa, quando o agente dela a começou de dentro de outra tarefa. */
  parentId?: string | null;
  objective: string;
  model?: string | null;
  channel: string;
  state: AgentTaskState;
  result?: string | null;
  error?: string | null;
  promptTokens?: number | null;
  completionTokens?: number | null;
  totalTokens?: number | null;
  createdAtMs: number;
  startedAtMs?: number | null;
  finishedAtMs?: number | null;
  /** Roda no processo do hub que respondeu: dá para pausar, retomar ou parar por aqui. */
  controllable?: boolean;
  /** Entre essas, as que também podem ser pausadas: uma delegação que o agente espera só pode ser parada. */
  pausable?: boolean;
}

/** O que a tela de Organização lê de um agente nas configurações do hub (P120, P123): o resto do `AgentSettings` fica de fora. */
export interface AgentInfo {
  id: string;
  role?: string | null;
  reportsTo?: string | null;
  canDelegateToAgents: boolean;
  canManageAgents: boolean;
  canMessageAgents: boolean;
  canManageTasks: boolean;
  /** 1 só responde, 2 sugere, 3 pede antes, 4 age sozinho, 5 também gerencia os subordinados sem perguntar. */
  autonomy: number;
  approvalRequired: string[];
  /** Os modelos que o agente pode escolher ao delegar; vazio = aberto. */
  delegationModels: string[];
}

/** Mirrors `ModelPolicyDto` (P123): um nome ("fast", "reasoning"...) que um agente que delega usa para o modelo de uma tarefa. */
export interface ModelPolicy {
  id: string;
  model: string;
  description?: string;
}

/** Uma mudança feita pela árvore, como o hub a recebe (`AgentOrgEdit`). `role` e `reportsTo` ausentes querem dizer nenhum. */
export type OrgEdit =
  | { kind: "setPosition"; id: string; role?: string; reportsTo?: string }
  | { kind: "addReport"; id: string; persona: string; role?: string; reportsTo?: string }
  | { kind: "remove"; id: string }
  /** P123 — the models `id` may pick when it delegates, in order (the first is the default; empty leaves the choice open). */
  | { kind: "setDelegationModels"; id: string; models: string[] }
  /** P123 — replaces the named model policies with this list. */
  | { kind: "setModelPolicies"; policies: Array<{ id: string; model: string; description: string }> };

/** What the hub's settings say about the agents (P87, P120, P123), reduced to what this client shows. */
export interface HubAgents {
  agents: AgentInfo[];
  modelPolicies: ModelPolicy[];
  /** The ids of the providers and combos: what a policy can be answered by, and what an agent can be limited to besides the policies. */
  modelIds: string[];
}

/** Mirrors `warden_server_protocol::protocol::HistoryMessage` (P40). */
export interface HistoryMessage {
  /** P125 — what a thread is attached to; absent on a hub from before threads. */
  id?: string;
  role: "user" | "assistant";
  content: string;
  createdAt: number;
  attachments: Attachment[];
}

export type ClientMessage =
  /** `authKey` is the hub's pairing key (P36), only needed until this device holds a
   * `deviceToken` from an earlier `helloAck`. */
  | { type: "hello"; deviceId: string; deviceName: string; authKey: string; deviceToken?: string; tools: ToolSpec[] }
  | { type: "ping"; nonce: number }
  /** `conversationId` (P78) picks one of this device's conversations — a new id starts a new one;
   * omitted, the turn goes to the device's default conversation. */
  | { type: "chat"; message: string; conversationId?: string; agentId?: string; workdir?: string; threadOf?: ThreadParent }
  /** P102 — the folders inside `path` on the hub's machine (or a node's, `node:<id>:<path>`); no `path`: where the person starts. Answered by `dirList`/`dirError`. */
  | { type: "listDirs"; requestId: number; path?: string }
  /** P87 — only for the configured agents' ids, as the web's selector does. */
  | { type: "requestSettings"; requestId: number }
  | { type: "resolveApproval"; approvalId: number; approved: boolean }
  /** P123 — the work agents delegated to each other, answered by `agentTaskList`. */
  | { type: "listAgentTasks"; requestId: number }
  /** P121 — the feed of activity, answered by `activityList`. Owner only; a member gets it empty. */
  | { type: "listActivity"; requestId: number }
  /** P123 — pauses, resumes or stops a task running on the hub (and the subtasks below it). Asks for the pairing key; answered by
   * `agentTaskList` or `taskError`. */
  | { type: "controlAgentTask"; requestId: number; pairingKey: string; taskId: string; action: AgentTaskAction }
  /** P120 — one change to the agents' organization. Asks for the pairing key; answered by `settingsSaved` or `settingsError`. */
  | { type: "editAgentOrg"; requestId: number; pairingKey: string; edit: OrgEdit }
  | { type: "toolCallResult"; callId: number; result: unknown }
  | { type: "toolCallError"; callId: number; message: string }
  /** Skills management (P72) — `requestId` is echoed on the matching reply. */
  | { type: "listSkills"; requestId: number }
  | { type: "saveSkill"; requestId: number; skill: SkillDto; overwrite: boolean }
  | { type: "deleteSkill"; requestId: number; name: string }
  /** P40 — this device's persisted conversation, answered by `history`/`historyError` with the same
   * `requestId`. `limit` keeps only the most recent messages. */
  | { type: "requestHistory"; requestId: number; limit?: number; conversationId?: string }
  /** P78 — this device's conversations, answered by `conversationList`/`conversationOk`/`conversationError`. */
  | { type: "listConversations"; requestId: number }
  /** P121 — the id of an agent's channel, the one conversation it keeps with the person; answered by `agentChannel`/`conversationError`. */
  | { type: "openAgentChannel"; requestId: number; agentId: string }
  | { type: "renameConversation"; requestId: number; conversationId: string; title: string }
  | { type: "deleteConversation"; requestId: number; conversationId: string }
  /** Fase 9.1 (redefined) — an unauthenticated presence probe, answered by `discoverAck` below.
   * No `authKey`/`deviceId` on purpose: the point is finding a hub before knowing its credential. */
  | { type: "discover" }
  | { type: "goodbye"; reason: string | null };

export function encode(message: ClientMessage): string {
  return JSON.stringify(message);
}

export type ServerMessage =
  /** `deviceToken` is present when this Hello paired with the pairing key (P36) — it replaces
   * whatever token this device held for the hub. */
  | { type: "helloAck"; serverName: string; deviceToken?: string }
  | { type: "authError"; reason: string }
  | { type: "pong"; nonce: number }
  /** `conversationId` (P78) — which conversation this answers; `chat` has no `requestId`. */
  | { type: "chatResponse"; content: string; usage: Usage | null; attachments: Attachment[]; conversationId?: string }
  | { type: "chatError"; message: string; conversationId?: string }
  | { type: "toolCallRequest"; callId: number; tool: string; arguments: unknown }
  | { type: "skillList"; requestId: number; skills: SkillDto[] }
  | { type: "skillOk"; requestId: number }
  | { type: "skillError"; requestId: number; message: string }
  | { type: "history"; requestId: number; messages: HistoryMessage[] }
  | { type: "historyError"; requestId: number; message: string }
  | { type: "conversationList"; requestId: number; conversations: ConversationSummary[] }
  | { type: "conversationOk"; requestId: number }
  | { type: "agentChannel"; requestId: number; conversationId: string }
  | { type: "conversationError"; requestId: number; message: string }
  | { type: "dirList"; requestId: number; path: string; parent?: string; dirs: DirEntry[] }
  | { type: "dirError"; requestId: number; message: string }
  /** Reply to `requestSettings`, reduced to what this client uses (P87). */
  | ({ type: "settings"; requestId: number; agentIds: string[] } & HubAgents)
  /** Reply to a successful `editAgentOrg`, with the agents as the file holds them now. */
  | ({ type: "settingsSaved"; requestId: number } & HubAgents)
  /** `authRejected`: the pairing key was wrong; nothing was written. */
  | { type: "settingsError"; requestId: number; message: string; authRejected?: boolean }
  | { type: "agentTaskList"; requestId: number; tasks: AgentTask[] }
  | { type: "activityList"; requestId: number; events: ActivityEvent[] }
  | { type: "taskError"; requestId: number; message: string; authRejected?: boolean }
  | ({ type: "approvalRequest" } & ApprovalPrompt)
  | { type: "approvalCancelled"; approvalId: number }
  /** An agent left a note in one of this device's conversations, or answered one (P46 `message_agent`). */
  | { type: "conversationsChanged"; conversationId: string }
  /** Reply to `ClientMessage.discover` — just enough to let the operator recognize which machine
   * this is, never a secret. */
  /** `secureUrl` — set by a TLS-only hub (P36): the wss:// URL to connect to instead. */
  | { type: "discoverAck"; serverName: string; secureUrl?: string }
  | { type: "goodbye"; reason: string | null };

type RawAgent = Partial<Omit<AgentInfo, "id">> & { id: string };
interface RawSettings {
  agents?: RawAgent[];
  modelPolicies?: ModelPolicy[];
  providers?: Array<{ id: string }>;
  combos?: Array<{ id: string }>;
}

/** The agents and model policies out of a settings payload, with the optional fields this client reads filled in. */
function hubAgents(settings: RawSettings): HubAgents {
  return {
    agents: (settings.agents ?? []).map((a) => ({
      id: a.id,
      role: a.role ?? null,
      reportsTo: a.reportsTo ?? null,
      canDelegateToAgents: a.canDelegateToAgents ?? false,
      canManageAgents: a.canManageAgents ?? false,
      canMessageAgents: a.canMessageAgents ?? false,
      canManageTasks: a.canManageTasks ?? false,
      autonomy: a.autonomy ?? 4,
      approvalRequired: a.approvalRequired ?? [],
      delegationModels: a.delegationModels ?? [],
    })),
    modelPolicies: settings.modelPolicies ?? [],
    modelIds: [...(settings.providers ?? []), ...(settings.combos ?? [])].map((m) => m.id),
  };
}

/**
 * Decodes one `ServerMessage`. Throws on anything this client doesn't understand — explicit is
 * better than a silent `as` cast producing a message shape this client can't actually handle.
 */
export function decode(text: string): ServerMessage {
  const json = JSON.parse(text) as { type?: unknown };
  switch (json.type) {
    case "helloAck":
    case "authError":
    case "pong":
    case "chatError":
    case "discoverAck":
    case "goodbye":
      return json as ServerMessage;
    case "chatResponse": {
      const raw = json as { content: string; usage: Usage | null; attachments?: Attachment[]; conversationId?: string };
      return { type: "chatResponse", content: raw.content, usage: raw.usage, attachments: raw.attachments ?? [], conversationId: raw.conversationId };
    }
    case "skillList": {
      const raw = json as { requestId: number; skills: Array<Omit<SkillDto, "agents"> & { agents?: string[] }> };
      return { type: "skillList", requestId: raw.requestId, skills: raw.skills.map((skill) => ({ ...skill, agents: skill.agents ?? [] })) };
    }
    case "settings": {
      const raw = json as { requestId: number; settings: RawSettings };
      return { type: "settings", requestId: raw.requestId, agentIds: (raw.settings.agents ?? []).map((a) => a.id), ...hubAgents(raw.settings) };
    }
    case "settingsSaved": {
      const raw = json as { requestId: number; settings: RawSettings };
      return { type: "settingsSaved", requestId: raw.requestId, ...hubAgents(raw.settings) };
    }
    case "agentTaskList":
    case "activityList":
    case "taskError":
    case "settingsError":
    case "approvalRequest":
    case "approvalCancelled":
    case "conversationsChanged":
    case "skillOk":
    case "skillError":
    case "historyError":
    case "conversationList":
    case "conversationOk":
    case "agentChannel":
    case "conversationError":
    case "dirList":
    case "dirError":
      return json as ServerMessage;
    case "history": {
      const raw = json as { requestId: number; messages: Array<Omit<HistoryMessage, "attachments"> & { attachments?: Attachment[] }> };
      return {
        type: "history",
        requestId: raw.requestId,
        messages: raw.messages.map((m) => ({ ...m, attachments: m.attachments ?? [] })),
      };
    }
    case "toolCallRequest": {
      const raw = json as { callId: number; tool: string; arguments: unknown };
      return { type: "toolCallRequest", callId: raw.callId, tool: raw.tool, arguments: raw.arguments };
    }
    default:
      throw new Error(`unknown or unsupported ServerMessage type: ${String(json.type)}`);
  }
}
