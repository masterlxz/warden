// P10 end to end: the Usage screen's dollars, split by provider, agent and person, and per day, from a spend ledger
// the hub reads when it starts. Run with `npm run test:e2e` (see `settings.test.mjs` for what it needs).

import assert from "node:assert/strict";
import { after, before, describe, test } from "node:test";
import { launchBrowser, signIn, startHub } from "./harness.mjs";

const TIMEOUT = 120_000;
const HOUR = 3_600_000;

let browser;
before(async () => {
  browser = await launchBrowser();
});
after(async () => {
  await browser?.close();
});

/** One ledger line, the way the hub writes it. The default limits keep the last 24 hours, so these are recent. */
function spend(hoursAgo, fields) {
  return JSON.stringify({ kind: "spend", ts: Date.now() - hoursAgo * HOUR, channel: "server", user: null, agent: null, person: null, ...fields });
}

/** $2.00 on `main` for the writer, $0.50 on `spare` for Ana, and one call from before the provider was kept, with no price. */
const LEDGER = [
  spend(1, { agent: "writer", model: "m", tokens: 1_000_000, cost_usd: 2.0, provider: "main" }),
  spend(2, { model: "m2", tokens: 500_000, cost_usd: 0.5, person: "ana", provider: "spare" }),
  spend(3, { model: "unpriced", tokens: 100, cost_usd: null }),
].join("\n") + "\n";

/** `US$ 2,00` → `2`: the screen writes numbers the Brazilian way. */
const dollars = (text) => Number(/US\$\s*([\d.]+(?:,\d+)?)/.exec(text)?.[1].replace(/\./g, "").replace(",", "."));

const table = (page, title) => page.locator("table.usage-table", { has: page.locator("caption", { hasText: title }) });

describe("the Usage screen", () => {
  test("shows the ledger's dollars by provider, agent and person, and per day", { timeout: TIMEOUT }, async () => {
    const hub = await startHub({ files: { "warden/spend_ledger.jsonl": LEDGER } });
    let context;
    try {
      const opened = await signIn(browser, hub);
      context = opened.context;
      const { page } = opened;
      await page.getByRole("button", { name: "Uso" }).click();
      await page.getByText(/Gasto recente/).waitFor();

      const provider = table(page, "Por provedor");
      assert.match(await provider.getByRole("row", { name: /^main/ }).innerText(), /US\$\s*2,00/, "$2.00 went to main");
      assert.match(await provider.getByRole("row", { name: /^spare/ }).innerText(), /US\$\s*0,50/, "$0.50 to spare");
      const legacy = await provider.getByRole("row", { name: /sem provedor registrado/ }).innerText();
      assert.ok(legacy.includes("—"), `a call with no price and no provider has no dollar figure: ${legacy}`);

      const agent = table(page, "Por agente");
      assert.match(await agent.getByRole("row", { name: /^writer/ }).innerText(), /US\$\s*2,00/);
      assert.ok(await agent.getByRole("row", { name: /sem agente/ }).count(), "the calls with no agent are labelled, not hidden");

      const person = table(page, "Por pessoa");
      assert.match(await person.getByRole("row", { name: /^ana/ }).innerText(), /US\$\s*0,50/);
      assert.ok(await person.getByRole("row", { name: /o dono/ }).count(), "the owner is the empty key");
      assert.match(await page.locator("body").innerText(), /“Por agente” é o agente com que o turno começou/, "the screen says what 'by agent' means");

      // Dollars per day: the three calls may fall on two calendar days, so the sum is what is certain.
      const section = page.locator("section.usage-section", { has: page.getByRole("heading", { name: /Gasto por dia/ }) });
      const labels = await section.locator(".usage-daily-slot").evaluateAll((slots) => slots.map((s) => s.getAttribute("aria-label") ?? ""));
      assert.equal(labels.length, 30, "thirty days");
      const total = labels.reduce((sum, label) => sum + (/US\$/.test(label) ? dollars(label) : 0), 0);
      const calls = labels.reduce((sum, label) => sum + Number(/em (\d+) chamadas/.exec(label)?.[1] ?? 0), 0);
      assert.equal(calls, 3, "every call is on some day");
      assert.ok(Math.abs(total - 2.5) < 1e-6, `$2.50 across the days, got ${total}`);
      assert.match(await section.innerText(), /só guarda a janela do limite mais longo/, "and it says how far back the ledger reaches");
      assert.deepEqual(opened.errors, [], "the page raised no error");
    } finally {
      await context?.close();
      await hub.stop();
    }
  });
});
