// P123 — the pure half of the model policies and the limit on what an agent delegates with (`src/lib/modelPolicies.ts`). Run with `npm test`.

import assert from "node:assert/strict";
import { describe, test } from "node:test";
import { delegationCandidates, delegationSummary, dropModel, dropPolicy, nextPolicyId, renameModel, renamePolicy } from "../src/lib/modelPolicies.ts";

const agent = (id, delegationModels) => ({ id, delegationModels });
const form = () => ({
  agents: [agent("chief", ["reasoning", "small", "big"]), agent("worker", undefined), agent("scribe", ["fast"])],
  modelPolicies: [
    { id: "reasoning", model: "big", description: "hard problems" },
    { id: "fast", model: "small" },
  ],
});
const lists = (f) => f.agents.map((a) => a.delegationModels ?? null);

describe("what an agent may delegate with", () => {
  test("every provider, combo and policy is a candidate, blanks left out", () => {
    assert.deepEqual(delegationCandidates([{ id: "small" }, { id: "big" }], [{ id: "mix" }], [{ id: "fast" }, { id: " " }]), ["small", "big", "mix", "fast"]);
  });

  test("a renamed provider or combo reaches the policies it answers and the agents that listed it", () => {
    const next = renameModel(form(), "big", "bigger");
    assert.deepEqual(next.modelPolicies.map((p) => p.model), ["bigger", "small"]);
    assert.deepEqual(lists(next), [["reasoning", "small", "bigger"], null, ["fast"]]);
    assert.equal(renameModel(form(), "big", "big").agents[0].delegationModels[2], "big", "no change when the name is the same");
  });

  test("a renamed policy reaches the agents that listed it", () => {
    assert.deepEqual(lists(renamePolicy(form(), "fast", "quick")), [["reasoning", "small", "big"], null, ["quick"]]);
  });

  test("a removed provider takes the policies it answered, and the agents forget both", () => {
    const next = dropModel(form(), "big");
    assert.deepEqual(next.modelPolicies.map((p) => p.id), ["fast"]);
    assert.deepEqual(lists(next), [["small"], null, ["fast"]]);
    // The agent whose only model went is open again.
    assert.deepEqual(lists(dropModel(form(), "small")), [["reasoning", "big"], null, []]);
  });

  test("a removed policy is forgotten by the agents and takes nothing else", () => {
    const next = dropPolicy(form(), "reasoning");
    assert.deepEqual(next.modelPolicies.map((p) => p.id), ["fast"]);
    assert.deepEqual(lists(next), [["small", "big"], null, ["fast"]]);
  });

  test("a new policy gets the first free name and the card says what the list means", () => {
    assert.equal(nextPolicyId(["small", "policy-1", "policy-3"]), "policy-2");
    assert.equal(nextPolicyId([]), "policy-1");
    assert.match(delegationSummary(undefined), /^Open/);
    assert.match(delegationSummary([]), /^Open/);
    assert.equal(delegationSummary(["big"]), "Dictated: every task it delegates runs on big.");
    assert.match(delegationSummary(["fast", "big"]), /Limited to 2 models; fast is what a task gets/);
  });
});
