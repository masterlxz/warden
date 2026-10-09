// P121 — what an agent did outside its channel: the notes it traded with other agents ("A → B", `message_agent`) and the runs nobody was
// watching (scheduled tasks and webhooks, `task-*`). Pure, so it is tested without a window. Mirrors `web/src/hub/agentWork.ts`. The ids and
// the title are the hub's (`activity.rs`).

import type { Conversation } from "../types";

/** The start of the id of a conversation between two agents (`message_agent::thread_id`). */
export const NOTE_PREFIX = "agents-";
/** The start of the id of a scheduled task's conversation (`tasks::CONVERSATION_PREFIX`); a webhook's is `task-hook-`. */
export const RUN_PREFIX = "task-";
const HOOK_PREFIX = "task-hook-";
const ARROW = " → ";

export type AgentWorkKind = "note-out" | "note-in" | "run-task" | "run-hook";

/** One conversation of the agent outside its channel. `other` is the colleague of a note. */
export interface AgentWorkItem {
  id: string;
  title: string;
  kind: AgentWorkKind;
  other?: string;
  updatedAt: number;
}

/** The two sides of a conversation "A → B", or `null` when the title has no arrow. */
function ends(title: string): [string, string] | null {
  const at = title.indexOf(ARROW);
  if (at < 0) return null;
  return [title.slice(0, at).trim(), title.slice(at + ARROW.length).trim()];
}

/** The conversations of `agent` outside its channel, newest first: the notes it left or got and the runs it made. A thread (P125) is not
 * one, it shows from the message it came from. */
export function agentWork(conversations: Pick<Conversation, "id" | "title" | "updatedAt" | "agentId" | "parent">[], agent: string): AgentWorkItem[] {
  const items: AgentWorkItem[] = [];
  for (const c of conversations) {
    if (c.parent) continue;
    if (c.id.startsWith(NOTE_PREFIX)) {
      const pair = ends(c.title);
      if (!pair || pair[0] === pair[1]) continue;
      if (pair[0] === agent) items.push({ id: c.id, title: c.title, kind: "note-out", other: pair[1], updatedAt: c.updatedAt });
      else if (pair[1] === agent) items.push({ id: c.id, title: c.title, kind: "note-in", other: pair[0], updatedAt: c.updatedAt });
    } else if (c.id.startsWith(RUN_PREFIX) && c.agentId === agent) {
      items.push({ id: c.id, title: c.title, kind: c.id.startsWith(HOOK_PREFIX) ? "run-hook" : "run-task", updatedAt: c.updatedAt });
    }
  }
  return items.sort((a, b) => b.updatedAt - a.updatedAt || a.id.localeCompare(b.id));
}

/** What the list says about a conversation: `to ana`, `from ana`, `scheduled task`, `webhook`. */
export function workLabel(item: AgentWorkItem): string {
  switch (item.kind) {
    case "note-out":
      return `to ${item.other}`;
    case "note-in":
      return `from ${item.other}`;
    case "run-task":
      return "scheduled task";
    case "run-hook":
      return "webhook";
  }
}

/** What the channel's button says: `Notes and runs (3)`, or without the number when there are none. */
export function workButtonLabel(count: number): string {
  return count === 0 ? "Notes and runs" : `Notes and runs (${count})`;
}
