import { useCallback, useEffect, useMemo, useState } from "react";
import type { ActivityEvent } from "../protocol/messages";
import type { ActivityResponse } from "../background/popup_protocol";
import { agentsIn, destination, groupByDay, headline, involving, mark, type Destination } from "./lib/activity";

const REFRESH_MS = 3000;
const timeFormatter = new Intl.DateTimeFormat("pt-BR", { timeStyle: "short" });

/** P121 — o feed de atividade: quem fez o quê entre os agentes, do mais novo ao mais velho, sem abrir cada conversa. Atualiza enquanto a aba
 * está aberta. Um clique leva ao que o evento toca: as tarefas do agente, a conversa entre dois agentes ou o canal dele. */
export default function ActivityView({ onOpen }: { onOpen: (to: Destination) => void }) {
  const [events, setEvents] = useState<ActivityEvent[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [agent, setAgent] = useState("");

  const load = useCallback(() => {
    chrome.runtime.sendMessage({ type: "listActivity" }).then((res: ActivityResponse) => {
      if (res.ok) {
        setEvents(res.events);
        setError(null);
      } else {
        setError(res.error ?? "falha ao listar a atividade");
      }
    });
  }, []);

  useEffect(() => {
    load();
    const timer = window.setInterval(load, REFRESH_MS);
    return () => window.clearInterval(timer);
  }, [load]);

  const agents = useMemo(() => agentsIn(events ?? []), [events]);
  // O filtro pode apontar para um agente que saiu do feed (os eventos dele envelheceram): aí mostra todos de novo.
  const chosen = agents.includes(agent) ? agent : "";
  const groups = useMemo(() => groupByDay(chosen ? involving(events ?? [], chosen) : (events ?? []), Date.now()), [events, chosen]);

  return (
    <div className="agents-pane">
      <p className="skills-hint">
        O que aconteceu entre os agentes neste hub: quem delegou, começou e terminou o quê, os recados que deixaram uns aos outros e as mensagens que
        um agente escreveu primeiro para você.
      </p>
      <label className="settings-field activity-filter">
        Agente
        <select value={chosen} onChange={(e) => setAgent(e.target.value)}>
          <option value="">Todos</option>
          {agents.map((name) => (
            <option key={name} value={name}>
              {name}
            </option>
          ))}
        </select>
      </label>
      {error && <p className="error-banner">{error}</p>}
      {events === null && !error && <p className="skills-hint">Carregando…</p>}
      {events !== null && groups.length === 0 && (
        <p className="skills-hint">{chosen ? `Nada envolve ${chosen} ainda.` : "Nada ainda. Quando os agentes delegarem tarefas, deixarem recados ou escreverem para você, aparece aqui."}</p>
      )}
      {groups.map((group) => (
        <section key={group.label} className="activity-day">
          <h3 className="activity-day-title">{group.label}</h3>
          <ul className="activity-list">
            {group.events.map((event) => {
              const to = destination(event);
              return (
                <li key={event.id} className={`activity-item activity-item--${event.kind}`}>
                  <span className="activity-time">{timeFormatter.format(event.atMs)}</span>
                  <span className="activity-mark" aria-hidden="true">
                    {mark(event.kind)}
                  </span>
                  <div className="activity-body">
                    {to ? (
                      <button type="button" className="link-button activity-headline" onClick={() => onOpen(to)}>
                        {headline(event)}
                      </button>
                    ) : (
                      <span className="activity-headline">{headline(event)}</span>
                    )}
                    {event.text && <p className="activity-text">{event.text}</p>}
                  </div>
                </li>
              );
            })}
          </ul>
        </section>
      ))}
    </div>
  );
}
