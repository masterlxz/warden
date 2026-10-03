// P119 end to end: the real settings page against real hubs, one with `--allow-machine-settings` and one without.
// Run with `npm run test:e2e` after `npm run build` and `cargo build -p warden-server --bin warden-server`.

import assert from "node:assert/strict";
import fs from "node:fs";
import { after, before, describe, test } from "node:test";
import { launchBrowser, lastSave, openSettings, saveWithKey, startHub } from "./harness.mjs";

const SECRET = "s3cret-mcp-value-xyz";
const TIMEOUT = 120_000;

/** A hub whose config already holds an MCP server with a secret env value, for what needs one to exist. */
const WITH_MCP = `
[[mcp_servers]]
name = "notes"
command = "/bin/true"
args = []

[mcp_servers.env]
TOKEN = "${SECRET}"
`;

let browser;
before(async () => {
  browser = await launchBrowser();
});
after(async () => {
  await browser?.close();
});

/** Runs `body` with a hub and a signed-in settings page, and always cleans both up. */
async function withSettings(options, body) {
  const hub = await startHub(options);
  let context;
  try {
    const opened = await openSettings(browser, hub);
    context = opened.context;
    await body({ hub, ...opened });
    assert.deepEqual(opened.errors, [], "the page raised no error");
  } finally {
    await context?.close();
    await hub.stop();
  }
}

/** Salvar → the machine confirmation (ticked) → the pairing key → saved. */
async function confirmMachineAndSave(page) {
  await page.getByRole("button", { name: "Salvar" }).click();
  const dialog = page.getByRole("alertdialog");
  await dialog.waitFor();
  await dialog.getByRole("checkbox").check();
  await dialog.getByRole("button", { name: "Continuar" }).click();
  await saveWithKey(page);
  await page.getByText(/✓ Salvo/).waitFor({ timeout: 60_000 });
}

