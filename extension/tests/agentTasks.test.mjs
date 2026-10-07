// P123 — the pure half of the "Agent work" screen (`src/lib/agentTasks.ts`). Run with `npm test`.

import assert from "node:assert/strict";
import { describe, test } from "node:test";
import { actionsFor, activityLine, activityOf, agoLabel, durationLabel, formatTokens, groupTasks, involvingAgent } from "../src/sidepanel/lib/agentTasks.ts";

const task = (id, group, state, extra = {}) => ({ id, group, owner: "chief", assignee: id, objective: `do ${id}`, channel: "desktop", state, createdAtMs: 1000, ...extra });

describe("grouping the tasks of a turn", () => {
  test("counts the finished ones as the progress and sums the tokens", () => {
    const [group] = groupTasks([
      task("backend", "g1", "done", { totalTokens: 100, createdAtMs: 1 }),
      task("frontend", "g1", "done", { totalTokens: 50, createdAtMs: 2 }),
      task("database", "g1", "running", { createdAtMs: 3 }),
      task("tests", "g1", "pending", { createdAtMs: 4 }),
      task("security", "g1", "failed", { createdAtMs: 5 }),
    ]);
    assert.equal(group.total, 5);
    assert.equal(group.finished, 3);
    assert.equal(group.percent, 60);
    assert.deepEqual(group.counts, { pending: 1, running: 1, waiting: 0, paused: 0, done: 2, failed: 1, cancelled: 0 });
    assert.equal(group.totalTokens, 150);
    assert.equal(group.active, true);
    assert.equal(group.owner, "chief");
    assert.deepEqual(group.tasks.map((t) => t.id), ["backend", "frontend", "database", "tests", "security"], "in the order the turn started them");
  });

  test("a batch with nothing left to wait for is not active, and cancelled counts as finished", () => {
    const [group] = groupTasks([task("a", "g", "done"), task("b", "g", "cancelled")]);
    assert.equal(group.active, false);
    assert.equal(group.percent, 100);
  });

  test("the newest group comes first and groups stay apart", () => {
    const groups = groupTasks([task("old", "g1", "done", { createdAtMs: 10 }), task("new", "g2", "running", { createdAtMs: 99 }), task("old2", "g1", "done", { createdAtMs: 11 })]);
    assert.deepEqual(groups.map((g) => [g.group, g.total]), [["g2", 1], ["g1", 2]]);
  });

  test("a state this app doesn't know counts as pending, and no tasks is no groups", () => {
    const [group] = groupTasks([task("a", "g", "some-new-state")]);
    assert.equal(group.counts.pending, 1);
    assert.deepEqual(groupTasks([]), []);
  });
});

describe("subtasks", () => {
  test("a task is followed by its subtasks, one level deeper, and the progress counts the whole tree", () => {
    const [group] = groupTasks([
      task("manager", "g", "waiting", { createdAtMs: 1 }),
      task("helper-a", "g", "done", { parentId: "manager", createdAtMs: 2 }),
      task("helper-b", "g", "running", { parentId: "manager", createdAtMs: 3 }),
      task("other", "g", "done", { createdAtMs: 4 }),
      task("deep", "g", "pending", { parentId: "helper-b", createdAtMs: 5 }),
    ]);
    assert.deepEqual(group.rows.map((r) => [r.task.id, r.depth]), [["manager", 0], ["helper-a", 1], ["helper-b", 1], ["deep", 2], ["other", 0]]);
    assert.equal(group.total, 5);
    assert.equal(group.finished, 2);
    assert.equal(group.counts.waiting, 1);
    assert.equal(group.active, true, "a task waiting for an agent is still active");
  });

  test("a subtask whose parent is missing is shown at the top, and a loop does not hang", () => {
    const [group] = groupTasks([task("orphan", "g", "done", { parentId: "ghost", createdAtMs: 1 })]);
    assert.deepEqual(group.rows.map((r) => [r.task.id, r.depth]), [["orphan", 0]]);
    const [loop] = groupTasks([task("a", "g", "done", { parentId: "b", createdAtMs: 1 }), task("b", "g", "done", { parentId: "a", createdAtMs: 2 })]);
    assert.equal(loop.rows.length, 2, "both are shown once");
  });
});

describe("how a task is shown", () => {
  test("tokens are compact", () => {
    assert.equal(formatTokens(950), "950");
    // pt-BR puts a no-break space before "mil".
    assert.match(formatTokens(12900), /^12,9\smil$/);
  });

  test("the duration is empty before it starts, and counts to now while it runs", () => {
    assert.equal(durationLabel(task("a", "g", "pending"), 99999), null);
    assert.equal(durationLabel(task("a", "g", "running", { startedAtMs: 1000 }), 5000), "4s");
    assert.equal(durationLabel(task("a", "g", "done", { startedAtMs: 0, finishedAtMs: 125000 }), 999999), "2m 05s");
    assert.equal(durationLabel(task("a", "g", "done", { startedAtMs: 0, finishedAtMs: 3780000 }), 0), "1h 03m");
  });
});

