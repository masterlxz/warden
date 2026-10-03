// What every end-to-end test of the web page needs: a real hub that touches nothing of the user's, and a
// headless browser to drive the real page against it. See `settings.test.mjs` for a use.
//
// Needs, once: `npm run build` (the hub embeds `web/dist`) and `cargo build -p warden-server --bin warden-server`.

import { spawn } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright-core";

export const PAIRING_KEY = "e2e-test-pairing-key-0123456789-abcdef";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "..");

/** The hub binary: `WARDEN_SERVER_BIN`, else the debug build. A missing one says how to build it. */
export function serverBinary() {
  const bin = process.env.WARDEN_SERVER_BIN ?? path.join(repoRoot, "target", "debug", "warden-server");
  if (!fs.existsSync(bin)) {
    throw new Error(`There is no hub binary at ${bin}. Build the page and the hub first:\n  npm run build\n  cargo build -p warden-server --bin warden-server\n(or set WARDEN_SERVER_BIN).`);
  }
  return bin;
}

const BASE_CONFIG = `active_provider = "main"

[[providers]]
id = "main"
kind = "gemini"
api_key = "fake-key-for-test-only-0123456789"

[[agents]]
id = "writer"
persona = "You write."

[[agents]]
id = "ops"
persona = "You operate."
`;

/**
 * A real `warden-server serve` on a free loopback port, in a folder of its own: HOME and the XDG folders
 * point there, so the hub's devices, conversations and vault never touch the user's. `extraConfig` is TOML
 * appended to the minimal config (a provider with a fake key and two agents). `files` maps a file name to its
 * text, written beside `config.toml` (where the hub keeps `bot_pairing.json` and `bot_hub.json`).
 * Returns `{ url, config, log(), stop() }`; `stop()` ends the process and deletes the folder.
 */
export async function startHub({ allowMachineSettings = false, extraConfig = "", files = {} } = {}) {
  const bin = serverBinary();
  const home = fs.mkdtempSync(path.join(os.tmpdir(), "warden-e2e-"));
  const config = path.join(home, "config.toml");
  fs.writeFileSync(config, BASE_CONFIG + extraConfig);
  for (const [name, text] of Object.entries(files)) fs.writeFileSync(path.join(home, name), text);
  const args = ["serve", "--listen", "127.0.0.1:0", "--auth-key", PAIRING_KEY, "--config", config, "--vault-path", path.join(home, "vault")];
  if (allowMachineSettings) args.push("--allow-machine-settings");
  const child = spawn(bin, args, {
    env: { PATH: process.env.PATH, HOME: home, XDG_CONFIG_HOME: home, XDG_DATA_HOME: home, GEMINI_API_KEY: "fake" },
    stdio: ["ignore", "pipe", "pipe"],
  });
  let log = "";
  child.stdout.on("data", (d) => (log += d));
  child.stderr.on("data", (d) => (log += d));
  const exited = new Promise((resolve) => child.once("exit", resolve));

  const stop = async () => {
    child.kill();
    await Promise.race([exited, new Promise((r) => setTimeout(r, 5000))]);
    fs.rmSync(home, { recursive: true, force: true });
  };

  let address = null;
  for (let waited = 0; waited < 15000 && !address; waited += 100) {
    address = /listening on (127\.0\.0\.1:\d+)/.exec(log)?.[1] ?? null;
    if (!address) {
      if (child.exitCode !== null) break;
      await new Promise((r) => setTimeout(r, 100));
    }
  }
  if (!address) {
    await stop();
    throw new Error(`The hub did not start:\n${log}`);
  }
  return { url: `http://${address}/`, config, log: () => log, stop };
}

/** Headless Chromium: `PLAYWRIGHT_CHROMIUM_EXECUTABLE`, else Playwright's own, else the system's Chrome. */
export async function launchBrowser() {
  const executablePath = process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE;
  const attempts = executablePath ? [{ executablePath }] : [{}, { channel: "chrome" }];
  let last;
  for (const options of attempts) {
    try {
      return await chromium.launch(options);
    } catch (err) {
      last = err;
    }
  }
  throw new Error(`There is no browser to drive. Install one with \`npx playwright-core install chromium\`, or point PLAYWRIGHT_CHROMIUM_EXECUTABLE at Chrome or Chromium.\n${last?.message ?? ""}`);
}

/**
 * Signs in with the pairing key and opens Configurações. `sent` collects every WebSocket frame the page
 * sends, which is how a test proves what travelled to the hub rather than only what the screen showed.
 */
export async function openSettings(browser, hub) {
  const context = await browser.newContext({ viewport: { width: 1100, height: 1400 } });
  const page = await context.newPage();
  const sent = [];
  page.on("websocket", (ws) => ws.on("framesent", (f) => typeof f.payload === "string" && sent.push(f.payload)));
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await page.goto(hub.url);
  await page.getByRole("tab", { name: "Chave de pareamento" }).click();
  await page.getByLabel("Chave de pareamento").first().fill(PAIRING_KEY);
  await page.getByRole("button", { name: "Entrar" }).click();
  await page.getByRole("button", { name: "Configurações" }).click();
  await page.getByText("Máquina do hub").first().waitFor();
  return { page, sent, context, errors };
}

/** Types the pairing key into the save prompt and confirms. */
export async function saveWithKey(page) {
  await page.getByLabel("Chave de pareamento do hub").fill(PAIRING_KEY);
  await page.getByRole("button", { name: "Confirmar" }).click();
}

/** The last `saveSettings` message the page sent, parsed; `undefined` before any. */
export function lastSave(sent) {
  const frame = sent.filter((s) => s.includes('"saveSettings"')).at(-1);
  return frame === undefined ? undefined : JSON.parse(frame);
}
