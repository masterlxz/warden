// P125 — as threads: uma conversa filha ligada a uma mensagem de outra. Pura, para testar sem janela. Espelha `web/src/hub/threads.ts`.

import type { ConversationSummary } from "./messages";

/** O começo do id do canal de um agente (P121): a conversa fixa que ele mantém com a pessoa. Igual ao `CHANNEL_PREFIX` do hub. */
export const CHANNEL_PREFIX = "channel-";

/** Se `id` é o canal de um agente. */
export function isAgentChannel(id: string): boolean {
  return id.startsWith(CHANNEL_PREFIX);
}

/** As conversas que a lista mostra: as threads ficam de fora, só aparecem a partir da mensagem de onde saíram, e os canais dos agentes
 * também, que têm a aba deles. */
export function visibleConversations<T extends Pick<ConversationSummary, "parent" | "id">>(conversations: T[]): T[] {
  return conversations.filter((c) => !c.parent && !isAgentChannel(c.id));
}

/** Uma thread de uma mensagem: a conversa dela e quantas mensagens a pessoa mandou nela. */
export interface ThreadInfo {
  conversationId: string;
  replies: number;
}

/** As threads de uma conversa, pelo id da mensagem de onde saíram. Uma mensagem tem no máximo uma; se por uma corrida entre dois aparelhos
 * houver duas, vale a que tem mais respostas. */
export function threadsOf(conversations: ConversationSummary[], conversationId: string): Record<string, ThreadInfo> {
  const found: Record<string, ThreadInfo> = {};
  for (const c of conversations) {
    if (c.parent?.conversationId !== conversationId) continue;
    const info = { conversationId: c.id, replies: c.replies ?? 0 };
    const known = found[c.parent.messageId];
    if (!known || info.replies > known.replies) found[c.parent.messageId] = info;
  }
  return found;
}

/** Cola os ids do histórico do hub nas mensagens que já estão na tela e ainda não os têm (a resposta que acabou de chegar). Só vale quando
 * as mensagens da tela, fora os erros (que o hub não guarda), são tantas quanto as do histórico: do contrário há um turno no meio do caminho
 * e as posições não casam, então a lista fica como está. */
export function withMessageIds<T extends { role: string; id?: string }>(entries: T[], history: Array<{ id?: string }>): T[] {
  if (entries.filter((e) => e.role !== "error").length !== history.length) return entries;
  let next = 0;
  return entries.map((entry) => {
    if (entry.role === "error") return entry;
    const id = history[next++].id;
    return id && !entry.id ? { ...entry, id } : entry;
  });
}

/** O que o chip de uma mensagem diz: `1 resposta`, `3 respostas`. */
export function repliesLabel(replies: number): string {
  return replies === 1 ? "1 resposta" : `${replies} respostas`;
}
