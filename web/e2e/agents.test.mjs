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

/** The id the hub gives an agent's channel (`channel_id` in `message_agent.rs`: FNV-1a, 64 bits, of the name), to seed a conversation on disk. */
function channelId(agent) {
  let hash = 0xcbf29ce484222325n;
  for (const byte of Buffer.from(agent)) {
    hash ^= BigInt(byte);
    hash = (hash * 0x100000001b3n) & 0xffffffffffffffffn;
  }
  return `channel-${hash.toString(16).padStart(16, "0")}`;
}
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

  test("a message an agent left in its channel while the page was closed shows as unread until the channel is opened", { timeout: TIMEOUT }, async () => {
    const message = { id: "m1", role: "assistant", content: "O disco está cheio", createdAt: 5000, attachments: [], generatedFiles: [] };
    const channel = JSON.stringify({ id: channelId("writer"), title: "writer", messages: [message], createdAt: 5000, updatedAt: 5000, agentId: "writer" });
    const hub = await startHub({ files: { [`warden/conversations-server/root/${channelId("writer")}.json`]: channel } });
    let context;
    try {
      const opened = await signIn(browser, hub);
      context = opened.context;
      const { page } = opened;
      await page.getByRole("button", { name: "Agents", exact: true }).waitFor();
      // A browser that has been here before and last saw nothing of this channel: the mark is what the earlier visit left.
      await page.evaluate(() => localStorage.setItem("warden.channelSeen", "{}"));
      await page.reload();

      const tab = page.getByRole("button", { name: /^Agents/ });
      await tab.locator(".tab-badge").waitFor();
      assert.equal(await tab.locator(".tab-badge").innerText(), "1");

      await tab.click();
      await contacts(page).locator(".unread-dot").waitFor();
      assert.match(await contacts(page).getByRole("button", { name: /writer/ }).innerText(), /writer/);
      assert.equal(await contacts(page).getByRole("button", { name: /ops/ }).locator(".unread-dot").count(), 0, "only the channel that changed");

      await contacts(page).getByRole("button", { name: /writer/ }).click();
      await page.getByText("O disco está cheio").waitFor();
      await tab.locator(".tab-badge").waitFor({ state: "detached" });
      assert.equal(await contacts(page).locator(".unread-dot").count(), 0, "read once it is open");

      // The mark is kept: after a reload it is still read.
      await page.reload();
      await page.getByRole("button", { name: /^Agents/ }).waitFor();
      assert.equal(await page.locator(".tab-badge").count(), 0);
    } finally {
      await context?.close();
      await hub.stop();
    }
  });
});
