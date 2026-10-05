// P102 phase 2 — using a hub from the native screens: thin calls to the `remote_*` Tauri commands (the connection, the
// reconnecting and the matching of replies live in Rust, `warden_server::remote_client`) and the mappers of
// `hubMap.ts`. Everything here asks the hub in use; "this computer" is simply not calling these.

import { invoke } from "@tauri-apps/api/core";
import type {
  Attachment,
  AgentEntry,
  ChatMessage,
  ProjectEntry,
  RunMessage,
  SkillEntry,
  SpendStatus,
  Task,
  TaskInfo,
  TaskList,
  UsageSummary,
  Webhook,
  WebhookAuth,
  WebhookCreated,
  WebhookInfo,
  WebhookList,
} from "../types";
import {
  agentFromHub,
  chatMessage,
  expectReply,
  expectVaultReply,
  messagesFromHistory,
  projectFromHub,
  turnFromReply,
  usageFromReport,
  type HubAgent,
  type HubUsageReport,
  type HubConversationSummary,
  type HubHistoryMessage,
  type HubProject,
  type RemoteStatePayload,
  type TurnExtras,
} from "./hubMap";

import type { DirListing, NodeInfo } from "./workdir";

export { HubTurnError } from "./hubMap";
export type { RemoteState, RemoteStatePayload, HubUser } from "./hubMap";

/** How the first sign-in to a hub is made. Not kept: only the token the hub issues is. */
export type HubCredential = { kind: "key"; key: string } | { kind: "member"; username: string; password: string };

/** What `remote_connect` says when this computer has no token for the hub yet: the person has to sign in. */
export function needsSignIn(error: unknown): boolean {
  return String(error).includes("not signed in");
}

/** Starts using the saved hub. With no credential it uses the token from the last sign-in. */
export function hubConnect(hubId: string, credential?: HubCredential): Promise<void> {
  return invoke("remote_connect", { hubId, credential: credential ?? null });
}

/** Back to this computer; `forget` also drops the token, so the next connection asks for the key or password. */
export function hubDisconnect(forget = false): Promise<void> {
  return invoke("remote_disconnect", { forget });
}

export function hubStatus(): Promise<RemoteStatePayload | null> {
  return invoke("remote_status");
}

async function ask<T extends { type: string }>(message: Record<string, unknown>, ...types: string[]): Promise<T> {
  return expectReply<T>(await invoke<unknown>("remote_request", { message }), ...types);
}

export async function hubListConversations(): Promise<HubConversationSummary[]> {
  const reply = await ask<{ type: string; conversations: HubConversationSummary[] }>({ type: "listConversations" }, "conversationList");
  return reply.conversations;
}

/** A conversation's messages. One the hub doesn't have yet comes back empty. */
export async function hubHistory(conversationId: string): Promise<ChatMessage[]> {
  const reply = await ask<{ type: string; messages: HubHistoryMessage[] }>({ type: "requestHistory", conversationId }, "history");
  return messagesFromHistory(conversationId, reply.messages);
}

export async function hubListProjects(): Promise<ProjectEntry[]> {
  const reply = await ask<{ type: string; projects: HubProject[] }>({ type: "listProjects" }, "projectList");
  return reply.projects.map(projectFromHub);
}

/** The agents this person can use on the hub (a member sees the ones shared with them and their own). */
export async function hubAgents(): Promise<AgentEntry[]> {
  const reply = await ask<{ type: string; settings: { agents: HubAgent[] } }>({ type: "requestSettings" }, "settings");
  return reply.settings.agents.map(agentFromHub);
}

/** The folders inside `path` on the hub's machine, or on a node for a `node:<id>:<path>`; no path starts where the person
 * may (a member sees only the folders the owner allowed). Rejects with the hub's reason (outside what they may see, not a
 * folder, unreadable, a node that is away). */
export async function hubListDirs(path?: string): Promise<DirListing> {
  const reply = await ask<{ type: string } & DirListing>({ type: "listDirs", ...(path ? { path } : {}) }, "dirList");
  return { path: reply.path, ...(reply.parent !== undefined ? { parent: reply.parent } : {}), dirs: reply.dirs };
}

/** The nodes of the hub, to pick a folder on one. Only the owner may ask: a member's hub list already names the node
 * folders the owner gave them. */
