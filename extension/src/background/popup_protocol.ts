/**
 * The popup↔background message contract (Fase 8.2) — a separate, side-effect-free module from
 * `index.ts` on purpose: the popup imports these types too, and `index.ts` registers a real
 * `chrome.runtime.onMessage` listener at module load, which must never run inside the popup's own
 * bundle.
 */

import type { ChatEntry, ConnectionStatus } from "./connection";
import type { DiscoveredHub } from "./discovery";
import type { SkillDto } from "../protocol/messages";
import type { GroupTab } from "./tab_group";

export interface ConnectionSettings {
  host: string;
  port: number;
  deviceName: string;
  authKey: string;
}

export type PopupRequest =
  | { type: "getStatus" }
  | ({ type: "connect" } & ConnectionSettings)
  | { type: "disconnect" }
  | { type: "sendChat"; message: string }
  | { type: "discoverHubs"; port: number }
  | { type: "listSkills" }
  | { type: "saveSkill"; skill: SkillDto; overwrite: boolean }
  | { type: "deleteSkill"; name: string }
  | { type: "addTabToGroup" }
  | { type: "removeTabFromGroup"; tabId: number }
  | { type: "listGroupTabs" };

export interface GetStatusResponse {
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
export type BackgroundEvent = StatusChangedEvent | ChatMessageEvent | GroupChangedEvent;
