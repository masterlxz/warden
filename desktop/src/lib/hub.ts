// P102 phase 2 — using a hub from the native screens: thin calls to the `remote_*` Tauri commands (the connection, the
// reconnecting and the matching of replies live in Rust, `warden_server::remote_client`) and the mappers of
// `hubMap.ts`. Everything here asks the hub in use; "this computer" is simply not calling these.

import { invoke } from "@tauri-apps/api/core";
import type { Attachment, AgentEntry, ChatMessage, ProjectEntry } from "../types";
import {
  agentFromHub,
  chatMessage,
  expectReply,
  messagesFromHistory,
  projectFromHub,
  turnFromReply,
  type HubAgent,
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
