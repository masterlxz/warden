// P120 — the pure half of what a member does with the organization of the agents (`src/hub/org.ts`). Run with `npm test`.

import assert from "node:assert/strict";
import { describe, test } from "node:test";
import { ORG_ACCESS_CHOICES, orgAccessLabel, orgAccessOf } from "../src/hub/org.ts";

describe("the access to the organization", () => {
  test("is what the hub says, and anything else is none: no tab the hub would refuse", () => {
    assert.equal(orgAccessOf("view"), "view");
    assert.equal(orgAccessOf("edit"), "edit");
    for (const none of [undefined, null, "", "none", "admin", "EDIT"]) assert.equal(orgAccessOf(none), "none", String(none));
  });

  test("is offered in three levels, none first, and each has words for the owner and for the card", () => {
    assert.deepEqual(ORG_ACCESS_CHOICES.map((c) => c.value), ["none", "view", "edit"]);
    assert.ok(ORG_ACCESS_CHOICES.every((c) => c.label && c.hint));
    assert.deepEqual(["none", "view", "edit"].map(orgAccessLabel), ["organograma: não vê", "organograma: só vê", "organograma: vê e edita"]);
  });
});