describe("the tasks of one agent", () => {
  test("it keeps the ones the agent was given and the ones it delegated, and nobody else's", () => {
    const tasks = [
      task("backend", "g1", "done", { owner: "chief" }),
      task("frontend", "g1", "done", { owner: "chief" }),
      task("db", "g1", "running", { owner: "backend", parentId: "backend" }),
      task("docs", "g2", "done", { owner: "writer" }),
    ];
    assert.deepEqual(involvingAgent(tasks, "backend").map((t) => t.id), ["backend", "db"], "given to it, and delegated by it");
    assert.deepEqual(involvingAgent(tasks, "chief").map((t) => t.id), ["backend", "frontend"]);
    assert.deepEqual(involvingAgent(tasks, "ghost"), []);
  });
});

describe("what a person can do to a task", () => {
  test("only a task running in the answering process can be controlled, and the actions follow its state", () => {
    const mine = (state) => task("a", "g", state, { controllable: true, pausable: true });
    assert.deepEqual(actionsFor(mine("pending")), ["cancel"]);
    assert.deepEqual(actionsFor(mine("running")), ["pause", "cancel"]);
    assert.deepEqual(actionsFor(mine("waiting")), ["pause", "cancel"]);
    assert.deepEqual(actionsFor(mine("paused")), ["resume", "cancel"]);
    for (const state of ["done", "failed", "cancelled"]) assert.deepEqual(actionsFor(mine(state)), []);
    assert.deepEqual(actionsFor(task("a", "g", "running")), [], "a task of another process has no controls");
  });

  test("a delegation the agent is waiting on can only be stopped", () => {
    const waitedOn = (state) => task("a", "g", state, { controllable: true });
    assert.deepEqual(actionsFor(waitedOn("running")), ["cancel"]);
    assert.deepEqual(actionsFor(waitedOn("waiting")), ["cancel"]);
  });

  test("a paused task still counts as active work", () => {
    const [group] = groupTasks([task("a", "g", "paused"), task("b", "g", "done")]);
    assert.equal(group.active, true);
    assert.equal(group.counts.paused, 1);
    assert.equal(group.finished, 1);
  });
});

describe("what an agent has been up to", () => {
  const tasks = [
    task("a", "g1", "done", { assignee: "dev", totalTokens: 100, createdAtMs: 1000, startedAtMs: 1100, finishedAtMs: 5000 }),
    task("b", "g1", "failed", { assignee: "dev", createdAtMs: 2000, startedAtMs: 2100 }),
    task("c", "g1", "running", { assignee: "dev", createdAtMs: 3000, startedAtMs: 3100 }),
    task("d", "g1", "pending", { assignee: "writer", owner: "dev", createdAtMs: 4000 }),
  ];
  const now = 5000 + 4 * 60000;

  test("counts what it was given by state, sums the tokens, counts what it delegated and finds the last move", () => {
    assert.deepEqual(activityOf(tasks, "dev"), { done: 1, failed: 1, cancelled: 0, active: 1, tokens: 100, delegated: 1, lastActiveMs: 5000 });
  });

  test("an agent that only delegated has no work of its own, and one no task involves has no activity", () => {
    assert.deepEqual(activityOf(tasks, "chief"), { done: 0, failed: 0, cancelled: 0, active: 0, tokens: 0, delegated: 3, lastActiveMs: 5000 });
    assert.equal(activityOf(tasks, "ghost"), null);
    assert.equal(activityOf([], "dev"), null);
  });

  test("a state this client does not know counts as active", () => {
    assert.equal(activityOf([task("x", "g", "some-new-state", { assignee: "dev" })], "dev").active, 1);
  });

  test("the card line says it in a few words, and leaves out what is zero", () => {
    assert.equal(activityLine(activityOf(tasks, "dev"), now), "1 concluída, 1 falhou, 1 em andamento · 100 tokens · delegou 1 · ativo há 4 min");
    assert.equal(activityLine(activityOf(tasks, "chief"), now), "delegou 3 · ativo há 4 min");
  });

  test("how long ago, in minutes, hours and days", () => {
    assert.equal(agoLabel(1000, 1000), "agora");
    assert.equal(agoLabel(0, 59 * 60000), "há 59 min");
    assert.equal(agoLabel(0, 2 * 3600000), "há 2 h");
    assert.equal(agoLabel(0, 3 * 86400000), "há 3 d");
    assert.equal(agoLabel(5000, 1000), "agora", "a clock that is behind never shows a negative time");
  });
});
