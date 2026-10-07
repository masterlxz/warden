// P125 — the pure half of the threads of a hub's conversations (`src/lib/threads.ts`). Run with `npm test`.

import assert from "node:assert/strict";
import { describe, test } from "node:test";
import { isAgentChannel, repliesLabel, replyCount, THREAD_CONTEXT_MAX, threadAnchor, threadHistory, threadsOf, visibleConversations } from "../src/lib/threads.ts";

const conversation = (id, extra = {}) => ({ id, title: id, messages: [], createdAt: 1, updatedAt: 1, ...extra });
const thread = (id, parentConversation, messageId, replies) => conversation(id, { parent: { conversationId: parentConversation, messageId }, replies });

describe("the list of conversations", () => {
  test("leaves the threads out: they show from the message they came from", () => {
    const list = [conversation("main"), thread("side", "main", "m1", 2), conversation("other")];
    assert.deepEqual(visibleConversations(list).map((c) => c.id), ["main", "other"]);
    assert.deepEqual(visibleConversations([]), []);
  });

  test("leaves the channels of the agents out (P121): they have a screen of their own", () => {
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
    assert.equal(repliesLabel(1), "1 reply");
    assert.equal(repliesLabel(3), "3 replies");
  });

  test("only a message the hub gave an id can start one", () => {
    assert.equal(threadAnchor({ id: "c1:0", role: "user", content: "x", createdAt: 1 }), null);
    assert.equal(threadAnchor({ id: "170001", hubId: "170001", role: "user", content: "x", createdAt: 1 }), "170001");
  });
});

describe("threads on this computer", () => {
  const message = (id, role = "user") => ({ id, role, content: id, createdAt: 1 });

  test("every saved message can start one, by its own id", () => {
    assert.equal(threadAnchor(message("m1"), true), "m1");
    assert.equal(threadAnchor(message("m1"), false), null);
    assert.equal(threadAnchor({ ...message("m1"), hubId: "h1" }, true), "h1");
  });

  test("the replies are the messages the person sent, when the list has no count", () => {
    const messages = [message("a"), message("b", "assistant"), message("c")];
    assert.equal(replyCount(conversation("t", { messages })), 2);
    assert.equal(replyCount(conversation("t", { messages, replies: 5 })), 5);
    const list = [conversation("main"), conversation("t", { parent: { conversationId: "main", messageId: "m1" }, messages })];
    assert.deepEqual(threadsOf(list, "main"), { m1: { conversationId: "t", replies: 2 } });
  });

  test("the model sees the conversation up to the message, then the thread", () => {
    const main = conversation("main", { messages: ["a", "b", "c", "d"].map((id) => message(id)) });
    const own = [message("t1"), message("t2", "assistant")];
    assert.deepEqual(threadHistory(main, "b", own).map((m) => m.id), ["a", "b", "t1", "t2"]);
    assert.deepEqual(threadHistory(main, "ghost", own).map((m) => m.id), ["t1", "t2"], "a message that isn't there gives no context");
  });

  test("keeps only the last messages that end at the one it came from", () => {
    const many = Array.from({ length: THREAD_CONTEXT_MAX + 10 }, (_, i) => message(`m${i}`));
    const main = conversation("main", { messages: many });
    const seen = threadHistory(main, `m${many.length - 1}`, []);
    assert.equal(seen.length, THREAD_CONTEXT_MAX);
    assert.equal(seen[0].id, `m${many.length - THREAD_CONTEXT_MAX}`);
    assert.equal(threadHistory(main, "m4", []).length, 5, "near the start it keeps all of them");
  });
});
