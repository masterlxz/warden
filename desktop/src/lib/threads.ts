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
    const info = { conversationId: c.id, replies: replyCount(c) };
    const known = found[c.parent.messageId];
    if (!known || info.replies > known.replies) found[c.parent.messageId] = info;
  }
  return found;
}

/** How many messages the person sent in a thread: the hub counts them in the list it sends; a conversation read from this computer has its
 * messages, so they are counted here. */
export function replyCount(thread: Conversation): number {
  return thread.replies ?? thread.messages.filter((m) => m.role === "user").length;
}

/** How many messages of the conversation a thread's model sees before the thread itself: the last ones that end at the message it came
 * from. The same cut as the hub's (`THREAD_CONTEXT_MAX`). */
export const THREAD_CONTEXT_MAX = 40;

/** What a thread on this computer sends the model: the conversation it came from up to the message it started from (`THREAD_CONTEXT_MAX`
 * at most), then the thread's own messages. Empty context when the message isn't there. */
export function threadHistory(parent: Conversation, messageId: string, thread: ChatMessage[]): ChatMessage[] {
  const at = parent.messages.findIndex((m) => m.id === messageId);
  const context = at < 0 ? [] : parent.messages.slice(Math.max(0, at + 1 - THREAD_CONTEXT_MAX), at + 1);
  return [...context, ...thread];
}

/** What the chip of a message says: `1 reply`, `3 replies`. */
export function repliesLabel(replies: number): string {
  return replies === 1 ? "1 reply" : `${replies} replies`;
}

/** The id a thread can start from: on a hub, only a message read from it has one; on this computer every saved message does (its own
 * id is the one on disk). */
export function threadAnchor(message: ChatMessage, local = false): string | null {
  return message.hubId ?? (local ? message.id : null);
}
