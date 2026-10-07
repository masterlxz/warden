// P121 — which agent channels have something the person has not seen. Pure, so it is tested without a window. Mirrors `web/src/hub/unread.ts`.
//
// The hub does not keep a "read" mark: each device remembers, per channel, the time of the last change it showed (`seen`). A channel is unread
// when it changed after that and is not the one open. The first time a device runs there is nothing to compare with, so what is there then
// counts as seen (`baseline`); after that a channel with no mark is a new one, and unread.

import { isAgentChannel } from "./threads.ts";

/** Channel id → the `updatedAt` of the last change this device showed. */
export type SeenMap = Record<string, number>;

type Stamped = { id: string; updatedAt: number };

const SEEN_KEY = "warden.channelSeen";

/** The marks this app has kept, or `null` when there are none yet (the first run). */
export function loadSeen(): SeenMap | null {
  try {
    const raw = localStorage.getItem(SEEN_KEY);
    const parsed: unknown = raw ? JSON.parse(raw) : null;
    return parsed && typeof parsed === "object" && !Array.isArray(parsed) ? (parsed as SeenMap) : null;
  } catch {
    return null;
  }
}

export function saveSeen(seen: SeenMap | null): void {
  if (!seen) return;
  try {
    localStorage.setItem(SEEN_KEY, JSON.stringify(seen));
  } catch {
    // Without storage the app just forgets what it showed when it closes.
  }
}

/** The channels among `conversations`. */
export function channelsOf<T extends { id: string }>(conversations: T[]): T[] {
  return conversations.filter((c) => isAgentChannel(c.id));
}

/** The first run on a device: every channel that exists counts as seen, so the first list does not light up all of them. */
export function baseline(conversations: Stamped[]): SeenMap {
  return Object.fromEntries(channelsOf(conversations).map((c) => [c.id, c.updatedAt]));
}

/** Whether `channel` has changes the device has not shown: it changed after the last one it showed, and it is not open right now. */
export function isUnread(channel: Stamped, seen: SeenMap, openId: string | null): boolean {
  return channel.id !== openId && channel.updatedAt > (seen[channel.id] ?? 0);
}

/** The ids of the channels with something unseen. */
export function unreadIds(conversations: Stamped[], seen: SeenMap, openId: string | null): string[] {
  return channelsOf(conversations).filter((c) => isUnread(c, seen, openId)).map((c) => c.id);
}

/** `seen` with `channel` shown up to its latest change. The same object when nothing moves, so a state update can be skipped. */
export function markSeen(seen: SeenMap, channel: Stamped): SeenMap {
  return (seen[channel.id] ?? 0) >= channel.updatedAt ? seen : { ...seen, [channel.id]: channel.updatedAt };
}