export async function hubListNodes(): Promise<NodeInfo[]> {
  const reply = await ask<{ type: string; nodes: NodeInfo[] }>({ type: "listNodes" }, "nodeList");
  return reply.nodes;
}

export async function hubMoveConversation(conversationId: string, projectId: string): Promise<void> {
  await ask({ type: "moveConversation", conversationId, ...(projectId ? { projectId } : {}) }, "conversationOk");
}

/** Runs a turn on the hub. Throws a `HubTurnError` with the hub's words when it answers with an error. */
export async function hubChat(args: {
  content: string;
  attachments: Attachment[];
  conversationId: string;
  agentId: string;
  projectId: string;
  workdir: string;
  creating: boolean;
}): Promise<{ content: string } & TurnExtras> {
  return turnFromReply(await invoke<unknown>("remote_chat", { message: chatMessage(args) }));
}

/** A message that has no reply (`cancelTurn`, `setCodeMode`). */
export function hubSend(message: Record<string, unknown>): Promise<void> {
  return invoke("remote_send", { message });
}

/** The hub's usage and spending, with the day split at this computer's midnight. One report feeds the three parts of the
 * screen, so it is asked once and kept until a limit is extended. */
export function hubUsage(): { summary: () => Promise<UsageSummary>; spend: () => Promise<SpendStatus>; extend: (limitId: string) => Promise<void> } {
  let cached: Promise<ReturnType<typeof usageFromReport>> | null = null;
  const report = () => {
    cached ??= ask<{ type: string; report: HubUsageReport }>({ type: "requestUsage", tzOffsetMinutes: -new Date().getTimezoneOffset() }, "usageReport")
      .then((reply) => usageFromReport(reply.report))
      .catch((err) => {
        cached = null;
        throw err;
      });
    return cached;
  };
  return {
    summary: async () => (await report()).summary,
    spend: async () => (await report()).spend,
    extend: async (limitId) => {
      await ask({ type: "extendLimit", limitId }, "limitExtended");
      cached = null;
    },
  };
}

/** The skills of the hub in use. The hub has no messages for a skill's attached files or for drafting one with a model,
 * so the screen leaves those out on a hub. */
export const hubSkills = {
  async list(): Promise<SkillEntry[]> {
    return (await ask<{ type: string; skills: SkillEntry[] }>({ type: "listSkills" }, "skillList")).skills;
  },
  async save(skill: SkillEntry, overwrite: boolean): Promise<void> {
    await ask({ type: "saveSkill", skill, overwrite }, "skillOk");
  },
  async remove(name: string): Promise<void> {
    await ask({ type: "deleteSkill", name }, "skillOk");
  },
};

/** Asks the person for the hub's pairing key (rejects if they decline). The hub wants it for every change to its tasks and
 * webhooks, and this app never keeps it. */
type KeyAsker = (reason: string) => Promise<string>;

interface HubTaskReply {
  type: string;
  tasks: TaskInfo[];
  runsHere: boolean;
}

const taskList = (reply: HubTaskReply): TaskList => ({ tasks: reply.tasks, runHere: reply.runsHere, hubRunning: true });

/** The scheduled tasks of the hub in use. Listing is open; every change asks for the pairing key. Whether the hub runs them
 * on schedule is its own setting, so the screen shows it and doesn't change it. */
export function hubTasks(askKey: KeyAsker) {
  const change = async (reason: string, message: Record<string, unknown>) =>
    taskList(await ask<HubTaskReply>({ ...message, pairingKey: await askKey(reason) }, "taskList"));
  return {
    async list(): Promise<TaskList> {
      return taskList(await ask<HubTaskReply>({ type: "listTasks" }, "taskList"));
    },
    save: (originalId: string | null, task: Task) => change("Saving this task changes the hub.", { type: "saveTask", ...(originalId ? { originalId } : {}), task }),
    setEnabled: (id: string, enabled: boolean) => change(`${enabled ? "Resuming" : "Pausing"} ${id} changes the hub.`, { type: "setTaskEnabled", id, enabled }),
    remove: (id: string) => change(`Deleting ${id} changes the hub.`, { type: "deleteTask", id }),
    run: (id: string) => change(`Running ${id} now starts a run on the hub.`, { type: "runTask", id }),
    /** The task's conversation is `task-<id>`, which the hub gives to any device. */
    async history(id: string): Promise<RunMessage[]> {
      return (await hubHistory(`task-${id}`)).map(({ role, content, createdAt }) => ({ role, content, createdAt }));
    },
  };
}

