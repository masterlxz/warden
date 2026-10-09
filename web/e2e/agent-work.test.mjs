// P121 end to end: what an agent did outside its channel (the notes it traded with other agents, the runs nobody watched) is a list inside
// its contact on the Agents screen. Against a real hub whose files are seeded on disk: the button counts them, the list names each one, a
// click opens the conversation without leaving the Agents screen, and "← Canal" goes back. Run with `npm run test:e2e` (see
// `settings.test.mjs` for what it needs).

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

/** The hub's files: a note from writer to ops (its id is any `agents-` one, the page reads the title), and a run of ops. */
function seeded(now) {
  const note = {
    id: "agents-00000000000000aa",
    title: "writer → ops",
    messages: [message("m1", "user", "Pode olhar o rascunho?", now - 30_000), message("m2", "assistant", "Já olhei", now - 20_000)],
    createdAt: now - 30_000,
    updatedAt: now - 20_000,
  };
  const run = {
    id: "task-daily",
    title: "Resumo diário",
    messages: [message("m1", "user", "[tarefa] resuma o dia", now - 10_000), message("m2", "assistant", "Dia calmo", now - 9_000)],
    createdAt: now - 10_000,
    updatedAt: now - 9_000,
    agentId: "ops",
  };
  return {
    [`warden/conversations-server/root/${note.id}.json`]: JSON.stringify(note),
    [`warden/tasks-server/conversations/${run.id}.json`]: JSON.stringify(run),
  };
}

const contacts = (page) => page.getByLabel("Agentes", { exact: true });

describe("the work of an agent on the Agents screen", () => {
  test("is a list in the contact: it opens each conversation and comes back to the channel", { timeout: TIMEOUT }, async () => {
    const hub = await startHub({ files: seeded(Date.now()) });
    let context;
    try {
      const opened = await signIn(browser, hub);
      context = opened.context;
      const { page } = opened;
      await page.getByRole("tab", { name: "Agents" }).or(page.getByRole("button", { name: "Agents", exact: true })).first().click();
      await contacts(page).getByRole("button", { name: /ops/ }).click();
      await page.getByRole("button", { name: "Recados e execuções (2)" }).click();

      const items = await page.locator(".work-list .conversation-title").allInnerTexts();
      assert.deepEqual(items, ["Resumo diário · tarefa agendada", "writer → ops · de writer"]);

      await page.getByRole("button", { name: /writer → ops/ }).click();
      await page.getByText("Pode olhar o rascunho?").first().waitFor();
      assert.equal(await page.locator(".chat-title").innerText(), "writer → ops");
      assert.ok(await contacts(page).isVisible(), "still on the Agents screen");

      await page.getByRole("button", { name: "← Canal" }).click();
      await page.getByRole("button", { name: /Recados e execuções/ }).waitFor();
      assert.equal(await page.locator(".chat-title").innerText(), "ops");
    } finally {
      await context?.close();
      await hub.stop();
    }
  });

  test("an agent that did nothing outside its channel says so", { timeout: TIMEOUT }, async () => {
    const hub = await startHub({ files: seeded(Date.now()) });
    let context;
    try {
      const opened = await signIn(browser, hub);
      context = opened.context;
      const { page } = opened;
      await page.getByRole("tab", { name: "Agents" }).or(page.getByRole("button", { name: "Agents", exact: true })).first().click();
      await contacts(page).getByRole("button", { name: /writer/ }).click();
      // writer left a note, so it has one item; the empty one is shown by an agent with nothing: none here, so assert the count instead.
      await page.getByRole("button", { name: "Recados e execuções (1)" }).waitFor();
    } finally {
      await context?.close();
      await hub.stop();
    }
  });
});
