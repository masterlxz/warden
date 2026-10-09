// P121 — the pure half of what an agent did outside its channel (`src/lib/agentWork.ts`). Run with `npm test`.

import assert from "node:assert/strict";
import { describe, test } from "node:test";
import { agentWork, workButtonLabel, workLabel } from "../src/lib/agentWork.ts";

const conversation = (id, title, updatedAt, extra = {}) => ({ id, title, messages: [], createdAt: 1, updatedAt, ...extra });

describe("the work of an agent", () => {
  test("has the notes it left and received, and the runs it made, newest first", () => {
    const list = [
      conversation("agents-1", "pirate → poet", 10),
      conversation("agents-2", "chief → pirate", 30),
      conversation("task-7", "Task", 20, { agentId: "pirate" }),
      conversation("task-hook-9", "Webhook", 40, { agentId: "pirate" }),
    ];
    assert.deepEqual(agentWork(list, "pirate").map((w) => [w.id, w.kind, w.other]), [
      ["task-hook-9", "run-hook", undefined],
      ["agents-2", "note-in", "chief"],
      ["task-7", "run-task", undefined],
      ["agents-1", "note-out", "poet"],
    ]);
  });

  test("leaves out other agents' work, the channels, loose conversations and threads", () => {
    const list = [
      conversation("agents-1", "chief → poet", 10),
      conversation("task-7", "Task", 20, { agentId: "poet" }),
      conversation("task-8", "Task", 20),
      conversation("channel-00ff", "pirate", 30, { agentId: "pirate" }),
      conversation("c1", "chat", 30, { agentId: "pirate" }),
      conversation("task-9", "Task", 50, { agentId: "pirate", parent: { conversationId: "task-9x", messageId: "m" } }),
    ];
    assert.deepEqual(agentWork(list, "pirate"), []);
  });

  test("a title without the arrow, or with the same agent on both sides, is not a note", () => {
    const list = [conversation("agents-1", "no arrow", 1), conversation("agents-2", "pirate → pirate", 1)];
    assert.deepEqual(agentWork(list, "pirate"), []);
  });

  test("the same time keeps a steady order, by id", () => {
    const list = [conversation("task-b", "x", 5, { agentId: "a" }), conversation("task-a", "x", 5, { agentId: "a" })];
    assert.deepEqual(agentWork(list, "a").map((w) => w.id), ["task-a", "task-b"]);
  });
});

describe("the words", () => {
  test("say who a note is to or from, and what kind of run", () => {
    const labels = [{ kind: "note-out", other: "poet" }, { kind: "note-in", other: "chief" }, { kind: "run-task" }, { kind: "run-hook" }].map(workLabel);
    assert.deepEqual(labels, ["to poet", "from chief", "scheduled task", "webhook"]);
  });

  test("the button shows the count only when there is something", () => {
    assert.equal(workButtonLabel(0), "Notes and runs");
    assert.equal(workButtonLabel(3), "Notes and runs (3)");
  });
});
