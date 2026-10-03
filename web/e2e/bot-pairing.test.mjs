// P117 end to end: the owner approves a Telegram or WhatsApp pairing request from the web's settings, as themselves
// or as a member of the workspace. Run with `npm run test:e2e` (see `settings.test.mjs` for what it needs).

import assert from "node:assert/strict";
import fs from "node:fs";
import { after, before, describe, test } from "node:test";
import { launchBrowser, openSettings, PAIRING_KEY, startHub } from "./harness.mjs";

const TIMEOUT = 120_000;
const IN_AN_HOUR = Math.floor(Date.now() / 1000) + 3600;

/** A workspace with two members, the bots linked to the hub as Ana only, and two strangers waiting. */
const WORKSPACE = {
  extraConfig: `
[bot_hub]
url = "ws://127.0.0.1:7420"

[[users]]
id = "ana"
name = "Ana"
password_hash = "not-a-real-hash"

[[users]]
id = "bia"
name = "Bia"
password_hash = "not-a-real-hash"
`,
  files: {
    "bot_hub.json": JSON.stringify({ members: { ana: { device_id: "warden-bot-ana", device_token: "t" } } }),
    "bot_pairing.json": JSON.stringify([
      { channel: "telegram", sender: "42", label: "ana", code: "ABCD2345", expires_at: IN_AN_HOUR },
      { channel: "whatsapp", sender: "5511999999999@lid", label: "", code: "WXYZ6789", expires_at: IN_AN_HOUR },
    ]),
  },
};

let browser;
before(async () => {
  browser = await launchBrowser();
});
after(async () => {
  await browser?.close();
});

/** Runs `body` with a hub and the signed-in settings page, and always cleans both up. */
async function withPairings(body) {
  const hub = await startHub(WORKSPACE);
  let context;
  try {
    const opened = await openSettings(browser, hub);
    context = opened.context;
    await body({ hub, ...opened });
    assert.deepEqual(opened.errors, [], "the page raised no error");
  } finally {
    await context?.close();
    await hub.stop();
  }
}

/** The `resolveBotPairing` messages the page sent, parsed. */
const resolves = (sent) => sent.filter((s) => s.includes('"resolveBotPairing"')).map((s) => JSON.parse(s));

/** The pairing key prompt (its label names the code and, for a member, who the chat will speak as), then Confirmar. */
async function confirmWithKey(page, label) {
  await page.getByLabel(label).fill(PAIRING_KEY);
  await page.getByRole("button", { name: "Confirmar" }).click();
}

const telegramRequest = (page) => page.locator("li.settings-card", { hasText: "ABCD-2345" });
const whatsappRequest = (page) => page.locator("li.settings-card", { hasText: "WXYZ-6789" });

describe("approving a pairing request from the web", () => {
  test("only a member the bots are linked to can be chosen, and the owner is the default", { timeout: TIMEOUT }, async () => {
    await withPairings(async ({ page }) => {
      const select = telegramRequest(page).locator("select");
      assert.equal(await select.inputValue(), "", "the owner is the default");
      // Playwright's `isDisabled()` ignores an <option>, so the property is read off the element itself.
      const disabled = (member) => select.locator(`option[value="${member}"]`).evaluate((option) => option.disabled);
      assert.equal(await disabled("ana"), false, "Ana is linked, so she can be chosen");
      assert.equal(await disabled("bia"), true, "Bia isn't linked, so she can't");
      assert.match(await select.locator('option[value="bia"]').innerText(), /vincule antes/);
      // What really stops an unlinked member is the hub, which refuses the approval whole (tested in
      // `crates/warden-server/tests/people.rs`): a script can set a disabled option's value, a person can't.
    });
  });

  test("approving as a linked member sends the member and maps the chat to them", { timeout: TIMEOUT }, async () => {
    await withPairings(async ({ hub, page, sent }) => {
      const request = telegramRequest(page);
      await request.locator("select").selectOption("ana");
      await request.getByRole("button", { name: "Aprovar" }).click();
      await confirmWithKey(page, /para aprovar ABCD-2345 como Ana/);
      await request.waitFor({ state: "detached" });

      const [resolve] = resolves(sent);
      assert.equal(resolve.approve, true);
      assert.equal(resolve.code, "ABCD-2345");
      assert.equal(resolve.member, "ana", "the choice travelled to the hub");
      const file = fs.readFileSync(hub.config, "utf8");
      assert.match(file, /allowed_users = \[42\]/, "the chat is let in");
      assert.match(file, /\[telegram\.members\]\s*\n42 = "ana"/, "and speaks as Ana");
      assert.equal(await whatsappRequest(page).count(), 1, "the other request is still waiting");
    });
  });

  test("approving without choosing, and denying, map nobody to a member", { timeout: TIMEOUT }, async () => {
    await withPairings(async ({ hub, page, sent }) => {
      const whatsapp = whatsappRequest(page);
      await whatsapp.getByRole("button", { name: "Aprovar" }).click();
      await confirmWithKey(page, /para aprovar WXYZ-6789$/);
      await whatsapp.waitFor({ state: "detached" });
      assert.equal("member" in resolves(sent).at(-1), false, "no member chosen, none sent");
      let file = fs.readFileSync(hub.config, "utf8");
      assert.ok(file.includes("5511999999999@lid"), "the chat is let in as the owner's");
      assert.ok(!file.includes("[whatsapp.members]"), "and mapped to nobody");

      // Denying never carries the member, even with one picked.
      const telegram = telegramRequest(page);
      await telegram.locator("select").selectOption("ana");
      await telegram.getByRole("button", { name: "Recusar" }).click();
      await confirmWithKey(page, /para recusar ABCD-2345$/);
      await telegram.waitFor({ state: "detached" });
      const denial = resolves(sent).at(-1);
      assert.deepEqual([denial.approve, "member" in denial], [false, false]);
      file = fs.readFileSync(hub.config, "utf8");
      assert.ok(!/allowed_users = \[42\]/.test(file) && !file.includes("[telegram.members]"), "denied: nobody let in, nobody mapped");
    });
  });
});
