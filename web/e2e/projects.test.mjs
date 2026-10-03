// P103 end to end: the Projects tab (make, add a note to, remove a project), the chat's project picker, and the
// conversation list grouped by project, against a real hub whose vault and conversations are seeded on disk.
// The hub's model is a fake key, so no turn gets an answer here: what is proved is what the page shows and what it sends
// (`chat` carries `projectId`); that a turn then runs in the project's folder is the hub's own tests (`people.rs`).
// Run with `npm run test:e2e` (see `settings.test.mjs` for what it needs).

import assert from "node:assert/strict";
import fs from "node:fs";
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

/** A conversation file the way the hub saves one. `project` is where it was started, if anywhere. */
function conversation(id, title, updatedAt, project) {
  const message = (role, content, at) => ({ id: `${id}-${role}`, role, content, createdAt: at, attachments: [], generatedFiles: [] });
  return JSON.stringify({
    id,
    title,
    messages: [message("user", "hello", updatedAt - 1), message("assistant", "hi", updatedAt)],
    createdAt: updatedAt - 1,
    updatedAt,
    ...(project && { projectId: project }),
  });
}

/** The owner's conversations on a hub started with `HOME` here: `<config>/warden/conversations-server/root/`. */
const conversationFile = (id) => `warden/conversations-server/root/${id}.json`;

const projectFile = `---\nname: Declaração\ndescription: O imposto deste ano\n---\nResponda em português.\n`;

const SEED = {
  "vault/projects/tax/PROJECT.md": projectFile,
  "vault/projects/tax/jan.md": "recibos de janeiro",
  [conversationFile("in-tax")]: conversation("in-tax", "Recibos de janeiro", 3000, "tax"),
  [conversationFile("plain")]: conversation("plain", "Conversa solta", 2000),
  // Started in a project that has since been removed: listed with the others, as an ordinary conversation.
  [conversationFile("orphan")]: conversation("orphan", "Conversa de projeto removido", 1000, "gone"),
};

/** Runs `body` against a hub with `SEED`, signed in; `disk(rel)` is a path in the hub's folder. */
async function withHub(body) {
  const hub = await startHub({ files: SEED });
  const home = path.dirname(hub.config);
  let context;
  try {
    const opened = await signIn(browser, hub);
    context = opened.context;
    // The list loads after the page is ready, and what is open follows it: a test that clicks before then would be
    // racing the page's own start-up.
    await opened.page.getByText("Conversa solta").waitFor();
    await body({ ...opened, hub, disk: (rel) => path.join(home, rel) });
    assert.deepEqual(opened.errors, [], "the page raised no error");
  } finally {
    await context?.close();
    await hub.stop();
  }
}

const chatFrames = (sent) => sent.filter((s) => s.includes('"type":"chat"')).map((s) => JSON.parse(s));
const picker = (page) => page.getByRole("combobox", { name: "Projeto" });

