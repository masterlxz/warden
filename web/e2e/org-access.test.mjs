// P120 end to end: the workspace has one organization of agents, and the owner says in Pessoas → "Organograma" what each member does with it.
// A member with `view` sees the tree (cargo and superior, no powers) and no buttons to change it; with `edit` they change it without any
// pairing key. Against a real hub: what is proved is what the pages show and send, and what lands in the owner's config.toml. Run with
// `npm run test:e2e` (see `settings.test.mjs` for what it needs).

import assert from "node:assert/strict";
import fs from "node:fs";
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

/** The owner gives `ana` this access to the organization (the radio's text starts with the label). */
async function giveAccess(page, label) {
  const ana = page.locator("li", { hasText: "Ana Souza" });
  await ana.getByRole("button", { name: "Organograma" }).click();
  await page.getByRole("radio", { name: new RegExp(`^${label}`) }).check();
  await typeKey(page, "Salvar acesso");
  await page.getByRole("button", { name: "Salvar acesso" }).click();
}

/** A new page, signed in as `ana` past the forced change of password. */
async function signInAsAna(hub, temporary, password) {
  const context = await browser.newContext({ viewport: { width: 1100, height: 1400 } });
  const page = await context.newPage();
  const sent = [];
  page.on("websocket", (ws) => ws.on("framesent", (f) => typeof f.payload === "string" && sent.push(f.payload)));
  await page.goto(hub.url);
  await page.getByRole("tab", { name: "Usuário" }).click();
  await page.getByLabel("Usuário").fill("ana");
  await page.getByLabel("Senha").fill(password ?? temporary);
  await page.getByRole("button", { name: "Entrar" }).click();
  if (password === undefined) {
    await page.getByText("troque a senha provisória por uma sua").waitFor();
    await page.getByLabel("Senha provisória").fill(temporary);
    await page.getByLabel("Senha nova", { exact: true }).fill("a-new-password-123");
    await page.getByLabel("Repita a senha nova").fill("a-new-password-123");
    await page.getByRole("button", { name: "Trocar senha" }).click();
  }
  const newConversation = page.getByRole("button", { name: "+ Nova conversa" });
  const saved = page.getByLabel(/anotei|guardei|salvei/i);
  await Promise.race([newConversation.waitFor(), saved.waitFor()]);
  if (await saved.count()) {
    await saved.check();
    await page.getByRole("button", { name: /continuar|concluir|pronto/i }).click();
    await newConversation.waitFor();
  }
  return { page, sent, context };
}

const MEMBER_PASSWORD = "a-new-password-123";
const hasOrgTab = (page) => page.getByRole("button", { name: "Organização", exact: true }).count();

