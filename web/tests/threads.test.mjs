// P125 — the pure half of the threads (`src/hub/threads.ts`). Run with `npm test`.

import assert from "node:assert/strict";
import { describe, test } from "node:test";
import { repliesLabel, threadsOf, visibleConversations, withMessageIds } from "../src/hub/threads.ts";

const conversation = (id, extra = {}) => ({ id, title: id, createdAt: 1, updatedAt: 1, ...extra });
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

  test("a hub that sends no reply count counts none", () => {
    assert.deepEqual(threadsOf([conversation("t", { parent: { conversationId: "main", messageId: "m1" } })], "main"), { m1: { conversationId: "t", replies: 0 } });
  });

  test("the chip says one reply or several", () => {
    assert.equal(repliesLabel(1), "1 resposta");
    assert.equal(repliesLabel(3), "3 respostas");
  });
});

describe("the ids of the messages on screen", () => {
  const entry = (role, content, id) => ({ role, content, attachments: [], ...(id && { id }) });

  test("go onto the messages that just came, in order, and errors are skipped", () => {
    const shown = [entry("user", "hi", "a"), entry("assistant", "hello"), { role: "error", content: "boom", attachments: [] }, entry("user", "again"), entry("assistant", "again!")];
    // The error is not saved by the hub: four saved messages, four on screen besides it.
    const history = [{ id: "a" }, { id: "b" }, { id: "c" }, { id: "d" }];
    assert.deepEqual(withMessageIds(shown, history).map((e) => e.id), ["a", "b", undefined, "c", "d"]);
  });

  test("a message that already has an id keeps it, and a hub with no ids changes nothing", () => {
    const shown = [entry("user", "hi", "keep"), entry("assistant", "hello")];
    assert.deepEqual(withMessageIds(shown, [{ id: "other" }, { id: "b" }]).map((e) => e.id), ["keep", "b"]);
    assert.deepEqual(withMessageIds(shown, [{}, {}]).map((e) => e.id), ["keep", undefined]);
  });

  test("when the counts differ a turn is on its way: the list is left as it is", () => {
    const shown = [entry("user", "hi", "a"), entry("assistant", "hello"), entry("user", "and now?")];
    assert.equal(withMessageIds(shown, [{ id: "a" }, { id: "b" }]), shown);
  });
});
