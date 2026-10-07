// P121 — the pure half of the agents' permission to start messages (`src/lib/outreach.ts`). Run with `npm test`.

import assert from "node:assert/strict";
import { describe, test } from "node:test";
import { dropOutreach, outreachForward, outreachOn, renameOutreach, setForward, setOutreach } from "../src/lib/outreach.ts";

const entry = (agent, ...forward) => ({ agent, forward });

describe("turning an agent on and off", () => {
  test("turning on starts with no external channel, and does nothing twice", () => {
    const on = setOutreach([], "pirate", true);
    assert.deepEqual(on, [entry("pirate")]);
    assert.equal(setOutreach(on, "pirate", true), on);
    assert.equal(outreachOn(on, "pirate"), true);
    assert.equal(outreachOn(on, "chef"), false);
  });

  test("turning off forgets its channels", () => {
    const list = [entry("pirate", "telegram"), entry("chef")];
    assert.deepEqual(setOutreach(list, "pirate", false), [entry("chef")]);
    assert.deepEqual(outreachForward(setOutreach(list, "pirate", false), "pirate"), []);
  });
});

describe("the external channels", () => {
  test("each is switched alone, in a fixed order, without repeats", () => {
    let list = [entry("pirate")];
    list = setForward(list, "pirate", "whatsapp", true);
    list = setForward(list, "pirate", "telegram", true);
    list = setForward(list, "pirate", "telegram", true);
    assert.deepEqual(outreachForward(list, "pirate"), ["telegram", "whatsapp"]);
    list = setForward(list, "pirate", "telegram", false);
    assert.deepEqual(outreachForward(list, "pirate"), ["whatsapp"]);
  });

  test("an agent that may not start messages gets no channel", () => {
    const list = [entry("chef")];
    assert.equal(setForward(list, "pirate", "telegram", true).length, 1);
    assert.deepEqual(outreachForward(setForward(list, "pirate", "telegram", true), "pirate"), []);
  });
});

describe("renaming and removing an agent", () => {
  test("the entry follows the new name, and the others stay", () => {
    const list = [entry("pirate", "telegram"), entry("chef")];
    assert.deepEqual(renameOutreach(list, "pirate", "captain"), [entry("captain", "telegram"), entry("chef")]);
    assert.equal(renameOutreach(list, "pirate", "pirate"), list);
  });

  test("the entry goes with the agent", () => {
    assert.deepEqual(dropOutreach([entry("pirate"), entry("chef")], "pirate"), [entry("chef")]);
  });
});
