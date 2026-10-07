// P125 — the pure half of the threads (`src/protocol/threads.ts`). Run with `npm test`.

import assert from "node:assert/strict";
import { describe, test } from "node:test";
import { isAgentChannel, repliesLabel, threadsOf, visibleConversations, withMessageIds } from "../src/protocol/threads.ts";

const conversation = (id, extra = {}) => ({ id, title: id, createdAt: 1, updatedAt: 1, ...extra });
const thread = (id, parentConversation, messageId, replies) => conversation(id, { parent: { conversationId: parentConversation, messageId }, replies });

describe("the list of conversations", () => {
  test("leaves the threads out: they show from the message they came from", () => {
    const list = [conversation("main"), thread("side", "main", "m1", 2), conversation("other")];
    assert.deepEqual(visibleConversations(list).map((c) => c.id), ["main", "other"]);
    assert.deepEqual(visibleConversations([]), []);
  });

  test("leaves the channels of the agents out (P121): they have a tab of their own", () => {
    const list = [conversation("main"), conversation("channel-00ff00ff00ff00ff"), conversation("channeling")];
    assert.deepEqual(visibleConversations(list).map((c) => c.id), ["main", "channeling"], "only the hub's prefix with the dash counts");
    assert.equal(isAgentChannel("channel-00ff00ff00ff00ff"), true);
    assert.equal(isAgentChannel("agents-00ff"), false, "an agent-to-agent conversation is not a channel");
  });
});

describe("the threads of a conversation", () => {
  test("are found by the message they came from, with their reply count", () => {
    const list = [conversation("main"), thread("t1", "main", "m1", 2), thread("t2", "main", "m3", 1), thread("t3", "other", "m1", 9)];
    assert.deepEqual(threadsOf(list, "main"), { m1: { conversationId: "t1", replies: 2 }, m3: { conversationId: "t2", replies: 1 } });
    assert.deepEqual(threadsOf(list, "none"), {});
  });

  test("a message with two threads (a race between two devices) keeps the one with more replies", () => {
    const list = [thread("a", "main", "m1", 1), thread("b", "main", "m1", 4), thread("c", "main", "m1", 2)];
    assert.deepEqual(threadsOf(list, "main"), { m1: { conversationId: "b", replies: 4 } });
  });

  test("a thread that has no reply count counts none", () => {
    assert.deepEqual(threadsOf([conversation("t", { parent: { conversationId: "main", messageId: "m1" } })], "main"), { m1: { conversationId: "t", replies: 0 } });
  });

  test("the chip says one reply or several", () => {
    assert.equal(repliesLabel(1), "1 resposta");
    assert.equal(repliesLabel(3), "3 respostas");
  });
});

describe("the ids of the messages on screen", () => {
  const entry = (role, id) => ({ role, content: role, ...(id && { id }) });

  test("go to the messages that just arrived, skipping errors the hub does not keep", () => {
    const shown = [entry("user", "a"), entry("error"), entry("user"), entry("assistant")];
    const next = withMessageIds(shown, [{ id: "a" }, { id: "b" }, { id: "c" }]);
    assert.deepEqual(next.map((e) => e.id), ["a", undefined, "b", "c"]);
  });

  test("leave the list alone when the counts differ (a turn is on the way)", () => {
    const shown = [entry("user"), entry("assistant"), entry("user")];
    assert.equal(withMessageIds(shown, [{ id: "a" }, { id: "b" }]), shown);
  });
});
