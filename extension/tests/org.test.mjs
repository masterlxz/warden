// P120 — the pure half of the organization view (`src/lib/org.ts`). Run with `npm test`.

import assert from "node:assert/strict";
import { describe, test } from "node:test";
import { addReportEdit, buildOrg, descendantsOf, positionEdit, removeFromOrg, renameInReports, superiorChoices } from "../src/sidepanel/lib/org.ts";

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
