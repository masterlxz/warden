/** What a code engine has done so far in the task a conversation is running (P103 b), built from the hub's `chatEvent`s
 * and shown in place of the "thinking" dots until the turn ends with its `chatResponse`. A timeline, because the work and
 * the words alternate: some text, a tool, more text. */

import type { ChatEventDto } from "./messages";

export type LiveItem =
  | { kind: "text"; text: string }
  | { kind: "tool"; callId: string; tool: string; title: string; status: "running" | "completed" | "failed" };

export interface LiveTurn {
  items: LiveItem[];
  /** The engine saying something about itself (a retry, a wait): shown under the work, replaced by the next. */
  notice?: string;
}

export function applyEvent(turn: LiveTurn | undefined, event: ChatEventDto): LiveTurn {
  const current = turn ?? { items: [] };
  switch (event.type) {
    case "text": {
      const last = current.items[current.items.length - 1];
      // More words continue the last block of text; a tool in between starts a new one.
      if (last?.kind === "text") {
        return { ...current, items: [...current.items.slice(0, -1), { kind: "text", text: last.text + event.text }] };
      }
      return { ...current, items: [...current.items, { kind: "text", text: event.text }] };
    }
    case "tool": {
      const line: LiveItem = { kind: "tool", callId: event.callId, tool: event.tool, title: event.title, status: event.status };
      const at = current.items.findIndex((item) => item.kind === "tool" && item.callId === event.callId);
      // A later stage of the same call updates its line, wherever it was left.
      if (at >= 0) return { ...current, items: current.items.map((item, i) => (i === at ? line : item)) };
      return { ...current, items: [...current.items, line] };
    }
    case "notice":
      return { ...current, notice: event.text };
  }
}
