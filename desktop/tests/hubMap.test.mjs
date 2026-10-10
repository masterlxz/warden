// P102 phase 2 — the pure half of using a hub from the native screens (`src/lib/hubMap.ts`). Run with `npm test`
// (Node strips the types, so the same file the app builds is the one under test).

import assert from "node:assert/strict";
import { describe, test } from "node:test";
import {
  HubTurnError,
  agentFromHub,
  chatMessage,
  conversationFromSummary,
  decorateLastAnswer,
  expectReply,
  expectVaultReply,
  mergeConversations,
  messagesFromHistory,
  projectFromHub,
  turnFromReply,
  usageFromReport,
} from "../src/lib/hubMap.ts";

const summary = (id, updatedAt, extra = {}) => ({ id, title: `title ${id}`, createdAt: 1, updatedAt, ...extra });
const message = (id, role = "user") => ({ id, role, content: id, createdAt: 1 });

describe("the conversation list", () => {
  test("a summary becomes a conversation with only the fields the hub gave", () => {
    assert.deepEqual(conversationFromSummary(summary("c1", 5)), { id: "c1", title: "title c1", messages: [], createdAt: 1, updatedAt: 5 });
    const full = conversationFromSummary(summary("c2", 6, { agentId: "poet", projectId: "tax", workdir: "/srv/work" }));
    assert.equal(full.agentId, "poet");
    assert.equal(full.projectId, "tax");
    assert.equal(full.workdir, "/srv/work");
    for (const key of ["agentId", "projectId", "workdir"]) assert.equal(key in conversationFromSummary(summary("c3", 1, { [key]: "" })), false, `an empty ${key} is not carried`);
  });

  test("a refreshed list keeps the messages already loaded, is newest first, and drops what the hub no longer lists", () => {
    const loaded = { ...conversationFromSummary(summary("a", 1)), messages: [message("a:0")] };
    const gone = conversationFromSummary(summary("deleted", 9));
    const merged = mergeConversations([loaded, gone], [summary("b", 3), summary("a", 7, { title: "renamed" })]);
    assert.deepEqual(merged.map((c) => c.id), ["a", "b"], "newest first, and the deleted one is gone");
    assert.equal(merged[0].title, "renamed", "the hub's copy wins");
    assert.deepEqual(merged[0].messages.map((m) => m.id), ["a:0"], "but what was loaded stays");
    assert.deepEqual(merged[1].messages, []);
  });

  test("a conversation this screen just started stays on top until the hub lists it", () => {
    const fresh = { ...conversationFromSummary(summary("new", 100)), messages: [message("m")] };
    const merged = mergeConversations([fresh], [summary("old", 5)], new Set(["new"]));
    assert.deepEqual(merged.map((c) => c.id), ["new", "old"]);
    const listedNow = mergeConversations([fresh], [summary("old", 5), summary("new", 120)], new Set(["new"]));
    assert.deepEqual(listedNow.map((c) => c.id), ["new", "old"]);
    assert.equal(listedNow.filter((c) => c.id === "new").length, 1, "never twice");
    assert.deepEqual(mergeConversations([fresh], [summary("old", 5)]).map((c) => c.id), ["old"], "not asked to keep it, so it goes");
  });
});

describe("a conversation's history", () => {
  test("messages get an id from the conversation and the position, the same on every load", () => {
    const history = [
      { role: "user", content: "hi", createdAt: 10 },
      { role: "assistant", content: "hello", createdAt: 11, attachments: [{ mimeType: "image/png", data: "AAAA" }] },
      { role: "user", content: "and?", createdAt: 12, attachments: [] },
    ];
    const first = messagesFromHistory("c1", history);
    assert.deepEqual(first.map((m) => m.id), ["c1:0", "c1:1", "c1:2"]);
    assert.deepEqual(messagesFromHistory("c1", history), first);
    assert.deepEqual(first[1].attachments, [{ mimeType: "image/png", data: "AAAA" }]);
    assert.equal("attachments" in first[0], false);
    assert.equal("attachments" in first[2], false, "an empty list is left out");
    assert.deepEqual(messagesFromHistory("c1", []), []);
  });

  test("a turn's tokens and reserve notice go on the last answer, and only on an answer", () => {
    const extras = { usage: { promptTokens: 1, completionTokens: 2, totalTokens: 3 }, fallbacks: [{ from: "a", to: "b", model: "m", reason: "down" }] };
    const done = decorateLastAnswer([message("c:0"), message("c:1", "assistant")], extras);
    assert.deepEqual(done[1].usage, extras.usage);
    assert.deepEqual(done[1].fallbacks, extras.fallbacks);
    assert.equal("usage" in done[0], false);
    const ends = [message("c:0"), message("c:1", "assistant"), message("c:2")];
    assert.equal(decorateLastAnswer(ends, extras), ends, "the last message is the user's: nothing to decorate");
    assert.deepEqual(decorateLastAnswer([], extras), []);
    const plain = [message("c:0", "assistant")];
    assert.equal(decorateLastAnswer(plain, {}), plain);
  });
});

