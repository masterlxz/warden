// P125 end to end: a thread is a chat of its own from one message of a conversation. Against a real hub whose conversations are seeded on
// disk: the list leaves the thread out and opens the conversation, the message it came from shows "1 resposta" and opens the thread, a
// message without one offers to start it, and the first reply travels with `threadOf` and a conversation id of its own. The hub's model is
// a fake key, so no turn gets an answer here (the error lands in the thread, not in the chat); what the model sees inside a thread is the
// hub's own test (`warden-bootstrap`). Run with `npm run test:e2e` (see `settings.test.mjs` for what it needs).

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

const message = (id, role, content, createdAt) => ({ id, role, content, createdAt, attachments: [], generatedFiles: [] });

const MAIN = JSON.stringify({
  id: "main",
  title: "Geografia",
  messages: [message("m1", "user", "Qual a capital da França?", 1000), message("m2", "assistant", "Paris.", 1001)],
  createdAt: 1000,
  updatedAt: 1001,
});

// The most recently changed conversation of all, so a page that opened "the first of the list" would open it by mistake.
const THREAD = JSON.stringify({
  id: "side",
  title: "Conversa à parte da Itália",
  messages: [message("s1", "user", "E a da Itália?", 2000), message("s2", "assistant", "Roma.", 2001)],
  createdAt: 2000,
  updatedAt: 9000,
  parent: { conversationId: "main", messageId: "m2" },
});

const file = (id) => `warden/conversations-server/root/${id}.json`;

async function withHub(body) {
  const hub = await startHub({ files: { [file("main")]: MAIN, [file("side")]: THREAD } });
  let context;
  try {
    const opened = await signIn(browser, hub);
    context = opened.context;
    await opened.page.getByText("Paris.").waitFor();
    await body(opened);
  } finally {
    await context?.close();
    await hub.stop();
  }
}

const chatFrames = (sent) => sent.filter((s) => s.includes('"type":"chat"')).map((s) => JSON.parse(s));
const panel = (page) => page.getByLabel("Thread", { exact: true });

describe("threads in the chat", () => {
  test("the list opens the conversation and leaves the thread out; the message it came from shows its replies", { timeout: TIMEOUT }, async () => {
    await withHub(async ({ page }) => {
      const list = await page.locator("aside.conversations").innerText();
      assert.match(list, /Geografia/);
      assert.doesNotMatch(list, /Conversa à parte da Itália/, "a thread is not listed with the conversations");
      assert.equal(await page.getByText("Qual a capital da França?").count(), 1, "and the conversation is what opened, though the thread is newer");

      await page.getByRole("button", { name: "1 resposta" }).click();
      await panel(page).waitFor();
      const text = await panel(page).innerText();
      assert.match(text, /Paris\./, "the message it came from is at the top");
      assert.match(text, /E a da Itália\?/);
      assert.match(text, /Roma\./);

      await page.getByRole("button", { name: "Fechar a thread" }).click();
      assert.equal(await panel(page).count(), 0);
    });
  });

  test("a message with no thread offers one, and the first reply carries what it is a thread of", { timeout: TIMEOUT }, async () => {
    await withHub(async ({ page, sent }) => {
      await page.getByRole("button", { name: "Responder em thread" }).first().click();
      await panel(page).waitFor();
      assert.match(await panel(page).innerText(), /Qual a capital da França\?/, "the panel opens on the message it was asked on");

      await panel(page).getByRole("textbox").fill("e a da Espanha?");
      await panel(page).getByRole("button", { name: "Enviar" }).click();
      for (let waited = 0; waited < 5000 && chatFrames(sent).length === 0; waited += 100) await new Promise((r) => setTimeout(r, 100));
      const [frame] = chatFrames(sent);
      assert.equal(frame?.message, "e a da Espanha?");
      assert.deepEqual(frame?.threadOf, { conversationId: "main", messageId: "m1" });
      assert.ok(frame?.conversationId && frame.conversationId !== "main" && frame.conversationId !== "side", "a conversation id of its own");

      // The fake model fails: the error belongs to the thread, and the conversation it came from stays as it was.
      await panel(page).locator(".bubble--error").waitFor({ timeout: 15_000 });
      assert.equal(await page.locator(".chat-pane .bubble--error").count(), 0);
    });
  });
});
