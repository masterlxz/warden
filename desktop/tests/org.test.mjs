// P120 — the pure half of the organization view (`src/lib/org.ts`). Run with `npm test`.

import assert from "node:assert/strict";
import { describe, test } from "node:test";
import { addReportEdit, buildOrg, descendantsOf, moveEdit, orgAccessOf, positionEdit, removeFromOrg, renameInReports, superiorChoices } from "../src/lib/org.ts";

const agent = (id, reportsTo = null, role = null) => ({ id, reportsTo, role });
const ids = (nodes) => nodes.map((n) => n.agent.id);

describe("the organization tree", () => {
  test("puts the ones with no superior at the top, in order, with their reports under them", () => {
    const tree = buildOrg([agent("dev", "boss"), agent("boss"), agent("solo"), agent("qa", "boss"), agent("intern", "dev")]);
    assert.deepEqual(ids(tree), ["boss", "solo"]);
    assert.deepEqual(ids(tree[0].children), ["dev", "qa"]);
    assert.deepEqual(ids(tree[0].children[0].children), ["intern"]);
  });

  test("shows an agent whose superior is gone at the top, and never loops on a circle", () => {
    assert.deepEqual(ids(buildOrg([agent("orphan", "ghost"), agent("other")])), ["orphan", "other"]);
    assert.deepEqual(ids(buildOrg([agent("a", "b"), agent("b", "a"), agent("c")])), ["c"]);
  });

  test("an empty list is an empty tree", () => {
    assert.deepEqual(buildOrg([]), []);
  });
});

describe("editing the reporting lines", () => {
  test("an agent can't be made to report to anyone below it", () => {
    const agents = [agent("boss"), agent("lead", "boss"), agent("dev", "lead"), agent("other")];
    assert.deepEqual([...descendantsOf(agents, "boss")].sort(), ["dev", "lead"]);
    assert.deepEqual([...descendantsOf(agents, "lead")], ["dev"]);
    assert.deepEqual([...descendantsOf(agents, "other")], []);
  });

  test("a rename is followed by the reports", () => {
    const next = renameInReports([agent("boss"), agent("dev", "boss")], "boss", "chief");
    assert.equal(next[1].reportsTo, "chief");
    const same = [agent("boss")];
    assert.equal(renameInReports(same, "boss", "boss"), same);
  });

  test("removing an agent hands its reports to its superior", () => {
    const next = removeFromOrg([agent("boss"), agent("lead", "boss"), agent("dev", "lead")], "lead");
    assert.deepEqual(next.map((a) => [a.id, a.reportsTo]), [["boss", null], ["dev", "boss"]]);
    const top = removeFromOrg([agent("boss"), agent("dev", "boss")], "boss");
    assert.deepEqual(top.map((a) => [a.id, a.reportsTo]), [["dev", null]]);
  });
});

describe("editing the tree", () => {
  test("an agent can be moved under anyone but itself and the agents below it", () => {
    const team = [agent("boss"), agent("lead", "boss"), agent("dev", "lead"), agent("solo")];
    assert.deepEqual(superiorChoices(team, "lead").map((a) => a.id), ["boss", "solo"]);
    assert.deepEqual(superiorChoices(team, "solo").map((a) => a.id), ["boss", "lead", "dev"]);
    assert.deepEqual(superiorChoices(team, "boss").map((a) => a.id), ["solo"]);
  });

  test("a blank role or superior is left out of the edit and the rest is trimmed", () => {
    assert.deepEqual(positionEdit("dev", "  ", ""), { kind: "setPosition", id: "dev" });
    assert.deepEqual(positionEdit("dev", " Backend ", "lead"), { kind: "setPosition", id: "dev", role: "Backend", reportsTo: "lead" });
    assert.deepEqual(addReportEdit(" reviewer ", " Reviews code. ", "", "lead"), { kind: "addReport", id: "reviewer", persona: "Reviews code.", reportsTo: "lead" });
    assert.deepEqual(addReportEdit("top", "Leads.", "Chief", null), { kind: "addReport", id: "top", persona: "Leads.", role: "Chief" });
  });
});

describe("dragging a card onto another", () => {
  const team = [agent("boss", null, "CTO"), agent("lead", "boss", "Lead"), agent("dev", "lead"), agent("solo")];

  test("moves the agent under the target and keeps the role it already has", () => {
    assert.deepEqual(moveEdit(team, "dev", "boss"), { kind: "setPosition", id: "dev", reportsTo: "boss" });
    assert.deepEqual(moveEdit(team, "lead", "solo"), { kind: "setPosition", id: "lead", role: "Lead", reportsTo: "solo" });
  });

  test("dropping on the top takes the agent out from under its superior", () => {
    assert.deepEqual(moveEdit(team, "lead", null), { kind: "setPosition", id: "lead", role: "Lead" });
  });

  test("there is nothing to change when the target is itself, a report of it, or already its superior", () => {
    assert.equal(moveEdit(team, "lead", "lead"), null);
    assert.equal(moveEdit(team, "boss", "dev"), null, "that would close a circle");
    assert.equal(moveEdit(team, "boss", "lead"), null);
    assert.equal(moveEdit(team, "dev", "lead"), null, "it already reports to the target");
    assert.equal(moveEdit(team, "solo", null), null, "it is already at the top");
    assert.equal(moveEdit(team, "ghost", "boss"), null);
    assert.equal(moveEdit(team, "dev", "ghost"), null);
  });
});

describe("what a member may do with the organization", () => {
  test("view and edit are taken as they come; anything else, and nothing at all, is none", () => {
    assert.equal(orgAccessOf("view"), "view");
    assert.equal(orgAccessOf("edit"), "edit");
    assert.equal(orgAccessOf(undefined), "none");
    assert.equal(orgAccessOf(null), "none");
    assert.equal(orgAccessOf(""), "none");
    assert.equal(orgAccessOf("admin"), "none", "a level this app doesn't know never opens the screen");
  });
});