describe("a turn's reply", () => {
  test("an answer, without the fields the hub left empty", () => {
    assert.deepEqual(turnFromReply({ type: "chatResponse", content: "ahoy", usage: null, attachments: [], fallbacks: [] }), { content: "ahoy" });
    const usage = { promptTokens: 1, completionTokens: 1, totalTokens: 2 };
    const fallbacks = [{ from: "a", to: "b", model: "m", reason: "r" }];
    assert.deepEqual(turnFromReply({ type: "chatResponse", content: "x", usage, fallbacks }), { content: "x", usage, fallbacks });
  });

  test("an error is thrown with the hub's words and the limit it stopped on", () => {
    assert.throws(() => turnFromReply({ type: "chatError", message: "mock failure" }), (e) => e instanceof HubTurnError && e.message === "mock failure" && e.spendLimitId === undefined);
    assert.throws(() => turnFromReply({ type: "chatError", message: "limit", spendLimitId: "day" }), (e) => e instanceof HubTurnError && e.spendLimitId === "day");
    assert.throws(() => turnFromReply({ type: "conversationList" }), /something else/);
    assert.throws(() => turnFromReply(null), /something else/);
  });

  test("a refusal reply is an error and any other kind is refused", () => {
    const list = { type: "conversationList", requestId: 1, conversations: [] };
    assert.equal(expectReply(list, "conversationList"), list);
    assert.throws(() => expectReply({ type: "conversationError", requestId: 1, message: "no such conversation" }, "conversationOk"), /no such conversation/);
    assert.throws(() => expectReply({ type: "projectError", message: "id taken" }, "projectList"), /id taken/);
    assert.throws(() => expectReply({ type: "pong" }, "projectList"), /something else/);
    assert.throws(() => expectReply(undefined, "projectList"), /something else/);
  });
});

describe("agents and projects", () => {
  test("an agent keeps what the picker reads, with the opt-ins defaulting to off", () => {
    const agent = agentFromHub({ id: "poet", persona: "You write.", providerId: "", canDelegateToAgents: false, canManageAgents: true, allowedTools: null });
    assert.deepEqual(agent, { id: "poet", persona: "You write.", providerId: "", canDelegateToAgents: false, canManageAgents: true, canMessageAgents: false, canManageTasks: false, canStartTasks: true, canCreateWorkers: true, canMessageUser: true, canChooseModels: true, allowedTools: null, autonomy: 4, approvalRequired: [] });
    assert.deepEqual(agentFromHub({ id: "a", persona: "", providerId: "", canDelegateToAgents: false, canManageAgents: false, allowedTools: null, approvalRequired: ["critical_infra"] }).approvalRequired, ["critical_infra"]);
    assert.deepEqual(agentFromHub({ id: "a", persona: "", providerId: "p", canDelegateToAgents: false, canManageAgents: false, allowedTools: ["shell"], sharedWith: ["ana"] }).sharedWith, ["ana"]);
  });

  test("an off permission to start background work or create workers stays off, and a hub that says nothing means on", () => {
    const base = { id: "a", persona: "", providerId: "", canDelegateToAgents: false, canManageAgents: false, allowedTools: null };
    const off = agentFromHub({ ...base, canStartTasks: false, canCreateWorkers: false });
    assert.deepEqual([off.canStartTasks, off.canCreateWorkers], [false, false], "saving the form must not switch them back on");
    const silent = agentFromHub(base);
    assert.deepEqual([silent.canStartTasks, silent.canCreateWorkers], [true, true]);
  });

  test("an off permission to message the user or choose models stays off, and a hub that says nothing means on", () => {
    const base = { id: "a", persona: "", providerId: "", canDelegateToAgents: false, canManageAgents: false, allowedTools: null };
    const off = agentFromHub({ ...base, canMessageUser: false, canChooseModels: false });
    assert.deepEqual([off.canMessageUser, off.canChooseModels], [false, false]);
    const silent = agentFromHub(base);
    assert.deepEqual([silent.canMessageUser, silent.canChooseModels], [true, true]);
  });

  test("a project is read as is, and is not a code project unless the hub says so", () => {
    assert.deepEqual(projectFromHub({ id: "tax", name: "Tax", description: "d", instructions: "i" }), { id: "tax", name: "Tax", description: "d", instructions: "i", code: false });
    const code = projectFromHub({ id: "app", name: "App", description: "", instructions: "", workdir: "/srv/app", code: true });
    assert.equal(code.code, true);
    assert.equal(code.workdir, "/srv/app");
  });
});

