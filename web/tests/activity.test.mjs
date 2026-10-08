// P121 — the pure half of the feed of activity (`src/hub/activity.ts`). Run with `npm test`.

import assert from "node:assert/strict";
import { describe, test } from "node:test";
import { agentsIn, destination, groupByDay, headline, involving, mark } from "../src/hub/activity.ts";

const event = (kind, actor, extra = {}) => ({ id: `${kind}-${actor}`, atMs: 1, kind, actor, text: "", ...extra });

describe("the sentence of an event", () => {
  test("says who did what to whom", () => {
    assert.equal(headline(event("delegated", "manager", { target: "backend" })), "manager delegou uma tarefa a backend");
    assert.equal(headline(event("note", "ana", { target: "bia" })), "ana deixou um recado para bia");
    assert.equal(headline(event("reply", "bia", { target: "ana" })), "bia respondeu a ana");
    assert.equal(headline(event("messaged_user", "pirate")), "pirate escreveu para você");
    assert.equal(headline(event("done", "backend")), "backend concluiu a tarefa");
    assert.equal(headline(event("cancelled", "backend")), "A tarefa de backend foi cancelada");
  });

  test("names the assistant nobody picked", () => {
    assert.equal(headline(event("delegated", "", { target: "writer" })), "O assistente delegou uma tarefa a writer");
  });

  test("tells agents made or removed by another, and the runs of scheduled tasks and webhooks", () => {
    assert.equal(headline(event("created_agent", "chief", { target: "poet" })), "chief criou o agente poet");
    assert.equal(headline(event("removed_agent", "chief", { target: "poet" })), "chief removeu o agente poet");
    assert.equal(headline(event("scheduled_ran", "reporter", { target: "daily" })), "reporter rodou a tarefa agendada daily");
    assert.equal(headline(event("scheduled_failed", "reporter", { target: "daily" })), "A tarefa agendada daily falhou");
    assert.equal(headline(event("webhook_ran", "", { target: "build" })), "O assistente atendeu o webhook build");
    assert.equal(headline(event("webhook_failed", "", { target: "build" })), "O webhook build falhou");
  });

  test("every kind has a mark, and an unknown one still reads", () => {
    const kinds = ["delegated", "started", "done", "failed", "cancelled", "note", "reply", "messaged_user", "created_agent", "removed_agent", "scheduled_ran", "scheduled_failed", "webhook_ran", "webhook_failed"];
    for (const kind of kinds) assert.notEqual(mark(kind), "·");
    assert.equal(mark("later"), "·");
    assert.equal(headline(event("later", "x")), "x: later");
  });
});

describe("the agents", () => {
  const events = [event("delegated", "manager", { target: "backend" }), event("done", "backend"), event("messaged_user", "pirate"), event("started", "")];

  test("the filter keeps the events an agent did or received", () => {
    assert.deepEqual(involving(events, "backend").map((e) => e.kind), ["delegated", "done"]);
    assert.deepEqual(involving(events, "ghost"), []);
  });

  test("the list of agents is sorted and leaves out the nameless one", () => {
    assert.deepEqual(agentsIn(events), ["backend", "manager", "pirate"]);
  });

  test("the task or webhook a run is about is not an agent; the agent it made is", () => {
    const more = [event("scheduled_ran", "reporter", { target: "daily" }), event("created_agent", "chief", { target: "poet" })];
    assert.deepEqual(agentsIn(more), ["chief", "poet", "reporter"]);
    assert.deepEqual(involving(more, "daily"), []);
    assert.deepEqual(involving(more, "poet").map((e) => e.kind), ["created_agent"]);
  });
});

describe("where a click goes", () => {
  test("a message the agent started opens its channel", () => {
    assert.deepEqual(destination(event("messaged_user", "pirate", { conversationId: "channel-1" })), { kind: "channel", agent: "pirate" });
  });

  test("a note or an answer opens the conversation between the two", () => {
    assert.deepEqual(destination(event("note", "ana", { target: "bia", conversationId: "agents-1" })), { kind: "conversation", id: "agents-1" });
  });

  test("a task event opens the work of the agent it is about", () => {
    assert.deepEqual(destination(event("delegated", "manager", { target: "backend", taskId: "t1" })), { kind: "tasks", agent: "backend" });
    assert.deepEqual(destination(event("done", "backend", { taskId: "t1" })), { kind: "tasks", agent: "backend" });
    assert.equal(destination(event("done", "", { taskId: "t1" })), null, "no agent to filter by");
    assert.equal(destination(event("later", "x")), null);
  });

  test("a run opens its conversation, and an agent made or removed opens nothing", () => {
    assert.deepEqual(destination(event("scheduled_ran", "reporter", { target: "daily", conversationId: "task-daily" })), { kind: "conversation", id: "task-daily" });
    assert.equal(destination(event("created_agent", "chief", { target: "poet" })), null);
  });
});

describe("the days", () => {
  const at = (day, hour) => new Date(2026, 9, day, hour).getTime();
  const now = at(7, 15);

  test("groups the newest-first list by day and names today and yesterday", () => {
    const events = [{ ...event("done", "a"), atMs: at(7, 14) }, { ...event("done", "b"), atMs: at(7, 9) }, { ...event("done", "c"), atMs: at(6, 22) }, { ...event("done", "d"), atMs: at(1, 10) }];
    const groups = groupByDay(events, now);
    assert.deepEqual(groups.map((g) => [g.label, g.events.length]).slice(0, 2), [["Hoje", 2], ["Ontem", 1]]);
    assert.equal(groups.length, 3);
    assert.match(groups[2].label, /1.*outubro.*2026/);
  });

  test("an empty list has no day", () => {
    assert.deepEqual(groupByDay([], now), []);
  });
});
