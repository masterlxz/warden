// P10 end to end: the "Testar chave" button on a provider card. The hub asks a provider (here, a small server of our own
// that takes one key) for its model list; nothing is spent, so the spend ledger must come out exactly as it went in.
// Run with `npm run test:e2e` (see `settings.test.mjs` for what it needs).

import assert from "node:assert/strict";
import fs from "node:fs";
import http from "node:http";
import path from "node:path";
import { after, before, describe, test } from "node:test";
import { launchBrowser, openSettings, PAIRING_KEY, startHub } from "./harness.mjs";

const TIMEOUT = 120_000;
const GOOD_KEY = "sk-good";
const BAD_KEY = "sk-bad";

let browser;
before(async () => {
  browser = await launchBrowser();
});
after(async () => {
  await browser?.close();
});

/** An OpenAI-compatible server that lists models only for `GOOD_KEY`, and echoes the key it was sent when it refuses. */
function startProvider() {
  const requests = [];
  const server = http.createServer((req, res) => {
    requests.push({ url: req.url, authorization: req.headers.authorization });
    if (req.headers.authorization === `Bearer ${GOOD_KEY}`) {
      res.writeHead(200, { "content-type": "application/json" });
      res.end('{"data":[]}');
    } else {
      res.writeHead(401, { "content-type": "application/json" });
      res.end('{"error":"Incorrect API key provided: sk-bad"}');
    }
  });
  return new Promise((resolve) =>
    server.listen(0, "127.0.0.1", () => resolve({ url: `http://127.0.0.1:${server.address().port}/v1`, requests, close: () => new Promise((r) => server.close(r)) })),
  );
}

/** A ledger line that must come out of the whole test unchanged: testing a key books nothing. */
const LEDGER_LINE = JSON.stringify({ kind: "spend", ts: Date.now() - 3_600_000, channel: "server", user: null, agent: null, person: null, model: "m", tokens: 10, cost_usd: 0.01, provider: "main" }) + "\n";

/** Runs `body` with a provider, a hub that has it saved as `local`, and the signed-in settings page. */
async function withProvider(body) {
  const provider = await startProvider();
  const extraConfig = `
[[providers]]
id = "local"
kind = "openai_compatible"
api_key = "${GOOD_KEY}"
base_url = "${provider.url}"
model = "m"
`;
  const hub = await startHub({ extraConfig, files: { "warden/spend_ledger.jsonl": LEDGER_LINE } });
  const ledger = path.join(path.dirname(hub.config), "warden", "spend_ledger.jsonl");
  let context;
  try {
    const opened = await openSettings(browser, hub);
    context = opened.context;
    // `local` is the one provider with a base URL (the other is Gemini). Not found by its button's text: that text
    // leaves the card while the pairing key prompt is open, and a locator that stops matching can't be waited on.
    const card = opened.page.locator("li.settings-card", { hasText: "URL base" });
    await body({ ...opened, hub, provider, card, ledger, before: fs.readFileSync(ledger, "utf8") });
    assert.deepEqual(opened.errors, [], "the page raised no error");
  } finally {
    await context?.close();
    await hub.stop();
    await provider.close();
  }
}

/** Testar chave → the pairing key prompt → Testar. */
async function runTest(card, page) {
  await card.getByRole("button", { name: "Testar chave" }).click();
  await page.getByLabel("Chave de pareamento do hub, para testar a chave").fill(PAIRING_KEY);
  await card.getByRole("button", { name: "Testar", exact: true }).click();
}

const testFrames = (sent) => sent.filter((s) => s.includes('"testProvider"')).map((s) => JSON.parse(s));

describe("the Test key button", () => {
  test("tests the key saved on the hub without the page ever having it, and books nothing", { timeout: TIMEOUT }, async () => {
    await withProvider(async ({ page, sent, card, provider, ledger, before }) => {
      await runTest(card, page);
      const status = card.getByRole("status");
      await status.waitFor();
      assert.match(await status.innerText(), /✓ The provider accepted the key\./);

      const [frame] = testFrames(sent);
      assert.deepEqual(frame.provider.apiKey, { action: "keep" }, "the page asked about the saved key without holding it");
      assert.equal(frame.provider.originalId, "local");
      assert.deepEqual(provider.requests.map((r) => [r.url, r.authorization]), [["/v1/models", `Bearer ${GOOD_KEY}`]], "the hub asked the provider's model list with the saved key");
      assert.equal(fs.readFileSync(ledger, "utf8"), before, "testing a key spent nothing: the ledger is as it was");
    });
  });

  test("tests a key typed but not saved, and tells a wrong one without repeating what the provider said", { timeout: TIMEOUT }, async () => {
    await withProvider(async ({ page, sent, card, provider, ledger, before }) => {
      await card.getByRole("button", { name: "Trocar" }).click();
      await card.locator('input[type="password"]').fill(BAD_KEY);
      await runTest(card, page);
      const status = card.getByRole("status");
      await status.waitFor();
      const text = await status.innerText();
      assert.match(text, /✗ The server rejected the key\./);
      assert.ok(!text.includes(BAD_KEY) && !text.includes("Incorrect") && !text.includes("127.0.0.1"), `the answer carries no key, no provider text and no address: ${text}`);

      assert.deepEqual(testFrames(sent)[0].provider.apiKey, { action: "set", value: BAD_KEY }, "the typed key is what was sent");
      assert.equal(provider.requests.at(-1).authorization, `Bearer ${BAD_KEY}`, "and what the hub used, not the saved one");
      assert.equal(fs.readFileSync(ledger, "utf8"), before, "still nothing spent");
    });
  });

  test("refuses an address that isn't saved yet, and a wrong pairing key", { timeout: TIMEOUT }, async () => {
    await withProvider(async ({ page, card, provider }) => {
      await card.getByLabel(/URL base/).fill("http://127.0.0.1:1/v1");
      await runTest(card, page);
      await card.locator(".error-banner").waitFor();
      assert.match(await card.locator(".error-banner").innerText(), /Save the provider first/);
      assert.equal(provider.requests.length, 0, "the hub asked nobody: that address was never saved");

      // Back to the saved address, with a wrong pairing key.
      await card.getByLabel(/URL base/).fill(provider.url);
      await card.getByLabel("Chave de pareamento do hub, para testar a chave").fill("not-the-key");
      await card.getByRole("button", { name: "Testar", exact: true }).click();
      await card.locator(".error-banner", { hasText: "Chave de pareamento errada." }).waitFor();
      assert.equal(provider.requests.length, 0, "a wrong pairing key tests nothing");
    });
  });
});
