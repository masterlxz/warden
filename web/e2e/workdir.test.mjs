// P102 end to end: a conversation in no project picks a folder of the hub's machine before its first message, in the
// folder browser the hub feeds (folders only), and the first `chat` frame carries it. The hub's model is a fake key, so
// no turn gets an answer here: what is proved is what the page shows and sends. That the turn then runs in the folder,
// and who may pick which folder, is the hub's own tests (`workdir.rs`).
// Run with `npm run test:e2e` (see `settings.test.mjs` for what it needs).

import assert from "node:assert/strict";
import path from "node:path";
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

const projectFile = `---\nname: Declaração\ndescription: O imposto deste ano\n---\nResponda em português.\n`;

/** The hub's own folder is where its owner starts browsing (HOME there), so the tree to pick from goes in it. */
const SEED = {
  "work/alpha/a.txt": "a",
  "work/beta/inner/b.txt": "b",
  "work/notes.txt": "a file, which the browser never lists",
  "vault/projects/tax/PROJECT.md": projectFile,
};

async function withHub(body) {
  const hub = await startHub({ files: SEED });
  const home = path.dirname(hub.config);
  let context;
  try {
    const opened = await signIn(browser, hub);
    context = opened.context;
    await opened.page.getByRole("button", { name: "+ Nova conversa" }).waitFor();
    await body({ ...opened, hub, home });
    assert.deepEqual(opened.errors, [], "the page raised no error");
  } finally {
    await context?.close();
    await hub.stop();
  }
}

const chatFrames = (sent) => sent.filter((s) => s.includes('"type":"chat"')).map((s) => JSON.parse(s));
const folderButton = (page) => page.getByRole("button", { name: "Nenhuma", exact: true });
const browser_ = (page) => page.getByRole("dialog", { name: "Escolher a pasta de trabalho" });
const entries = async (page) => (await browser_(page).locator(".folder-picker-item").allInnerTexts()).map((t) => t.trim());

describe("the chat's working folder", () => {
  test("browses folders only, goes down and back up, and sends the one picked with the first message", { timeout: TIMEOUT }, async () => {
    await withHub(async ({ page, sent, home }) => {
      await page.getByRole("button", { name: "+ Nova conversa" }).click();
      await folderButton(page).click();
      await browser_(page).waitFor();
      assert.equal((await browser_(page).locator(".folder-picker-here").innerText()).trim(), home, "the owner starts at the hub's home");
      assert.equal(await browser_(page).getByRole("button", { name: "Usar esta pasta" }).isDisabled(), false);

      await browser_(page).getByRole("button", { name: "work", exact: true }).click();
      await browser_(page).getByRole("button", { name: "alpha", exact: true }).waitFor();
      assert.deepEqual(await entries(page), ["↑ Subir", "alpha", "beta"], "folders only: notes.txt is not there");

      await browser_(page).getByRole("button", { name: "beta", exact: true }).click();
      await browser_(page).getByRole("button", { name: "inner", exact: true }).waitFor();
      await browser_(page).getByRole("button", { name: "↑ Subir" }).click();
      await browser_(page).getByRole("button", { name: "alpha", exact: true }).waitFor();

      await browser_(page).getByRole("button", { name: "alpha", exact: true }).click();
      await browser_(page).getByRole("button", { name: "↑ Subir" }).waitFor();
      assert.equal((await browser_(page).locator(".folder-picker-here").innerText()).trim(), path.join(home, "work", "alpha"));
      await browser_(page).getByRole("button", { name: "Usar esta pasta" }).click();
      await browser_(page).waitFor({ state: "detached" });
      await page.getByRole("button", { name: "alpha", exact: true }).waitFor();

      await page.getByRole("textbox").last().fill("organize esta pasta");
      await page.getByRole("button", { name: "Enviar" }).click();
      for (let waited = 0; waited < 5000 && chatFrames(sent).length === 0; waited += 100) await new Promise((r) => setTimeout(r, 100));
      const [frame] = chatFrames(sent);
      assert.equal(frame?.workdir, path.join(home, "work", "alpha"), "the first message carries the folder it starts in");
      assert.equal(frame.projectId, undefined, "and no project");

      // Once the conversation exists the folder is only shown: no button to pick another.
      await page.locator(".folder-chip", { hasText: "alpha" }).waitFor();
      assert.equal(await page.getByRole("button", { name: "alpha", exact: true }).count(), 0, "it can't be changed after the first message");
    });
  });

  test("cancelling and clearing leave no folder, and a conversation without one sends none", { timeout: TIMEOUT }, async () => {
    await withHub(async ({ page, sent }) => {
      await page.getByRole("button", { name: "+ Nova conversa" }).click();
      await folderButton(page).click();
      await browser_(page).getByRole("button", { name: "Cancelar" }).click();
      await folderButton(page).waitFor();

      await folderButton(page).click();
      await browser_(page).getByRole("button", { name: "work", exact: true }).click();
      await browser_(page).getByRole("button", { name: "Usar esta pasta" }).click();
      await page.getByRole("button", { name: "work", exact: true }).waitFor();
      await page.getByRole("button", { name: "Tirar a pasta" }).click();
      await folderButton(page).waitFor();

      await page.getByRole("textbox").last().fill("oi");
      await page.getByRole("button", { name: "Enviar" }).click();
      for (let waited = 0; waited < 5000 && chatFrames(sent).length === 0; waited += 100) await new Promise((r) => setTimeout(r, 100));
      const [frame] = chatFrames(sent);
      assert.equal(frame?.message, "oi");
      assert.equal(frame.workdir, undefined, "no folder on it");
    });
  });

  test("a project and a folder are never both", { timeout: TIMEOUT }, async () => {
    await withHub(async ({ page, sent }) => {
      await page.getByRole("button", { name: "+ Nova conversa" }).click();
      await folderButton(page).click();
      await browser_(page).getByRole("button", { name: "work", exact: true }).click();
      await browser_(page).getByRole("button", { name: "Usar esta pasta" }).click();
      await page.getByRole("button", { name: "work", exact: true }).waitFor();

      // Choosing a project takes the folder away, and the folder picker with it: the project has its own.
      await page.getByRole("combobox", { name: "Projeto" }).selectOption("tax");
      assert.equal(await page.getByRole("button", { name: "work", exact: true }).count(), 0);
      assert.equal(await folderButton(page).count(), 0, "no folder to pick inside a project");

      await page.getByRole("textbox").last().fill("o que falta?");
      await page.getByRole("button", { name: "Enviar" }).click();
      for (let waited = 0; waited < 5000 && chatFrames(sent).length === 0; waited += 100) await new Promise((r) => setTimeout(r, 100));
      const [frame] = chatFrames(sent);
      assert.equal(frame?.projectId, "tax");
      assert.equal(frame.workdir, undefined, "the folder did not travel with the project");
    });
  });
});
