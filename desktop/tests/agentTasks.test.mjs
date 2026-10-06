// P123 — the pure half of the "Agent work" screen (`src/lib/agentTasks.ts`). Run with `npm test`.

import assert from "node:assert/strict";
import { describe, test } from "node:test";
import { durationLabel, formatTokens, groupTasks } from "../src/lib/agentTasks.ts";

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
    assert.deepEqual(group.counts, { pending: 1, running: 1, waiting: 0, done: 2, failed: 1, cancelled: 0 });
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
    const [group] = groupTasks([task("a", "g", "paused")]);
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
    assert.equal(formatTokens(12900), "12.9K");
  });

  test("the duration is empty before it starts, and counts to now while it runs", () => {
    assert.equal(durationLabel(task("a", "g", "pending"), 99999), null);
    assert.equal(durationLabel(task("a", "g", "running", { startedAtMs: 1000 }), 5000), "4s");
    assert.equal(durationLabel(task("a", "g", "done", { startedAtMs: 0, finishedAtMs: 125000 }), 999999), "2m 05s");
    assert.equal(durationLabel(task("a", "g", "done", { startedAtMs: 0, finishedAtMs: 3780000 }), 0), "1h 03m");
  });
});
