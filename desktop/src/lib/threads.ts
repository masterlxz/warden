// P125 — threads: a child conversation tied to one message of another, on a hub. Pure, so it can be tested without a window. Mirrors
// `web/src/hub/threads.ts`.

import type { ChatMessage, Conversation } from "../types";

/** The conversations the list shows: threads are left out, they only show from the message they came from. */
export function visibleConversations(conversations: Conversation[]): Conversation[] {
  return conversations.filter((c) => !c.parent);
}

/** The thread of a message: its conversation and how many messages the person sent in it. */
export interface ThreadInfo {
  conversationId: string;
  replies: number;
}

/** The threads of a conversation by the id of the message they came from. A message has at most one; if a race between two devices made
 * two, the one with more replies wins. */
export function threadsOf(conversations: Conversation[], conversationId: string): Record<string, ThreadInfo> {
  const found: Record<string, ThreadInfo> = {};
  for (const c of conversations) {
    if (c.parent?.conversationId !== conversationId) continue;
    const info = { conversationId: c.id, replies: c.replies ?? 0 };
    const known = found[c.parent.messageId];
    if (!known || info.replies > known.replies) found[c.parent.messageId] = info;
  }
  return found;
}

/** What the chip of a message says: `1 reply`, `3 replies`. */
export function repliesLabel(replies: number): string {
  return replies === 1 ? "1 reply" : `${replies} replies`;
}

/** The id a thread can start from: only a message read from the hub has one. */
export function threadAnchor(message: ChatMessage): string | null {
  return message.hubId ?? null;
}
