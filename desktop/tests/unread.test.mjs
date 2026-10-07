// P121 — the pure half of the unread marks of the agents' channels (`src/lib/unread.ts`). Run with `npm test`.

import assert from "node:assert/strict";
import { describe, test } from "node:test";
import { baseline, channelsOf, isUnread, markSeen, unreadIds } from "../src/lib/unread.ts";

const conversation = (id, updatedAt) => ({ id, title: id, messages: [], createdAt: 1, updatedAt });

describe("the unread channels", () => {
  test("only the channels count, not the loose conversations", () => {
    const list = [conversation("main", 5), conversation("channel-aa", 5), conversation("task-x", 5)];
    assert.deepEqual(channelsOf(list).map((c) => c.id), ["channel-aa"]);
    assert.deepEqual(unreadIds(list, {}, null), ["channel-aa"]);
  });

  test("a channel is unread when it changed after the last change shown, and is not the open one", () => {
    const channel = conversation("channel-aa", 10);
    assert.equal(isUnread(channel, { "channel-aa": 9 }, null), true);
    assert.equal(isUnread(channel, { "channel-aa": 10 }, null), false);
    assert.equal(isUnread(channel, {}, null), true, "a channel with no mark is a new one");
    assert.equal(isUnread(channel, { "channel-aa": 1 }, "channel-aa"), false, "the one being read is not unread");
  });

  test("the first run counts what is there as seen, and a channel that comes later is unread", () => {
    const seen = baseline([conversation("main", 5), conversation("channel-aa", 7)]);
    assert.deepEqual(seen, { "channel-aa": 7 });
    assert.deepEqual(unreadIds([conversation("channel-aa", 7), conversation("channel-bb", 8)], seen, null), ["channel-bb"]);
  });

  test("marking seen moves forward only, and keeps the same object when nothing moves", () => {
    const seen = { "channel-aa": 10 };
    assert.equal(markSeen(seen, conversation("channel-aa", 10)), seen);
    assert.equal(markSeen(seen, conversation("channel-aa", 4)), seen, "never backwards");
    assert.deepEqual(markSeen(seen, conversation("channel-aa", 12)), { "channel-aa": 12 });
    assert.deepEqual(markSeen(seen, conversation("channel-bb", 3)), { "channel-aa": 10, "channel-bb": 3 });
  });
});
