// P121 — the feed of activity: who did what among the agents, newest first. Pure; the screen only draws what comes out of here.
// Mirrors `web/src/hub/activity.ts`, in English.

import type { ActivityEvent } from "../types";

/** What the screen calls the assistant that answers with no agent picked (an event with no `actor`). */
export const NO_AGENT_NAME = "The assistant";

const who = (name: string) => (name.trim() === "" ? NO_AGENT_NAME : name);

/** The sentence of an event, without its text (the text is the objective, the answer or the message). */
export function headline(event: ActivityEvent): string {
  const actor = who(event.actor);
  const target = event.target ? who(event.target) : "";
  switch (event.kind) {
    case "delegated":
      return `${actor} delegated a task to ${target}`;
    case "started":
      return `${actor} started the task`;
    case "done":
      return `${actor} finished the task`;
    case "failed":
      return `${actor} could not finish the task`;
    case "cancelled":
      return `${actor}'s task was cancelled`;
    case "note":
      return `${actor} left a note for ${target}`;
    case "reply":
      return `${actor} answered ${target}`;
    case "messaged_user":
      return `${actor} wrote to you`;
    default:
      return `${actor}: ${event.kind}`;
  }
}

/** A short mark per kind, to scan the list by eye. */
export function mark(kind: string): string {
  switch (kind) {
    case "delegated":
      return "→";
    case "started":
      return "▶";
    case "done":
      return "✓";
    case "failed":
      return "✕";
    case "cancelled":
      return "■";
    case "note":
    case "reply":
      return "✉";
    case "messaged_user":
      return "●";
    default:
      return "·";
  }
}

/** The events an agent appears in, doing or receiving. */
export function involving(events: ActivityEvent[], agent: string): ActivityEvent[] {
  return events.filter((e) => e.actor === agent || e.target === agent);
}

/** The agents that appear in the events, alphabetical, without the nameless assistant. */
export function agentsIn(events: ActivityEvent[]): string[] {
  const names = new Set<string>();
  for (const e of events) {
    if (e.actor.trim() !== "") names.add(e.actor);
    if (e.target && e.target.trim() !== "") names.add(e.target);
  }
  return [...names].sort((a, b) => a.localeCompare(b));
}

/** What a click on an event opens: the agent's tasks, the conversation between two agents, or the agent's channel. */
export type Destination = { kind: "tasks"; agent: string } | { kind: "conversation"; id: string } | { kind: "channel"; agent: string };

export function destination(event: ActivityEvent): Destination | null {
  if (event.kind === "messaged_user") return { kind: "channel", agent: event.actor };
  if (event.conversationId) return { kind: "conversation", id: event.conversationId };
  if (event.taskId) {
    const agent = event.kind === "delegated" ? event.target : event.actor;
    return agent ? { kind: "tasks", agent } : null;
  }
  return null;
}

export interface DayGroup {
  /** "Today", "Yesterday" or the date. */
  label: string;
  events: ActivityEvent[];
}

const dayKey = (ms: number) => {
  const d = new Date(ms);
  return d.getFullYear() * 10000 + (d.getMonth() + 1) * 100 + d.getDate();
};

/** The events (already newest first) split by day, each day with its label. */
export function groupByDay(events: ActivityEvent[], now: number): DayGroup[] {
  const formatter = new Intl.DateTimeFormat(undefined, { day: "numeric", month: "long", year: "numeric" });
  const today = dayKey(now);
  const yesterday = dayKey(now - 24 * 60 * 60 * 1000);
  const groups: DayGroup[] = [];
  for (const event of events) {
    const key = dayKey(event.atMs);
    const label = key === today ? "Today" : key === yesterday ? "Yesterday" : formatter.format(event.atMs);
    const last = groups[groups.length - 1];
    if (last && last.label === label) last.events.push(event);
    else groups.push({ label, events: [event] });
  }
  return groups;
}
