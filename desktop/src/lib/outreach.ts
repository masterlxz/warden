// P121 — the agents allowed to start messages (`message_user`, `[[outreach]]`) and the external channels their messages also go to, as the
// Settings form edits them. Pure; mirrors `web/src/hub/outreach.ts`.

import type { OutreachEntry } from "../types";

/** The external channels a message an agent starts can also go to. */
export const OUTREACH_CHANNELS = ["telegram", "whatsapp"] as const;

export type OutreachChannel = (typeof OUTREACH_CHANNELS)[number];

/** Whether the agent may start messages. */
export function outreachOn(list: OutreachEntry[], agent: string): boolean {
  return list.some((o) => o.agent === agent);
}

/** An agent's external channels (none if it may not start messages). */
export function outreachForward(list: OutreachEntry[], agent: string): string[] {
  return list.find((o) => o.agent === agent)?.forward ?? [];
}

/** Turns the agent on or off. On starts with no external channel; off forgets its channels. */
export function setOutreach(list: OutreachEntry[], agent: string, on: boolean): OutreachEntry[] {
  if (on) return outreachOn(list, agent) ? list : [...list, { agent, forward: [] }];
  return list.filter((o) => o.agent !== agent);
}

/** Switches one external channel of an agent that may start messages; for one that may not, nothing changes. */
export function setForward(list: OutreachEntry[], agent: string, channel: OutreachChannel, on: boolean): OutreachEntry[] {
  return list.map((o) => {
    if (o.agent !== agent) return o;
    const rest = o.forward.filter((c) => c !== channel);
    return { ...o, forward: on ? OUTREACH_CHANNELS.filter((c) => c === channel || rest.includes(c)) : rest };
  });
}

/** An agent was renamed: its entry follows the new name. */
export function renameOutreach(list: OutreachEntry[], from: string, to: string): OutreachEntry[] {
  if (from === to) return list;
  return list.map((o) => (o.agent === from ? { ...o, agent: to } : o));
}

/** An agent goes away: its entry goes with it. */
export function dropOutreach(list: OutreachEntry[], agent: string): OutreachEntry[] {
  return list.filter((o) => o.agent !== agent);
}
