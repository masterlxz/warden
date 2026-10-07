// P120 end to end: dragging a card of the organization tree onto another changes its superior, with the pairing key, against a real
// hub whose config is seeded on disk. What is proved is what travels (`editAgentOrg` with `setPosition`, the role kept) and what the hub
// writes; a drop on a card that would close a circle does nothing at all. Run with `npm run test:e2e` (see `settings.test.mjs` for what
// it needs). Not covered: the drop zone of the top of the tree, which only exists while a card is being dragged.

import assert from "node:assert/strict";
import fs from "node:fs";
import { after, before, describe, test } from "node:test";
import { launchBrowser, PAIRING_KEY, signIn, startHub } from "./harness.mjs";

const TIMEOUT = 120_000;

let browser;
before(async () => {
  browser = await launchBrowser();
});
after(async () => {
  await browser?.close();
});

// The base config has `writer` and `ops`; `intern` reports to `writer`, and `ops` has a role to see it kept.
const TREE = `
[[agents]]
id = "intern"
persona = "You help."
reports_to = "writer"
`;

/** Runs `body` against a hub with the tree, signed in, on the Organização screen. */
async function withTree(body) {
  const hub = await startHub({ extraConfig: TREE });
  let context;
  try {
    const opened = await signIn(browser, hub);
    context = opened.context;
    await opened.page.getByRole("button", { name: "Organização" }).click();
    await opened.page.locator(".org-card", { hasText: "intern" }).waitFor();
    await body({ ...opened, hub });
  } finally {
    await context?.close();
    await hub.stop();
  }
}

const card = (page, name) => page.locator(".org-card").filter({ has: page.locator(".org-name", { hasText: new RegExp(`^${name}$`) }) });
const edits = (sent) => sent.filter((s) => s.includes('"editAgentOrg"')).map((s) => JSON.parse(s));

describe("dragging a card in the organization tree", () => {
  test("dropping a card on another moves it under it, after the pairing key", { timeout: TIMEOUT }, async () => {
    await withTree(async ({ page, sent, hub }) => {
      await card(page, "ops").dragTo(card(page, "writer"));
      await page.getByLabel("Chave de pareamento do hub").fill(PAIRING_KEY);
      await page.getByRole("button", { name: "Confirmar" }).click();
      await page.locator(".org-children .org-name", { hasText: /^ops$/ }).waitFor();

      const [edit] = edits(sent);
      assert.deepEqual(edit.edit, { kind: "setPosition", id: "ops", reportsTo: "writer" });
      assert.equal(edit.pairingKey, PAIRING_KEY);
      const saved = fs.readFileSync(hub.config, "utf8");
      assert.match(saved, /id = "ops"[\s\S]*?reports_to = "writer"/, "the hub wrote the new superior");
    });
  });

  test("a card cannot be dropped under one of its own reports: no prompt, nothing sent", { timeout: TIMEOUT }, async () => {
    await withTree(async ({ page, sent }) => {
      await card(page, "writer").dragTo(card(page, "intern"));
      // Give a wrongly accepted drop the time to show its prompt.
      await page.waitForTimeout(500);
      assert.equal(await page.getByLabel("Chave de pareamento do hub").count(), 0);
      assert.deepEqual(edits(sent), []);
    });
  });
});
