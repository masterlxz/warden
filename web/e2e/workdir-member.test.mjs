// P102 end to end, the member's side: the owner makes a person and gives them a folder in Pessoas → "Pastas de
// trabalho", then that person signs in with their own user and password and can browse only what was given. As in
// `workdir.test.mjs` the model is a fake key, so what is proved is what the pages show and send; that the hub refuses a
// folder outside the list on every turn is the hub's own tests (`workdir.rs`).
// Run with `npm run test:e2e` (see `settings.test.mjs` for what it needs).

import assert from "node:assert/strict";
import path from "node:path";
import { after, before, describe, test } from "node:test";
import { PAIRING_KEY, launchBrowser, signIn, startHub } from "./harness.mjs";

const TIMEOUT = 120_000;

let browser;
before(async () => {
  browser = await launchBrowser();
});
after(async () => {
  await browser?.close();
});

const SEED = {
  "work/allowed/inner/a.txt": "a",
  "work/allowed/other/b.txt": "b",
  "work/secret/s.txt": "not for the member",
};

const chatFrames = (sent) => sent.filter((s) => s.includes('"type":"chat"')).map((s) => JSON.parse(s));
const picker = (page) => page.getByRole("dialog", { name: "Escolher a pasta de trabalho" });
const entries = async (page) => (await picker(page).locator(".folder-picker-item").allInnerTexts()).map((t) => t.trim());

/** The pairing key every change in Pessoas asks for, typed into the form whose button is `submit` (the recovery policy
 * form on the same screen asks for it too). */
const typeKey = (page, submit) =>
  page
    .locator("form.settings-confirm", { has: page.getByRole("button", { name: submit, exact: true }) })
    .getByLabel("Chave de pareamento do hub")
    .fill(PAIRING_KEY);

/** The owner's page: makes `ana` and returns her temporary password. */
async function createAna(page) {
  await page.getByRole("button", { name: "Pessoas" }).click();
  await page.getByRole("button", { name: "Adicionar pessoa" }).click();
  await page.getByLabel("Usuário").fill("ana");
  await page.getByLabel("Nome", { exact: true }).fill("Ana Souza");
  await typeKey(page, "Criar");
  await page.getByRole("button", { name: "Criar" }).click();
  const shown = page.locator(".settings-confirm", { hasText: "Senha provisória de" });
  await shown.waitFor();
  const password = (await shown.locator("code").innerText()).trim();
  await shown.getByRole("button", { name: "Já anotei" }).click();
  return password;
}

/** The owner gives `ana` these folders (one per line). */
async function giveFolders(page, folders) {
  const ana = page.locator("li", { hasText: "Ana Souza" });
  await ana.getByRole("button", { name: "Pastas de trabalho" }).click();
  await page.getByLabel("Pastas do computador do hub que Ana Souza pode escolher").fill(folders.join("\n"));
  await typeKey(page, "Salvar pastas");
  await page.getByRole("button", { name: "Salvar pastas" }).click();
  await ana.getByText(`${folders.length} pasta${folders.length === 1 ? "" : "s"} de trabalho`).waitFor();
}

/** A new page, signed in as `ana` with her own user and password, past the forced change of password. */
async function signInAsAna(hub, temporary) {
  const context = await browser.newContext({ viewport: { width: 1100, height: 1400 } });
  const page = await context.newPage();
  const sent = [];
  page.on("websocket", (ws) => ws.on("framesent", (f) => typeof f.payload === "string" && sent.push(f.payload)));
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await page.goto(hub.url);
  await page.getByRole("tab", { name: "Usuário" }).click();
  await page.getByLabel("Usuário").fill("ana");
  await page.getByLabel("Senha").fill(temporary);
  await page.getByRole("button", { name: "Entrar" }).click();

  await page.getByText("troque a senha provisória por uma sua").waitFor();
  await page.getByLabel("Senha provisória").fill(temporary);
  await page.getByLabel("Senha nova", { exact: true }).fill("a-new-password-123");
  await page.getByLabel("Repita a senha nova").fill("a-new-password-123");
  await page.getByRole("button", { name: "Trocar senha" }).click();

  // An encrypted vault shows the recovery code once before anything else; this hub may or may not have one.
  const newConversation = page.getByRole("button", { name: "+ Nova conversa" });
  const saved = page.getByLabel(/anotei|guardei|salvei/i);
  await Promise.race([newConversation.waitFor(), saved.waitFor()]);
  if (await saved.count()) {
    await saved.check();
    await page.getByRole("button", { name: /continuar|concluir|pronto/i }).click();
    await newConversation.waitFor();
  }
  return { page, sent, context, errors };
}