interface HubWebhookReply {
  type: string;
  webhooks: WebhookInfo[];
  servesHere: boolean;
}

/** The incoming webhooks of the hub in use (the owner's). `hubUrl` is the address of the hub's web interface, where
 * `/hooks/<id>` is served. Listing is open; every change asks for the pairing key. */
export function hubWebhooks(askKey: KeyAsker, hubUrl: string | undefined) {
  const listOf = (reply: HubWebhookReply): WebhookList => ({ webhooks: reply.webhooks, hubRunning: reply.servesHere, ...(hubUrl ? { hubUrl } : {}) });
  const change = async (reason: string, message: Record<string, unknown>) =>
    listOf(await ask<HubWebhookReply>({ ...message, pairingKey: await askKey(reason) }, "webhookList"));
  return {
    async list(): Promise<WebhookList> {
      return listOf(await ask<HubWebhookReply>({ type: "listWebhooks" }, "webhookList"));
    },
    save: (originalId: string | null, webhook: Webhook) => change("Saving this webhook changes the hub.", { type: "saveWebhook", ...(originalId ? { originalId } : {}), webhook }),
    setEnabled: (id: string, enabled: boolean) => change(`${enabled ? "Resuming" : "Pausing"} ${id} changes the hub.`, { type: "setWebhookEnabled", id, enabled }),
    remove: (id: string) => change(`Deleting ${id} changes the hub.`, { type: "deleteWebhook", id }),
    revoke: (id: string) => change(`Revoking the credential of ${id} changes the hub.`, { type: "revokeWebhookCredential", id }),
    /** The credential comes back once, in this reply, and is not sent again. */
    async makeCredential(id: string): Promise<WebhookCreated> {
      const reply = await ask<HubWebhookReply & { id: string; credential: string; kind: WebhookAuth }>(
        { type: "createWebhookCredential", id, pairingKey: await askKey(`A new credential for ${id} replaces the old one at once.`) },
        "webhookCreated",
      );
      return { id: reply.id, credential: reply.credential, kind: reply.kind, list: listOf(reply) };
    },
    async history(conversation: string): Promise<RunMessage[]> {
      return (await hubHistory(conversation)).map(({ role, content, createdAt }) => ({ role, content, createdAt }));
    },
  };
}

async function askVault<T extends { type: string }>(message: Record<string, unknown>, ...types: string[]): Promise<T> {
  return expectVaultReply<T>(await invoke<unknown>("remote_request", { message }), ...types);
}

/** The vault of the hub in use, with the same five calls `vault_cmds.rs` gives for this computer's. A failure is
 * `{ message, conflict }`, the shape `VaultView` already reads. */
export const hubVault = {
  async list(): Promise<string[]> {
    return (await askVault<{ type: string; files: string[] }>({ type: "listVaultFiles" }, "vaultFileList")).files;
  },
  async read(path: string): Promise<{ content: string; version: string }> {
    const reply = await askVault<{ type: string; content: string; version: string }>({ type: "readVaultNote", path }, "vaultNote");
    return { content: reply.content, version: reply.version };
  },
  /** Creates the note when `expectedVersion` is null. Returns the new version. */
  async save(path: string, content: string, expectedVersion: string | null): Promise<string> {
    const reply = await askVault<{ type: string; version: string }>(
      { type: "saveVaultNote", path, content, ...(expectedVersion ? { expectedVersion } : {}) },
      "vaultSaved",
    );
    return reply.version;
  },
  async remove(path: string, expectedVersion: string): Promise<void> {
    await askVault({ type: "deleteVaultNote", path, expectedVersion }, "vaultOk");
  },
  async search(query: string): Promise<{ path: string; lineNumber: number; line: string }[]> {
    return (await askVault<{ type: string; hits: { path: string; lineNumber: number; line: string }[] }>({ type: "searchVault", query }, "vaultSearchResults")).hits;
  },
};
