// P121 end to end: the feed of activity, against a real hub whose files are seeded on disk (a log of delegated tasks, a note between two
// agents and a channel where an agent wrote first). What is proved is what the hub reads and sends back (`listActivity`), the order
// (newest first), the filter by agent, that an answer to the person is not counted, and where a click goes. Run with `npm run test:e2e`
// (see `settings.test.mjs` for what it needs).

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

/** FNV-1a, 64 bits, as the hub makes the ids of a channel (`message_agent.rs`); `parts` are joined by a zero byte, as `thread_id` does. */
function fnv(parts) {
  let hash = 0xcbf29ce484222325n;
  const bytes = Buffer.concat(parts.flatMap((part, i) => (i === 0 ? [Buffer.from(part)] : [Buffer.from([0]), Buffer.from(part)])));
  for (const byte of bytes) {
    hash ^= BigInt(byte);
    hash = (hash * 0x100000001b3n) & 0xffffffffffffffffn;
  }
  return hash.toString(16).padStart(16, "0");
}

const message = (id, role, content, createdAt) => ({ id, role, content, createdAt, attachments: [], generatedFiles: [] });

/** The hub's files for a small organization at work, `now` being the moment the test starts. */
function seeded(now) {
  const task = {
    id: "at-1",
    group: "turn-1",
    owner: "writer",
    assignee: "ops",
    parent_id: null,
    objective: "Revisar o texto",
    model: null,
    channel: "web",
    state: "pending",
    result: null,
    error: null,
    usage: null,
    created_at_ms: now - 60_000,
    started_at_ms: null,
    finished_at_ms: null,
  };
  const log = [
    { e: "task", task },
    { e: "running", id: "at-1", at: now - 50_000 },
    { e: "finished", id: "at-1", at: now - 40_000, state: "done", result: "Texto revisado", error: null, usage: null },
  ]
    .map((event) => JSON.stringify(event))
    .join("\n");
  const thread = { id: `agents-${fnv(["writer", "ops"])}`, title: "writer → ops", messages: [message("m1", "user", "Pode olhar o rascunho?", now - 30_000), message("m2", "assistant", "Já olhei", now - 20_000)], createdAt: now - 30_000, updatedAt: now - 20_000 };
  const channelName = "ops";
  const channel = {
    id: `channel-${fnv([channelName])}`,
    title: channelName,
    // The first message is the agent speaking first; the third answers a question of the person and is not an event.
    messages: [message("m1", "assistant", "O disco está cheio", now - 10_000), message("m2", "user", "Quanto?", now - 5_000), message("m3", "assistant", "98 por cento", now - 4_000)],
    createdAt: now - 10_000,
    updatedAt: now - 4_000,
    agentId: channelName,
  };
  return {
    "warden/agent_tasks.jsonl": log,
    [`warden/conversations-server/root/${thread.id}.json`]: JSON.stringify(thread),
    [`warden/conversations-server/root/${channel.id}.json`]: JSON.stringify(channel),
  };
}

describe("the feed of activity", () => {
  test("lists tasks, notes and messages the agent started, newest first, and filters by agent", { timeout: TIMEOUT }, async () => {
    const hub = await startHub({ files: seeded(Date.now()) });
    let context;
    try {
      const opened = await signIn(browser, hub);
      context = opened.context;
      const { page } = opened;
      await page.getByRole("button", { name: "Atividade", exact: true }).click();
      await page.getByText("ops escreveu para você").waitFor();

      const lines = await page.locator(".activity-item .activity-headline").allInnerTexts();
      assert.deepEqual(lines, [
        "ops escreveu para você",
        "ops respondeu a writer",
        "writer deixou um recado para ops",
        "ops concluiu a tarefa",
        "ops começou a tarefa",
        "writer delegou uma tarefa a ops",
      ]);
      assert.equal(await page.locator(".activity-day-title").first().innerText(), "Hoje");
      assert.equal(await page.getByText("98 por cento").count(), 0, "the answer to the person is not an event");
      assert.ok(await page.getByText("Texto revisado").count() > 0, "what the task produced is shown");

      // The filter lists the agents that appear and keeps the events an agent did or received.
      const options = await page.locator(".activity-filter option").allInnerTexts();
      assert.deepEqual(options, ["Todos", "ops", "writer"]);
      await page.locator(".activity-filter select").selectOption("writer");
      assert.equal(await page.locator(".activity-item").count(), 3, "writer delegated, left the note and was answered; the start and the end of the task are ops's");
      assert.equal(await page.getByText("ops escreveu para você").count(), 0);
    } finally {
      await context?.close();
      await hub.stop();
    }
  });

  test("a click goes to the tasks of the agent, to the conversation between two agents, or to the channel", { timeout: TIMEOUT }, async () => {
    const hub = await startHub({ files: seeded(Date.now()) });
    let context;
    try {
      const opened = await signIn(browser, hub);
      context = opened.context;
      const { page } = opened;
      await page.getByRole("button", { name: "Atividade", exact: true }).click();
      await page.getByRole("button", { name: "writer delegou uma tarefa a ops" }).click();
      await page.getByText("Só as tarefas de").waitFor();
      assert.match(await page.locator(".usage-view").innerText(), /ops/);

      await page.getByRole("button", { name: "Atividade", exact: true }).click();
      await page.getByRole("button", { name: "ops escreveu para você" }).click();
      await page.locator(".chat-title", { hasText: "ops" }).waitFor();
    } finally {
      await context?.close();
      await hub.stop();
    }
  });
});