describe("settings on a hub started with --allow-machine-settings", () => {
  test("a save that touches nothing of the machine sends no machine slice and asks no confirmation", { timeout: TIMEOUT }, async () => {
    await withSettings({ allowMachineSettings: true }, async ({ hub, page, sent }) => {
      assert.equal(await page.getByText("Somente leitura").count(), 0, "the machine section is editable");
      assert.ok(await page.getByRole("checkbox", { name: /Ligar o shell/ }).isEnabled());

      await page.getByLabel(/Profundidade da delegação/).fill("4");
      await page.getByRole("button", { name: "Salvar" }).click();
      assert.equal(await page.getByRole("alertdialog").count(), 0, "no machine confirmation for a delegation-only save");
      await saveWithKey(page);
      await page.getByText(/✓ Salvo/).waitFor();

      const { update } = lastSave(sent);
      assert.equal(update.machine, undefined, "no machine slice travelled");
      assert.equal(update.advanced.delegateMaxDepth, 4);
      assert.match(fs.readFileSync(hub.config, "utf8"), /delegate_max_depth = 4/);
    });
  });

  test("changing the shell, an SSH host and an MCP server asks first, and the secret never reaches the page or the log", { timeout: TIMEOUT }, async () => {
    await withSettings({ allowMachineSettings: true }, async ({ hub, page, sent }) => {
      await page.getByRole("checkbox", { name: /Ligar o shell/ }).check();

      await page.getByRole("button", { name: "+ Servidor SSH" }).click();
      const ssh = page.locator("li.settings-card", { hasText: "Remover servidor SSH" }).first();
      const sshInputs = ssh.locator("input:not([type=checkbox])");
      await sshInputs.nth(0).fill("box");
      await sshInputs.nth(1).fill("box.example.com");
      await sshInputs.nth(2).fill("deploy");

      await page.getByRole("button", { name: "+ Servidor MCP" }).click();
      const mcp = page.locator("li.settings-card", { hasText: "Remover servidor MCP" }).first();
      await mcp.locator("input").nth(0).fill("notes");
      await mcp.locator("input").nth(1).fill("/bin/true");
      await mcp.getByRole("button", { name: "+ Adicionar" }).click();
      await mcp.locator("input").nth(2).fill("TOKEN");
      await mcp.locator("input[type=password]").fill(SECRET);

      await page.getByRole("button", { name: "Salvar" }).click();
      const dialog = page.getByRole("alertdialog");
      await dialog.waitFor();
      const said = await dialog.innerText();
      assert.match(said, /Ligar o shell/);
      assert.match(said, /“box”/);
      assert.match(said, /“notes”/);
      assert.ok(!said.includes(SECRET), "the confirmation never shows the secret");
      assert.ok(await dialog.getByRole("button", { name: "Continuar" }).isDisabled(), "Continuar waits for the acknowledgement");
      await dialog.getByRole("checkbox").check();
      await dialog.getByRole("button", { name: "Continuar" }).click();
      await saveWithKey(page);
      await page.getByText(/✓ Salvo/).waitFor({ timeout: 60_000 });

      const { update } = lastSave(sent);
      assert.equal(update.machine.enableShell, true, "the machine slice travelled");
      const file = fs.readFileSync(hub.config, "utf8");
      assert.match(file, /enable_shell = true/);
      assert.ok(file.includes('id = "box"') && file.includes("box.example.com"), "the SSH host is in the file");
      assert.ok(file.includes(SECRET), "the hub keeps the MCP secret");
      assert.ok(!(await page.content()).includes(SECRET), "the page never has it after saving");
      assert.match(hub.log(), /machine settings changed from 127\.0\.0\.1: shell on, MCP servers \(1 now\), SSH hosts \(1 now\)/);
      assert.ok(!hub.log().includes(SECRET), "the hub's log has no secret");

      await page.reload();
      await page.getByRole("button", { name: "Configurações" }).click();
      await page.getByText("Máquina do hub").first().waitFor();
      const again = page.locator("li.settings-card", { hasText: "Remover servidor MCP" }).first();
      assert.match(await again.innerText(), /Salvo/, "after a reload the value shows as saved");
      assert.ok(!(await page.content()).includes(SECRET), "and the page still never has it");
      assert.ok(await page.getByRole("checkbox", { name: /Ligar o shell/ }).isChecked(), "the shell is on after a reload");
    });
  });

  test("renaming an MCP server keeps its secret, and a relative folder is refused on the page", { timeout: TIMEOUT }, async () => {
    await withSettings({ allowMachineSettings: true, extraConfig: WITH_MCP }, async ({ hub, page }) => {
      const mcp = page.locator("li.settings-card", { hasText: "Remover servidor MCP" }).first();
      await mcp.locator("input").nth(0).fill("notes-renamed");
      await confirmMachineAndSave(page);
      const file = fs.readFileSync(hub.config, "utf8");
      assert.ok(file.includes("notes-renamed"), "the rename was saved");
      assert.ok(file.includes(SECRET), "the renamed server kept its secret value");

      await page.getByLabel(/Pasta do cofre/).fill("relativa/pasta");
      assert.match(await page.locator(".error-banner").first().innerText(), /caminho absoluto/);
      assert.ok(await page.getByRole("button", { name: "Salvar" }).isDisabled(), "Salvar is disabled meanwhile");
    });
  });
});

describe("settings on a hub started without the flag", () => {
  test("the machine section is read-only and says how to turn it on, while the rest still saves", { timeout: TIMEOUT }, async () => {
    await withSettings({ allowMachineSettings: false }, async ({ hub, page, sent }) => {
      const note = page.getByText(/Somente leitura/);
      assert.equal(await note.count(), 1);
      assert.match(await note.innerText(), /--allow-machine-settings/);
      assert.ok(await page.getByRole("checkbox", { name: /Ligar o shell/ }).isDisabled(), "the shell checkbox is disabled");
      assert.ok(await page.getByRole("button", { name: "+ Servidor SSH" }).isDisabled(), "adding an SSH host is disabled");

      // What isn't the machine still works: the Telegram token and the delegation ceilings.
      await page.locator(".settings-field", { hasText: "Token do bot do Telegram" }).getByRole("button", { name: "Adicionar" }).click();
      await page.getByPlaceholder("Cole o token").fill("123456:E2E-telegram-token-abcdefghij");
      await page.getByLabel(/Jobs em paralelo/).fill("2");
      await page.getByRole("button", { name: "Salvar" }).click();
      await saveWithKey(page);
      await page.getByText(/✓ Salvo/).waitFor({ timeout: 60_000 });

      assert.equal(lastSave(sent).update.machine, undefined, "no machine slice travelled");
      const file = fs.readFileSync(hub.config, "utf8");
      assert.ok(file.includes("123456:E2E-telegram-token-abcdefghij"), "the Telegram token reached the file");
      assert.match(file, /max_parallel_jobs = 2/);
      assert.ok(!(await page.content()).includes("E2E-telegram-token"), "the token is not on the page afterwards");
      assert.ok(!/enable_shell = true/.test(file), "the shell stayed off");
    });
  });
});
