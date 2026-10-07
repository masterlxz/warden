// P120, P123 — what the extension reads of the hub's settings and of the delegated tasks (`src/protocol/messages.ts`, `lib/modelPolicies.ts`).
// Run with `npm test`.

import assert from "node:assert/strict";
import { describe, test } from "node:test";
import { decode } from "../src/protocol/messages.ts";
import { delegationSummary } from "../src/sidepanel/lib/modelPolicies.ts";

const settings = (agents, extra = {}) => ({ agents, ...extra });

describe("decoding the hub's settings", () => {
  test("keeps the agent ids the chat selector uses and reads the organization", () => {
    const reply = decode(
      JSON.stringify({
        type: "settings",
        requestId: 3,
        settings: settings(
          [
            { id: "chief", role: "CTO", canDelegateToAgents: true, canManageAgents: true, autonomy: 3, approvalRequired: ["spend_money"], delegationModels: ["fast"] },
            { id: "dev", reportsTo: "chief" },
          ],
          { modelPolicies: [{ id: "fast", model: "gpt-mini", description: "quick" }] },
        ),
      }),
    );
    assert.equal(reply.type, "settings");
    assert.deepEqual(reply.agentIds, ["chief", "dev"]);
    assert.deepEqual(reply.agents[0], {
      id: "chief",
      role: "CTO",
      reportsTo: null,
      canDelegateToAgents: true,
      canManageAgents: true,
      canMessageAgents: false,
      canManageTasks: false,
      autonomy: 3,
      approvalRequired: ["spend_money"],
      delegationModels: ["fast"],
    });
    assert.equal(reply.agents[1].reportsTo, "chief");
    assert.deepEqual(reply.modelPolicies, [{ id: "fast", model: "gpt-mini", description: "quick" }]);
  });

  test("fills what an older hub leaves out: no policies, autonomy 4, an open model choice", () => {
    const reply = decode(JSON.stringify({ type: "settings", requestId: 1, settings: settings([{ id: "solo" }]) }));
    assert.deepEqual(reply.modelPolicies, []);
    assert.equal(reply.agents[0].autonomy, 4);
    assert.deepEqual(reply.agents[0].delegationModels, []);
    assert.deepEqual(reply.agents[0].approvalRequired, []);
  });

  test("an organization edit is answered with the agents as they are now", () => {
    const reply = decode(JSON.stringify({ type: "settingsSaved", requestId: 9, version: "v2", settings: settings([{ id: "a", reportsTo: "b" }, { id: "b" }]) }));
    assert.equal(reply.type, "settingsSaved");
    assert.equal(reply.requestId, 9);
    assert.deepEqual(reply.agents.map((a) => a.id), ["a", "b"]);
  });

  test("a refused key comes through as authRejected", () => {
    const reply = decode(JSON.stringify({ type: "settingsError", requestId: 2, message: "wrong key", conflict: false, authRejected: true }));
    assert.equal(reply.type, "settingsError");
    assert.equal(reply.authRejected, true);
  });
});

describe("decoding the delegated tasks", () => {
  test("a task list keeps its tasks as the hub sent them", () => {
    const task = { id: "at-1", group: "g", assignee: "dev", objective: "x", channel: "cli", state: "running", createdAtMs: 5, controllable: true, pausable: true };
    const reply = decode(JSON.stringify({ type: "agentTaskList", requestId: 4, tasks: [task] }));
    assert.equal(reply.type, "agentTaskList");
    assert.deepEqual(reply.tasks, [task]);
  });

  test("a task error says whether the key was the problem", () => {
    const reply = decode(JSON.stringify({ type: "taskError", requestId: 5, message: "not running here", authRejected: false }));
    assert.equal(reply.type, "taskError");
    assert.equal(reply.authRejected, false);
  });
});

describe("what a model limit says", () => {
  test("open, dictated and limited", () => {
    assert.match(delegationSummary(undefined), /^Aberto/);
    assert.match(delegationSummary([]), /^Aberto/);
    assert.equal(delegationSummary(["fast"]), "Ditado: toda tarefa que ele delega roda em fast.");
    assert.match(delegationSummary(["fast", "deep"]), /^Limitado a 2 modelos; fast é o que/);
  });
});