describe("the chat message", () => {
  const base = { content: "hello", attachments: [], conversationId: "c1", agentId: "", projectId: "", workdir: "", creating: false };

  test("carries the conversation and the text, and no model or history", () => {
    assert.deepEqual(chatMessage(base), { type: "chat", message: "hello", conversationId: "c1", attachments: [] });
    assert.deepEqual(chatMessage({ ...base, agentId: "poet" }).agentId, "poet");
  });

  test("a project or a folder travels only with the message that creates the conversation, and never both", () => {
    assert.equal("projectId" in chatMessage({ ...base, projectId: "tax" }), false, "an existing conversation keeps its own");
    assert.equal("workdir" in chatMessage({ ...base, workdir: "/srv" }), false);
    assert.equal(chatMessage({ ...base, creating: true, projectId: "tax" }).projectId, "tax");
    assert.equal(chatMessage({ ...base, creating: true, workdir: "/srv/work" }).workdir, "/srv/work");
    const both = chatMessage({ ...base, creating: true, projectId: "tax", workdir: "/srv/work" });
    assert.equal(both.projectId, "tax");
    assert.equal("workdir" in both, false, "a project has its own folder");
  });
});

describe("the hub's usage report", () => {
  const total = { promptTokens: 7, completionTokens: 3, totalTokens: 10 };

  test("feeds the token tiles and the spending panels, with no split by agent or provider", () => {
    const limit = { id: "daily" };
    const recent = { windowHours: 24, byModel: [], byChannel: [], byProvider: [], byAgent: [], byPerson: [] };
    const { summary, spend } = usageFromReport({ total, conversationCount: 2, messageCount: 5, limitsEnabled: true, limits: [limit], recent, ledgerError: "disk full" });
    assert.deepEqual(summary, { conversationCount: 2, messageCount: 5, total, byAgent: [], byProvider: [] });
    assert.deepEqual(spend, { limitsEnabled: true, limits: [limit], recent, ledgerError: "disk full" });
  });

  test("a hub with the limits off has no recent spending and no ledger error", () => {
    const { spend } = usageFromReport({ total, conversationCount: 0, messageCount: 0, limitsEnabled: false, limits: [] });
    assert.deepEqual(spend, { limitsEnabled: false, limits: [], recent: null, ledgerError: null });
  });
});

describe("a vault reply", () => {
  test("a vaultError keeps the conflict flag, which the screen turns into reload or overwrite", () => {
    assert.throws(() => expectVaultReply({ type: "vaultError", requestId: 1, message: "changed", conflict: true }, "vaultSaved"), { message: "changed", conflict: true });
    assert.throws(() => expectVaultReply({ type: "vaultError", requestId: 1, message: "no such note" }, "vaultNote"), { message: "no such note", conflict: false });
  });

  test("the expected reply passes, another refusal is still an error", () => {
    const ok = { type: "vaultSaved", requestId: 1, version: "v2" };
    assert.equal(expectVaultReply(ok, "vaultSaved"), ok);
    assert.throws(() => expectVaultReply({ type: "settingsError", message: "no" }, "vaultSaved"), /no/);
  });
});

describe("threads (P125)", () => {
  test("a message the hub gave an id is known by it, and keeps the position id as a fallback for a hub that gives none", () => {
    const [saved, old] = messagesFromHistory("c1", [
      { id: "170001", role: "user", content: "hi", createdAt: 1 },
      { role: "assistant", content: "hello", createdAt: 2 },
    ]);
    assert.equal(saved.id, "170001");
    assert.equal(saved.hubId, "170001", "what a thread starts from");
    assert.equal(old.id, "c1:1");
    assert.equal("hubId" in old, false, "no thread can start from a message the hub has no id for");
  });

  test("a thread's summary carries the message it came from and its replies, and an ordinary one carries none", () => {
    const link = { conversationId: "main", messageId: "170002" };
    const side = conversationFromSummary(summary("side", 5, { parent: link, replies: 3 }));
    assert.deepEqual(side.parent, link);
    assert.equal(side.replies, 3);
    assert.equal(conversationFromSummary(summary("t", 5, { parent: link })).replies, 0, "a hub that sends no count counts none");
    assert.equal("parent" in conversationFromSummary(summary("plain", 5)), false);
    assert.equal("replies" in conversationFromSummary(summary("plain", 5)), false);
  });

  test("the turn that creates a thread says what it is a thread of, and not its project or folder (the hub uses the conversation's)", () => {
    const base = { content: "hi", attachments: [], conversationId: "t1", agentId: "poet", projectId: "tax", workdir: "/srv", creating: true, threadOf: { conversationId: "main", messageId: "170002" } };
    const first = chatMessage(base);
    assert.deepEqual(first.threadOf, { conversationId: "main", messageId: "170002" });
    assert.equal("projectId" in first || "workdir" in first, false);
    assert.equal(first.agentId, "poet", "it talks to the same agent");
    assert.equal("threadOf" in chatMessage({ ...base, creating: false }), false, "a thread that exists keeps its message");
    assert.equal("threadOf" in chatMessage({ ...base, threadOf: undefined }), false, "an ordinary turn has none");
  });
});
