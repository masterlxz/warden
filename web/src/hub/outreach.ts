// P121 — os agentes que podem iniciar mensagens (`message_user`, `[[outreach]]`) e os canais externos para onde elas também vão, como o
// formulário de Configurações os edita. Puro; espelha `desktop/src/lib/outreach.ts`.

import type { OutreachEntry } from "./messages";

/** Os canais externos a que uma mensagem iniciada pelo agente pode ir também. */
export const OUTREACH_CHANNELS = ["telegram", "whatsapp"] as const;

export type OutreachChannel = (typeof OUTREACH_CHANNELS)[number];

/** O agente pode iniciar mensagens. */
export function outreachOn(list: OutreachEntry[], agent: string): boolean {
  return list.some((o) => o.agent === agent);
}

/** Os canais externos de um agente (vazio se ele não pode iniciar mensagens). */
export function outreachForward(list: OutreachEntry[], agent: string): string[] {
  return list.find((o) => o.agent === agent)?.forward ?? [];
}

/** Liga ou desliga o agente. Ligar começa sem canal externo; desligar esquece os canais dele. */
export function setOutreach(list: OutreachEntry[], agent: string, on: boolean): OutreachEntry[] {
  if (on) return outreachOn(list, agent) ? list : [...list, { agent, forward: [] }];
  return list.filter((o) => o.agent !== agent);
}

/** Liga ou desliga um canal externo de um agente que pode iniciar mensagens; para um que não pode, não muda nada. */
export function setForward(list: OutreachEntry[], agent: string, channel: OutreachChannel, on: boolean): OutreachEntry[] {
  return list.map((o) => {
    if (o.agent !== agent) return o;
    const rest = o.forward.filter((c) => c !== channel);
    return { ...o, forward: on ? OUTREACH_CHANNELS.filter((c) => c === channel || rest.includes(c)) : rest };
  });
}

/** Um agente foi renomeado: a entrada dele o segue. */
export function renameOutreach(list: OutreachEntry[], from: string, to: string): OutreachEntry[] {
  if (from === to) return list;
  return list.map((o) => (o.agent === from ? { ...o, agent: to } : o));
}

/** Um agente vai embora: a entrada dele vai junto. */
export function dropOutreach(list: OutreachEntry[], agent: string): OutreachEntry[] {
  return list.filter((o) => o.agent !== agent);
}