describe("the chat and its projects", () => {
  test("lists a project's conversations under its name and fixes the project of an existing one", { timeout: TIMEOUT }, async () => {
    await withHub(async ({ page }) => {
      const group = page.locator("section.conversation-group", { has: page.getByRole("heading", { name: "Declaração" }) });
      await group.waitFor();
      assert.match(await group.innerText(), /Recibos de janeiro/, "the conversation started in the project is under it");
      assert.doesNotMatch(await group.innerText(), /Conversa solta|removido/, "and only that one");
      const loose = await page.locator("aside.conversations > ul.conversations-list").innerText();
      assert.match(loose, /Conversa solta/);
      assert.match(loose, /Conversa de projeto removido/, "one whose project is gone is listed, as an ordinary conversation");
      assert.doesNotMatch(loose, /Recibos de janeiro/, "and a conversation of a project is not listed twice");
      assert.equal(await page.locator("section.conversation-group").count(), 1, "no group for a project that doesn't exist");

      // Open conversations show the project they are in.
      await page.getByRole("button", { name: /Recibos de janeiro/ }).click();
      await picker(page).waitFor();
      assert.equal(await picker(page).inputValue(), "tax");
      await page.getByRole("button", { name: /Conversa solta/ }).click();
      assert.equal(await picker(page).inputValue(), "", "an ordinary conversation shows none");
      await page.getByRole("button", { name: /Conversa de projeto removido/ }).click();
      assert.equal(await picker(page).inputValue(), "", "a removed project shows none, not a blank entry");
    });
  });

  test("a new conversation picks its project and sends it with the first message", { timeout: TIMEOUT }, async () => {
    await withHub(async ({ page, sent }) => {
      await page.getByRole("button", { name: "+ Nova conversa" }).click();
      await picker(page).waitFor();
      assert.equal(await picker(page).isDisabled(), false, "a conversation that hasn't started can still choose");
      assert.deepEqual(await picker(page).locator("option").allInnerTexts(), ["Nenhum", "Declaração"]);

      await picker(page).selectOption("tax");
      await page.getByRole("textbox").last().fill("o que falta declarar?");
      await page.getByRole("button", { name: "Enviar" }).click();
      for (let waited = 0; waited < 5000 && chatFrames(sent).length === 0; waited += 100) await new Promise((r) => setTimeout(r, 100));
      const [frame] = chatFrames(sent);
      assert.equal(frame?.projectId, "tax", "the first message carries the project it starts in");
      assert.equal(frame?.message, "o que falta declarar?");
    });
  });

  test("the picker moves a conversation that exists, after asking, and only then", { timeout: TIMEOUT }, async () => {
    await withHub(async ({ page, sent }) => {
      const moves = () => sent.filter((s) => s.includes('"type":"moveConversation"')).map((s) => JSON.parse(s));
      const group = page.locator("section.conversation-group", { has: page.getByRole("heading", { name: "Declaração" }) });
      await group.waitFor();
      await page.getByRole("button", { name: /Conversa solta/ }).click();
      assert.equal(await picker(page).isDisabled(), false, "a conversation that exists can be moved");

      // Said no: nothing is sent and the picker goes back to what the conversation is.
      page.once("dialog", (dialog) => dialog.dismiss());
      await picker(page).selectOption("tax");
      assert.equal(moves().length, 0, "no move without a yes");
      assert.equal(await picker(page).inputValue(), "", "the picker still shows where the conversation is");
      assert.doesNotMatch(await group.innerText(), /Conversa solta/);

      // Said yes: the move is sent, and the list regroups.
      let warning = "";
      page.once("dialog", (dialog) => {
        warning = dialog.message();
        void dialog.accept();
      });
      await picker(page).selectOption("tax");
      await group.getByText("Conversa solta").waitFor();
      assert.match(warning, /Mover esta conversa para o projeto "Declaração"/, "it says what moving does");
      assert.deepEqual(moves().map((m) => [m.conversationId, m.projectId]), [["plain", "tax"]]);
      assert.equal(await picker(page).inputValue(), "tax");

      // And out again: no `projectId` on the frame, back with the others.
      page.once("dialog", (dialog) => {
        warning = dialog.message();
        void dialog.accept();
      });
      await picker(page).selectOption("");
      await page.locator("aside.conversations > ul.conversations-list").getByText("Conversa solta").waitFor();
      assert.match(warning, /Tirar esta conversa do projeto/);
      assert.equal(moves().at(-1).projectId, undefined, "out of any project");
      assert.doesNotMatch(await group.innerText(), /Conversa solta/);
    });
  });

  test("a conversation started outside a project sends none", { timeout: TIMEOUT }, async () => {
    await withHub(async ({ page, sent }) => {
      await page.getByRole("button", { name: "+ Nova conversa" }).click();
      await page.getByRole("textbox").last().fill("oi");
      await page.getByRole("button", { name: "Enviar" }).click();
      for (let waited = 0; waited < 5000 && chatFrames(sent).length === 0; waited += 100) await new Promise((r) => setTimeout(r, 100));
      const [frame] = chatFrames(sent);
      assert.equal(frame?.message, "oi", "the message was sent");
      assert.equal(frame.projectId, undefined, "with no project on it");
    });
  });
});