describe("the organization for a member", () => {
  test("none shows no tab, view shows the tree without a way to change it, edit changes it with no pairing key", { timeout: TIMEOUT }, async () => {
    const hub = await startHub();
    const owner = await signIn(browser, hub);
    const members = [];
    try {
      await owner.page.getByRole("button", { name: "+ Nova conversa" }).waitFor();
      const temporary = await createAna(owner.page);

      // Before the owner says anything, she has no tab for it.
      const first = await signInAsAna(hub, temporary);
      members.push(first);
      assert.equal(await hasOrgTab(first.page), 0, "none is what every member had before");

      // `view`.
      await giveAccess(owner.page, "Só vê");
      await owner.page.locator("li", { hasText: "Ana Souza" }).getByText("organograma: só vê").waitFor();
      const viewer = await signInAsAna(hub, temporary, MEMBER_PASSWORD);
      members.push(viewer);
      await viewer.page.getByRole("button", { name: "Organização", exact: true }).click();
      await viewer.page.getByText("Você só vê a árvore").waitFor();
      assert.deepEqual((await viewer.page.locator(".org-name").allInnerTexts()).sort(), ["ops", "writer"], "the owner's agents, by name");
      assert.equal(await viewer.page.getByRole("button", { name: "Editar", exact: true }).count(), 0);
      assert.equal(await viewer.page.getByRole("button", { name: "Remover", exact: true }).count(), 0);
      assert.equal(await viewer.page.getByRole("button", { name: "Conversar", exact: true }).count(), 0, "her chat is not with the owner's agents from here");
      assert.equal(await viewer.page.locator(".org-badge").count(), 0, "what an agent can do is not part of what she sees");

      // `edit`.
      await giveAccess(owner.page, "Vê e edita");
      await owner.page.locator("li", { hasText: "Ana Souza" }).getByText("organograma: vê e edita").waitFor();
      const editor = await signInAsAna(hub, temporary, MEMBER_PASSWORD);
      members.push(editor);
      await editor.page.getByRole("button", { name: "Organização", exact: true }).click();
      await editor.page.getByText("O dono deixou você mudar a hierarquia").waitFor();
      // The card, not the node: once `ops` reports to `writer` it sits inside `writer`'s node, whose own buttons would match too.
      const ops = editor.page.locator(".org-card", { has: editor.page.locator(".org-name", { hasText: /^ops$/ }) });
      await ops.getByRole("button", { name: "Editar", exact: true }).click();
      await editor.page.getByLabel("Cargo").fill("Operações");
      await editor.page.getByLabel("Reporta a").selectOption("writer");
      await editor.page.getByRole("button", { name: "Salvar", exact: true }).click();
      await editor.page.locator(".org-role", { hasText: "Operações" }).waitFor();
      assert.equal(await editor.page.getByLabel("Chave de pareamento do hub").count(), 0, "no pairing key is asked of her");

      const frame = editor.sent.map((s) => JSON.parse(s)).find((m) => m.type === "editAgentOrg");
      assert.ok(frame, "the page sent the edit");
      assert.equal(frame.pairingKey, undefined, "and with no key");
      const config = fs.readFileSync(hub.config, "utf8");
      assert.match(config, /role = "Operações"/);
      assert.match(config, /reports_to = "writer"/);

      // Taking it back: the hub tells her page at once, so the tree she had open goes away with the tab. (That the hub also refuses an
      // edit sent after the access is gone, whatever the page shows, is proved by `tests/org_access.rs`.)
      await giveAccess(owner.page, "Não vê");
      await owner.page.locator("li", { hasText: "Ana Souza" }).getByText("organograma: não vê").waitFor();
      await editor.page.getByRole("button", { name: "Organização", exact: true }).waitFor({ state: "detached" });
      await editor.page.getByText("O dono deixou você mudar a hierarquia").waitFor({ state: "detached" });
      assert.equal(await editor.page.locator(".org-card").count(), 0, "no tree is left on her page");
    } finally {
      for (const member of members) await member.context.close();
      await owner.context.close();
      await hub.stop();
    }
  });

  test("a change of access reaches a member who is already signed in, with no new sign-in", { timeout: TIMEOUT }, async () => {
    const hub = await startHub();
    const owner = await signIn(browser, hub);
    const members = [];
    try {
      await owner.page.getByRole("button", { name: "+ Nova conversa" }).waitFor();
      const temporary = await createAna(owner.page);
      const ana = await signInAsAna(hub, temporary);
      members.push(ana);
      assert.equal(await hasOrgTab(ana.page), 0);

      // The hub tells her as soon as the owner saves: nothing is done on her page (no focus, no click), and the tab appears.
      await giveAccess(owner.page, "Só vê");
      await owner.page.locator("li", { hasText: "Ana Souza" }).getByText("organograma: só vê").waitFor();
      await ana.page.getByRole("button", { name: "Organização", exact: true }).waitFor();

      // And goes away when the owner takes it back.
      await giveAccess(owner.page, "Não vê");
      await owner.page.locator("li", { hasText: "Ana Souza" }).getByText("organograma: não vê").waitFor();
      await ana.page.getByRole("button", { name: "Organização", exact: true }).waitFor({ state: "detached" });

      // A change made outside the hub's screens (the file) is not pushed: the page finds it when it comes back to the front.
      const config = fs.readFileSync(hub.config, "utf8");
      fs.writeFileSync(hub.config, config.replace(/org_access = "[a-z]+"\n?/, "").replace(/(\[\[users\]\]\n(?:.*\n)*?id = "ana"\n)/, '$1org_access = "view"\n'));
      await ana.page.evaluate(() => window.dispatchEvent(new Event("focus")));
      await ana.page.getByRole("button", { name: "Organização", exact: true }).waitFor();
    } finally {
      for (const member of members) await member.context.close();
      await owner.context.close();
      await hub.stop();
    }
  });
});
