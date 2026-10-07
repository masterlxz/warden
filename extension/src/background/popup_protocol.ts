/**
 * The popup↔background message contract (Fase 8.2) — a separate, side-effect-free module from
 * `index.ts` on purpose: the popup imports these types too, and `index.ts` registers a real
 * `chrome.runtime.onMessage` listener at module load, which must never run inside the popup's own
 * bundle.
 */

import type { ChatEntry, ConnectionStatus } from "./connection";
import type { DiscoveredHub } from "./discovery";
import type { AgentTask, AgentTaskAction, ApprovalPrompt, ConversationSummary, DirListing, HubAgents, OrgEdit, SkillDto, ThreadParent } from "../protocol/messages";
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
  /** P121 — opens the channel of an agent: the one conversation it keeps with the person, shown in the chat with the agent fixed. */
  | { type: "openAgentChannel"; agentId: string }
  /** P121 — the channel in front of the panel (`null`: none, or the panel is closed): it is read as it changes, and does not light the icon. */
  | { type: "watchChannel"; conversationId: string | null }
  /** P125 — opens the thread of a message (the existing one, or an empty one the first reply creates). */
  | { type: "openThread"; conversationId: string; messageId: string }
  | { type: "renameConversation"; conversationId: string; title: string }
  | { type: "deleteConversation"; conversationId: string }
  /** P87 — the agent the next turns speak as (`null`: none). */
  | { type: "selectAgent"; agentId: string | null }
  | { type: "refreshAgents" }
  /** P102 — the folder the conversation about to start works in (`null`: none). Only before its first message. */
  | { type: "selectWorkdir"; path: string | null }
  /** P102 — the folders inside `path` on the hub's machine (or a node's); no `path`: where the person starts. */
  | { type: "listDirs"; path?: string }
  | { type: "resolveApproval"; approvalId: number; approved: boolean }
  | { type: "discoverHubs"; port: number }
  /** P123 — the work agents delegated to each other. */
  | { type: "listAgentTasks" }
  /** P123 — pauses, resumes or stops a task, with the hub's pairing key (asked every time, never kept). */
  | { type: "controlAgentTask"; pairingKey: string; taskId: string; action: AgentTaskAction }
  /** P120 — the agents with their roles, superiors and model limits, and the model policies. */
  | { type: "listHubAgents" }
  /** P120 — one change to the organization, with the pairing key. */
  | { type: "editAgentOrg"; pairingKey: string; edit: OrgEdit }
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
  /** P87 — the hub's configured agents, and the one the next turn speaks as. Opening a
   * conversation restores the agent it last spoke with; a new one keeps the last choice. */
  agentIds: string[];
  agentId: string | null;
  /** P102 — the open conversation's working folder, or the one a new conversation will start in (`null`: none). */
  workdir: string | null;
  /** P125 — set when the open conversation is a thread: the message it came from. */
  threadParent: ThreadParent | null;
  /** P121 — the id of each agent's channel (the hub makes it from the agent's name). The open conversation is a channel when its id is one of these. */
  channels: Record<string, string>;
  /** P121 — the ids of the channels with something the person has not seen. */
  unreadChannels: string[];
}

export interface GetStatusResponse extends ConversationState {
  status: ConnectionStatus;
  /** P87 — approvals the hub is waiting on, oldest first. */
  approvals: ApprovalPrompt[];
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

/** Reply to `listAgentTasks` and `controlAgentTask`. `authRejected`: the pairing key was wrong. */
export interface AgentTasksResponse {
  ok: boolean;
  tasks: AgentTask[];
  error?: string;
  authRejected?: boolean;
}

/** Reply to `listHubAgents` and `editAgentOrg`. `authRejected`: the pairing key was wrong. */
export interface HubAgentsResponse {
  ok: boolean;
  hub?: HubAgents;
  error?: string;
  authRejected?: boolean;
}

/** Reply to `{ type: "listSkills" }` — always `skills` (empty on failure too), same posture as
 * `DiscoverHubsResponse`. */
export interface ListSkillsResponse {
  ok: boolean;
  skills: SkillDto[];
  error?: string;
}

/** Reply to `{ type: "listDirs" }` — `listing` is set when `ok`, same posture as the other list replies. */
export interface ListDirsResponse {
  ok: boolean;
  listing?: DirListing;
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
/** P87 — the queue of approvals the hub is waiting on changed. */
export type ApprovalsChangedEvent = { type: "approvalsChanged"; approvals: ApprovalPrompt[] };
export type BackgroundEvent =
  | StatusChangedEvent
  | ChatMessageEvent
  | GroupChangedEvent
  | HistoryLoadedEvent
  | ConversationsChangedEvent
  | ApprovalsChangedEvent;