describe("a member's working folder", () => {
  test("the owner gives one folder and the member can browse only that one", { timeout: TIMEOUT }, async () => {
    const hub = await startHub({ files: SEED });
    const home = path.dirname(hub.config);
    const allowed = path.join(home, "work", "allowed");
    const owner = await signIn(browser, hub);
    let member;
    try {
      await owner.page.getByRole("button", { name: "+ Nova conversa" }).waitFor();
      const temporary = await createAna(owner.page);
      await giveFolders(owner.page, [allowed]);

      member = await signInAsAna(hub, temporary);
      const { page, sent } = member;
      await page.getByRole("button", { name: "+ Nova conversa" }).click();
      await page.getByRole("button", { name: "Nenhuma", exact: true }).click();
      await picker(page).waitFor();

      // At the top there is no path to pick, only her folders; the folder next to it is not among them.
      assert.equal((await picker(page).locator(".folder-picker-here").innerText()).trim(), "Suas pastas");
      assert.equal(await picker(page).getByRole("button", { name: "Usar esta pasta" }).isDisabled(), true, "nothing to use at the top");
      assert.deepEqual(await entries(page), ["allowed"]);
      assert.equal(await picker(page).getByRole("button", { name: "secret" }).count(), 0);

      await picker(page).getByRole("button", { name: "allowed", exact: true }).click();
      await picker(page).getByRole("button", { name: "inner", exact: true }).waitFor();
      assert.deepEqual(await entries(page), ["↑ Subir", "inner", "other"]);
      await picker(page).getByRole("button", { name: "inner", exact: true }).click();
      await picker(page).getByRole("button", { name: "↑ Subir" }).waitFor();
      await picker(page).getByRole("button", { name: "Usar esta pasta" }).click();

      await page.getByRole("textbox").last().fill("o que há aqui?");
      await page.getByRole("button", { name: "Enviar" }).click();
      for (let waited = 0; waited < 5000 && chatFrames(sent).length === 0; waited += 100) await new Promise((r) => setTimeout(r, 100));
      assert.equal(chatFrames(sent)[0]?.workdir, path.join(allowed, "inner"), "the first message carries the folder she picked");
      assert.deepEqual(member.errors, [], "her page raised no error");
    } finally {
      assert.deepEqual(owner.errors, [], "the owner's page raised no error");
      await member?.context.close();
      await owner.context.close();
      await hub.stop();
    }
  });

  test("a member with no folder has nothing to pick, and the page says so instead of failing", { timeout: TIMEOUT }, async () => {
    const hub = await startHub({ files: SEED });
    const owner = await signIn(browser, hub);
    let member;
    try {
      await owner.page.getByRole("button", { name: "+ Nova conversa" }).waitFor();
      const temporary = await createAna(owner.page);

      member = await signInAsAna(hub, temporary);
      await member.page.getByRole("button", { name: "+ Nova conversa" }).click();
      await member.page.getByRole("button", { name: "Nenhuma", exact: true }).click();
      await picker(member.page).waitFor();
      assert.deepEqual(await entries(member.page), []);
      await picker(member.page).getByText("Nenhuma subpasta.").waitFor();
      assert.equal(await picker(member.page).getByRole("button", { name: "Usar esta pasta" }).isDisabled(), true);
      assert.deepEqual(member.errors, []);
    } finally {
      await member?.context.close();
      await owner.context.close();
      await hub.stop();
    }
  });
});
