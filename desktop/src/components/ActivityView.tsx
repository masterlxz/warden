import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { ActivityEvent } from "../types";
import { hubActivity } from "../lib/hub";
import { agentsIn, destination, groupByDay, headline, involving, mark, type Destination } from "../lib/activity";

const REFRESH_MS = 3000;
const timeFormatter = new Intl.DateTimeFormat(undefined, { timeStyle: "short" });

/** P121 — the feed of activity: who did what among the agents, newest first, without opening each conversation. It refreshes while it is
 * open. For this computer it reads the files the engine already writes, and for a hub in use it asks the hub. A click goes to what the
 * event touches: the agent's tasks, the conversation between two agents, or the agent's channel. */
function ActivityView({ remote = false, onOpen }: { remote?: boolean; onOpen: (to: Destination) => void }) {
  const [events, setEvents] = useState<ActivityEvent[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [agent, setAgent] = useState("");

  const load = useCallback(async () => {
    try {
      setEvents(remote ? await hubActivity() : await invoke<ActivityEvent[]>("list_activity"));
      setError(null);
    } catch (err) {
      setError(String(err));
    }
  }, [remote]);

  useEffect(() => {
    void load();
    const timer = window.setInterval(() => void load(), REFRESH_MS);
    return () => window.clearInterval(timer);
  }, [load]);

  const agents = useMemo(() => agentsIn(events ?? []), [events]);
  // The filter names an agent that may leave the feed (its events aged out): then it shows everyone again.
  const chosen = agents.includes(agent) ? agent : "";
  const groups = useMemo(() => groupByDay(chosen ? involving(events ?? [], chosen) : (events ?? []), Date.now()), [events, chosen]);

  return (
    <div className="settings-view">
      <h2 className="settings-title">Activity</h2>
      <p className="settings-hint">
        What happened among the agents {remote ? "on this hub" : "on this computer"}: who delegated, started and finished what, the notes they left each other
        {remote ? " and the messages an agent wrote first to you" : ""}.
      </p>
      <label className="settings-field activity-filter">
        <span className="settings-label">Agent</span>
        <select className="settings-select" value={chosen} onChange={(e) => setAgent(e.currentTarget.value)}>
          <option value="">All</option>
          {agents.map((name) => (
            <option key={name} value={name}>
              {name}
            </option>
          ))}
        </select>
      </label>
      {error && <p className="usage-error">{error}</p>}
      {events === null && !error && <p className="settings-hint">Loading…</p>}
      {events !== null && groups.length === 0 && (
        <p className="settings-hint">{chosen ? `Nothing involves ${chosen} yet.` : "Nothing yet. When agents delegate tasks or leave each other notes, it shows here."}</p>
      )}
      {groups.map((group) => (
        <section key={group.label} className="activity-day">
          <h3 className="activity-day-title">{group.label}</h3>
          <ul className="activity-list">
            {group.events.map((event) => {
              const to = destination(event);
              // A channel is the hub's: on this computer there is none to open.
              const canOpen = to !== null && !(to.kind === "channel" && !remote);
              return (
                <li key={event.id} className={`activity-item activity-item--${event.kind}`}>
                  <span className="activity-time">{timeFormatter.format(event.atMs)}</span>
                  <span className="activity-mark" aria-hidden="true">
                    {mark(event.kind)}
                  </span>
                  <div className="activity-body">
                    {canOpen ? (
                      <button type="button" className="settings-browse-btn activity-headline" onClick={() => onOpen(to)}>
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

export default ActivityView;
