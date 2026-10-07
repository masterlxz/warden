// P125 — the pure half of the threads of a hub's conversations (`src/lib/threads.ts`). Run with `npm test`.

import assert from "node:assert/strict";
import { describe, test } from "node:test";
import { repliesLabel, threadAnchor, threadsOf, visibleConversations } from "../src/lib/threads.ts";

const conversation = (id, extra = {}) => ({ id, title: id, messages: [], createdAt: 1, updatedAt: 1, ...extra });
const thread = (id, parentConversation, messageId, replies) => conversation(id, { parent: { conversationId: parentConversation, messageId }, replies });

describe("the list of conversations", () => {
  test("leaves the threads out: they show from the message they came from", () => {
    const list = [conversation("main"), thread("side", "main", "m1", 2), conversation("other")];
    assert.deepEqual(visibleConversations(list).map((c) => c.id), ["main", "other"]);
    assert.deepEqual(visibleConversations([]), []);
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
    assert.equal(repliesLabel(1), "1 reply");
    assert.equal(repliesLabel(3), "3 replies");
  });

  test("only a message the hub gave an id can start one", () => {
    assert.equal(threadAnchor({ id: "c1:0", role: "user", content: "x", createdAt: 1 }), null);
    assert.equal(threadAnchor({ id: "170001", hubId: "170001", role: "user", content: "x", createdAt: 1 }), "170001");
  });
});
