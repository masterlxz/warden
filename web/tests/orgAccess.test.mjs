// P120 — the pure half of what a member does with the organization of the agents (`src/hub/org.ts`). Run with `npm test`.

import assert from "node:assert/strict";
import { describe, test } from "node:test";
import { ORG_ACCESS_CHOICES, orgAccessLabel, orgAccessOf, userWithOrgAccess } from "../src/hub/org.ts";

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

describe("the member with the level the hub just said", () => {
  const ana = { id: "ana", name: "Ana" };

  test("takes the new level, and none takes the field away as the hub does", () => {
    assert.deepEqual(userWithOrgAccess(ana, "edit"), { id: "ana", name: "Ana", orgAccess: "edit" });
    assert.deepEqual(userWithOrgAccess({ ...ana, orgAccess: "edit" }, "view"), { id: "ana", name: "Ana", orgAccess: "view" });
    assert.deepEqual(userWithOrgAccess({ ...ana, orgAccess: "view" }, "none"), ana);
    assert.ok(!("orgAccess" in userWithOrgAccess({ ...ana, orgAccess: "view" }, "none")));
  });

  test("is the same object when nothing changes, so nothing is drawn again", () => {
    const viewer = { ...ana, orgAccess: "view" };
    assert.equal(userWithOrgAccess(viewer, "view"), viewer);
    assert.equal(userWithOrgAccess(ana, "none"), ana);
    assert.equal(userWithOrgAccess(ana, "garbage"), ana, "a level this app doesn't know is none");
  });
});
