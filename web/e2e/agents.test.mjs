// P121 end to end: each agent has one conversation with the person, its channel, on its own screen. Against a real hub: the Agents screen
// lists the configured agents as contacts, opening one asks the hub for its channel id, the first message goes to that id with the agent,
// and the channel stays out of the list of loose conversations. The hub's model is a fake key, so no turn gets an answer here (the error
// lands in the channel). Run with `npm run test:e2e` (see `settings.test.mjs` for what it needs).

import assert from "node:assert/strict";
import { after, before, describe, test } from "node:test";
import { launchBrowser, signIn, startHub } from "./harness.mjs";

const TIMEOUT = 120_000;

let browser;
before(async () => {
  browser = await launchBrowser();
});
after(async () => {
  await browser?.close();
});

const chatFrames = (sent) => sent.filter((s) => s.includes('"type":"chat"')).map((s) => JSON.parse(s));
const contacts = (page) => page.getByLabel("Agentes", { exact: true });

describe("the channel of an agent", () => {
  test("the screen lists the agents, the first message goes to the channel the hub names, and the chat list leaves it out", { timeout: TIMEOUT }, async () => {
    const hub = await startHub();
    let context;
    try {
      const opened = await signIn(browser, hub);
      context = opened.context;
      const { page, sent } = opened;
      await page.getByRole("tab", { name: "Agents" }).or(page.getByRole("button", { name: "Agents", exact: true })).first().click();
      await contacts(page).getByRole("button", { name: /writer/ }).waitFor();
      assert.equal(await contacts(page).getByRole("button", { name: /ops/ }).count(), 1);

      await contacts(page).getByRole("button", { name: /writer/ }).click();
      await page.locator(".chat-title", { hasText: "writer" }).waitFor();
      const asked = sent.map((s) => JSON.parse(s)).filter((m) => m.type === "openAgentChannel");
      assert.ok(asked.some((m) => m.agentId === "writer"), "the page asked the hub for the channel of the agent");

      await page.getByRole("textbox").fill("oi, writer");
      await page.getByRole("button", { name: "Enviar" }).click();
      for (let waited = 0; waited < 5000 && chatFrames(sent).length === 0; waited += 100) await new Promise((r) => setTimeout(r, 100));
      const [frame] = chatFrames(sent);
      assert.equal(frame?.agentId, "writer");
      assert.match(frame?.conversationId ?? "", /^channel-[0-9a-f]{16}$/, "the id the hub made, not one of the page's");

      // Asking again for the same agent gives the same channel: there is only one.
      await contacts(page).getByRole("button", { name: /ops/ }).click();
      await page.locator(".chat-title", { hasText: "ops" }).waitFor();
      await contacts(page).getByRole("button", { name: /writer/ }).click();
      await page.getByText("oi, writer").first().waitFor();

      // The chat screen does not list it with the loose conversations.
      await page.getByRole("button", { name: "Chat", exact: true }).click();
      await page.locator("aside.conversations").waitFor();
      assert.doesNotMatch(await page.locator("aside.conversations").innerText(), /oi, writer/);
    } finally {
      await context?.close();
      await hub.stop();
    }
  });
});