describe("the Projects tab", () => {
  test("makes a project, adds a note to it, and removing it keeps the files", { timeout: TIMEOUT }, async () => {
    await withHub(async ({ page, disk }) => {
      await page.getByRole("button", { name: "Projetos" }).click();
      await page.getByText("Declaração").first().waitFor();
      assert.match(await page.locator(".skills-item").first().innerText(), /O imposto deste ano/, "the seeded project is listed with its description");

      await page.getByRole("button", { name: "+ Novo" }).click();
      await page.getByLabel("Nome", { exact: true }).fill("Declaração 2026");
      assert.equal(await page.getByLabel("Nome da pasta").inputValue(), "declaracao-2026", "the folder name follows the name until it is typed");
      await page.getByLabel("Nome da pasta").fill("irpf");
      await page.getByLabel("Nome", { exact: true }).fill("Declaração 2026 completa");
      assert.equal(await page.getByLabel("Nome da pasta").inputValue(), "irpf", "and stops following once it was typed");
      await page.getByLabel("Instruções").fill("Seja breve.");
      await page.getByRole("button", { name: "Salvar", exact: true }).click();
      await page.getByText("Declaração 2026 completa").waitFor();
      const saved = fs.readFileSync(disk("vault/projects/irpf/PROJECT.md"), "utf8");
      assert.match(saved, /name: Declaração 2026 completa/);
      assert.match(saved, /Seja breve\./);

      // An id that is taken is refused, with the message on the form.
      await page.getByRole("button", { name: "+ Novo" }).click();
      await page.getByLabel("Nome", { exact: true }).fill("Outro");
      await page.getByLabel("Nome da pasta").fill("irpf");
      await page.getByRole("button", { name: "Salvar", exact: true }).click();
      await page.getByText(/already exists/).waitFor();
      await page.getByRole("button", { name: "Cancelar" }).click();

      // Edit it and add a note: it lands in the project's folder.
      await page.locator(".skills-item", { hasText: "irpf" }).getByRole("button", { name: "Editar" }).click();
      assert.equal(await page.getByLabel("Nome da pasta").isDisabled(), true, "the folder name is fixed after saving");
      await page.getByRole("button", { name: "+ Adicionar nota" }).click();
      await page.getByLabel("Nome do arquivo").fill("recibos.md");
      await page.getByLabel("Texto da nota").fill("recibo 1");
      await page.getByRole("button", { name: "Salvar nota" }).click();
      await page.locator(".project-file-list").getByText("recibos.md").waitFor();
      assert.equal(fs.readFileSync(disk("vault/projects/irpf/recibos.md"), "utf8"), "recibo 1");
      await page.getByRole("button", { name: "Cancelar" }).click();

      // Remove: only PROJECT.md goes.
      await page.locator(".skills-item", { hasText: "irpf" }).getByRole("button", { name: "Remover" }).click();
      await page.getByRole("button", { name: "Remover mesmo" }).click();
      await page.locator(".skills-item", { hasText: "irpf" }).waitFor({ state: "detached" });
      assert.equal(fs.existsSync(disk("vault/projects/irpf/PROJECT.md")), false, "the project is gone");
      assert.equal(fs.readFileSync(disk("vault/projects/irpf/recibos.md"), "utf8"), "recibo 1", "its files stay in the vault");
    });
  });

  test("the chat's picker follows a project made in the tab", { timeout: TIMEOUT }, async () => {
    await withHub(async ({ page }) => {
      await page.getByRole("button", { name: "Projetos" }).click();
      await page.getByRole("button", { name: "+ Novo" }).click();
      await page.getByLabel("Nome", { exact: true }).fill("Jardim");
      await page.getByRole("button", { name: "Salvar", exact: true }).click();
      await page.locator(".skills-item", { hasText: "Jardim" }).waitFor();
      await page.getByRole("button", { name: "Chat" }).click();
      await page.getByRole("button", { name: "+ Nova conversa" }).click();
      assert.deepEqual(await picker(page).locator("option").allInnerTexts(), ["Nenhum", "Declaração", "Jardim"]);
    });
  });
});
