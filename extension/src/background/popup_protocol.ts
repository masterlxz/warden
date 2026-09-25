/**
 * The popup↔background message contract (Fase 8.2) — a separate, side-effect-free module from
 * `index.ts` on purpose: the popup imports these types too, and `index.ts` registers a real
 * `chrome.runtime.onMessage` listener at module load, which must never run inside the popup's own
 * bundle.
 */

import type { ChatEntry, ConnectionStatus } from "./connection";
import type { DiscoveredHub } from "./discovery";
import type { ConversationSummary, SkillDto } from "../protocol/messages";
import type { GroupTab } from "./tab_group";

export interface ConnectionSettings {
  host: string;
  port: number;
  deviceName: string;
  authKey: string;
  /** P36 — connect over wss:// (a TLS-only hub, host = the name its certificate covers). */
  secure: boolean;
}

export type PopupRequest =
  | { type: "getStatus" }
  | ({ type: "connect" } & ConnectionSettings)
  | { type: "disconnect" }
  | { type: "sendChat"; message: string }
  /** P78 — switches the chat to another of this device's conversations. */
  | { type: "selectConversation"; conversationId: string }
  /** P78 — an empty conversation, created on the hub by its first message. */
  | { type: "newConversation" }
  | { type: "renameConversation"; conversationId: string; title: string }
  | { type: "deleteConversation"; conversationId: string }
  | { type: "discoverHubs"; port: number }
  | { type: "listSkills" }
  | { type: "saveSkill"; skill: SkillDto; overwrite: boolean }
  | { type: "deleteSkill"; name: string }
  | { type: "addTabToGroup" }
  | { type: "removeTabFromGroup"; tabId: number }
  | { type: "listGroupTabs" };

/** P78 — this device's conversations and which one the chat shows. `activeConversationId` may be
 * missing from `conversations`: a new conversation only exists on the hub after its first message.
 * `pendingIds` are the conversations waiting on an answer. */
export interface ConversationState {
  conversations: ConversationSummary[];
  activeConversationId: string | null;
  pendingIds: string[];
}

export interface GetStatusResponse extends ConversationState {
  status: ConnectionStatus;
  history: ChatEntry[];
  savedSettings: Partial<ConnectionSettings>;
}

export interface OkResponse {
  ok: boolean;
  error?: string;
}

/** Reply to `{ type: "discoverHubs" }` — always `hubs` (empty on failure too), so the panel never
 * needs to special-case `undefined` before rendering the list. */
export interface DiscoverHubsResponse {
  ok: boolean;
  hubs: DiscoveredHub[];
  error?: string;
}

/** Reply to `{ type: "listSkills" }` — always `skills` (empty on failure too), same posture as
 * `DiscoverHubsResponse`. */
export interface ListSkillsResponse {
  ok: boolean;
  skills: SkillDto[];
  error?: string;
}

/** Reply to `{ type: "addTabToGroup" }`. */
export interface AddTabToGroupResponse {
  ok: boolean;
  tab?: GroupTab;
  error?: string;
}

/** Reply to `{ type: "listGroupTabs" }` — always `tabs` (empty on failure too), same posture as
 * `DiscoverHubsResponse`/`ListSkillsResponse`. */
export interface ListGroupTabsResponse {
  ok: boolean;
  tabs: GroupTab[];
  error?: string;
}

export type StatusChangedEvent = { type: "statusChanged"; status: ConnectionStatus };
export type ChatMessageEvent = { type: "chatMessage"; entry: ChatEntry };
export type GroupChangedEvent = { type: "groupChanged" };
/** P40 — the open conversation's transcript arrived from the hub (after connecting, or after
 * switching conversations, P78); `history` is the whole transcript now (older messages first),
 * replacing what the panel had. */
export type HistoryLoadedEvent = { type: "historyLoaded"; history: ChatEntry[] };
/** P78 — the conversation list, the open conversation or what's waiting on an answer changed. */
export type ConversationsChangedEvent = { type: "conversationsChanged" } & ConversationState;
export type BackgroundEvent = StatusChangedEvent | ChatMessageEvent | GroupChangedEvent | HistoryLoadedEvent | ConversationsChangedEvent;
