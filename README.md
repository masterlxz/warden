# Warden

Warden is a personal AI agent that lives on your own devices, remembers things in a plain markdown
vault you fully own, and reaches you wherever you already are — desktop, phone, terminal, browser,
Telegram, or WhatsApp.

It's model-agnostic (OpenAI, Anthropic, Gemini, Ollama, or any OpenAI-compatible endpoint), local-first
by default, and every piece of it is built to work without depending on a company staying online:
memory is just files, sync is optional and decentralized, and any device you own can act as the hub
for the others.

## Highlights

- **Memory you can read** — the agent's long-term memory is a folder of markdown files (Obsidian-
  compatible), with grep and semantic search. Nothing proprietary, nothing locked away.
- **Model-agnostic** — swap between OpenAI, Anthropic, Gemini, or a local Ollama model per agent, per
  conversation, without losing memory or history.
- **Multi-channel** — the same agent, the same memory, reachable from a native desktop app (Tauri), a
  mobile app (Flutter), a terminal (`warden-cli`), a Chrome extension, Telegram, and WhatsApp.
- **Tools & MCP** — shell access, file I/O, web search, document generation, and a full [MCP](https://modelcontextprotocol.io)
  client/server, so Warden can use (or be used by) any MCP-compatible tool.
- **Self-hosted networking, no manual setup** — any device running the desktop app can flip into being
  the hub for the others. Other devices find it automatically on the local network — no typing IP
  addresses, no VPN required (though nothing stops you from putting it behind Tailscale or exposing a
  port to the internet yourself, same as any self-hosted app).
- **Decentralized, optional sync** — the vault can sync between your own devices two ways: through
  [Arweave](https://arweave.org) (paid for via the TruthID app, no central Warden server required at
  all) or through a plain git remote you already control.
- **Skills** — reusable instructions the model loads on demand, stored as plain markdown in your vault;
  create them by hand, by describing them, or just by asking for one in a chat.
- **Warden API** — the hub also speaks the OpenAI chat-completions format: point any OpenAI client
  (a script, n8n, a chat app) at `http(s)://<hub>/v1` with a key created in the app, and it talks to
  your agent, vault and tools included.
- **Scheduled tasks** — "every weekday at 8, summarize my e-mails": an agent runs a prompt on its own,
  on the always-on hub, and each run lands in the task's conversation on every device.
- **Sub-agents, named personas, voice, generated documents** — delegate sub-tasks to scoped sub-agents,
  configure named agents with their own personality, talk by voice, and have the model produce real
  PDF/CSV/XLSX/Markdown files as deliverables.

## Project layout

```
crates/                 Shared Rust: the agent's core logic and every non-UI backend
  warden-core             Orchestrator, memory (Vault), model/tool abstractions
  warden-bootstrap         Reads config.toml, wires a ready-to-use agent for any channel
  warden-server-protocol   Wire protocol + LAN discovery between hub and clients
  warden-server            The hub (the warden-server binary)
  warden-sync              Vault sync — Arweave/TruthID or a self-hosted git remote
  warden-truthid           Client for the TruthID app's pairing/payment protocol
  warden-cli               The `warden` terminal command
  warden-telegram          Telegram bot channel
  warden-whatsapp          WhatsApp channel (via a Baileys sidecar, see sidecar/)
  warden-mcp-server        Exposes Warden's own tools as an MCP server
  warden-mobile-bridge     Rust↔Flutter FFI bridge for the mobile app

desktop/                Native desktop app — Tauri (Rust) + React/TypeScript
mobile/                 Mobile app — Flutter/Dart
extension/              Chrome extension — TypeScript (no Rust dependency)
sidecar/                Node.js sidecar process for the WhatsApp channel (Baileys)
project/                Living project docs: architecture decisions, phase plan, roadmap
```

`crates/warden-core` sits at the bottom of the dependency graph and knows nothing about any specific
channel. Every "face" of Warden (CLI, desktop, mobile, extension, Telegram, WhatsApp) is built on top of
it through `warden-bootstrap`, so they all share one agent and one memory model.

## Getting started

**Prerequisites**: a recent [Rust toolchain](https://rustup.rs) for everything under `crates/` and
`desktop/src-tauri/`; [Node.js](https://nodejs.org) + npm for `desktop/` (frontend) and `extension/`;
[Flutter](https://flutter.dev) for `mobile/`; a model provider API key (or a local
[Ollama](https://ollama.com) install).

```bash
# Build and test the whole Rust workspace
cargo build --workspace
cargo test --workspace

# Run the terminal client
cargo run -p warden-cli

# Run the desktop app in dev mode
cd desktop && npm install && npm run tauri dev

# Build the browser extension
cd extension && npm install && npm run build   # load dist/ as an unpacked extension

# Run the mobile app
cd mobile && flutter pub get && flutter run
```

### Warden API (OpenAI-compatible)

With a hub running (`warden-server serve`, or the desktop's embedded hub), create a key and use it as
any OpenAI client's API key:

```bash
warden-server api-keys create n8n        # prints the key once; also in Settings (desktop and web)
curl http://<hub>:7420/v1/chat/completions \
  -H "Authorization: Bearer wdn_..." -H "Content-Type: application/json" \
  -d '{"model": "warden", "messages": [{"role": "user", "content": "What did I write about the trip?"}]}'
```

`GET /v1/models` lists `warden` and one `warden/<agent>` per configured agent. A key can also be bound
to one agent (`api-keys create bot --agent <id>`, or the selector in Settings): it then only speaks as
that agent, whatever `model` says. Streaming (`"stream": true`) works, nothing is saved as a
conversation, and the spend counts toward the `api` channel's limits.

Function calling works too, so coding harnesses (opencode and the like) can bring their own tools. The
model sees the client's `tools` next to the agent's own (the client's wins on a name clash). A call to
one of the client's tools comes back as `tool_calls` with `finish_reason: "tool_calls"`, and the turn
continues when the client sends the `tool` results back. To narrow which of the agent's own tools
(shell, files...) such a client can reach, bind its key to an agent with `allowed_tools`.
`tool_choice: "none"` leaves the client's tools out, and any other choice acts as `auto`.

### Scheduled tasks

A task is a prompt an agent runs on a schedule, kept as `[[tasks]]` in `config.toml`. Only a hub started
with `--run-tasks` runs them (the config syncs, so switch it on in the one hub that's always up):

```bash
warden-server tasks add news --agent reader --cron "0 8 * * 1-5" --timezone America/Sao_Paulo \
  --prompt "Summarize today's headlines"
warden-server tasks add check --every 2h --prompt "Is the site up?"   # or --once 2026-10-01T09:00
warden-server tasks list          # schedule, last run and next run
warden-server tasks pause news    # resume / remove / run <id> (run it now, here)
warden-server serve --run-tasks
```

Each run adds the prompt and the answer to the task's conversation (`task-<id>`), which every device
lists and can keep talking in. A run missed while the hub was down happens once when it's back. Nobody
is watching a run, so a tool that needs approval refuses, and the spending limits count it under the
`tasks` channel.

The same is on screen: the web's **Tarefas** tab (every change asks for the pairing key, like the API
keys) and the desktop's **Tasks** screen, which also has this computer's own switch to run them on
the embedded hub — kept in `hub-local.json`, outside the synced config. And in a chat: an agent with "can create and edit scheduled tasks"
switched on (Settings or `/agents`) turns "every weekday at 8, summarize the news" into a task, after
you approve the card it shows.

### Nodes: lend another machine to your agents

Any other machine (a home PC, a second VPS) can lend its shell and/or a folder to the hub's agents,
while everything else stays on the hub:

```bash
# on that machine — the pairing key is only needed the first time
warden-server node --hub wss://<hub>:7420 --auth-key <key> --description "home PC" --tag home --shell --files ~/shared
# on the hub (or the web's Devices tab / the desktop's Workspace)
warden-server devices approve <node id>
warden-server nodes allow <node id> --agent ops --approval
```

Agents then get `list_nodes`, `node_shell`, `node_read_file`, `node_write_file` and `node_list_files`.
A node can also lend MCP servers from its own `config.toml`, by name (`--mcp github --mcp postgres`):
each of their tools shows up on the hub as `<node>__<tool>`, with its own schema, while the node is online.
And it can lend a model (`--model ollama`, a provider in its own config): on the hub, a provider with
`kind = "node"`, the node's id and `model = "ollama"` streams from it. Put it in a combo with a cloud model
and the cloud answers whenever the home machine is off.
Two locks: the node chooses what it lends, the hub chooses which agents may use it and whether each call
asks you first. Every call is logged to `node_audit.jsonl`; a call cut short by the node dropping fails
right away and is never retried.

Configuration lives in `config.toml` (path resolved per-OS via `dirs::config_dir()`, e.g.
`~/.config/warden/config.toml` on Linux) — see `project/ARCHITECTURE.md` for the full schema and the
reasoning behind it.

## Project status & docs

Warden is under active, fast-moving development. `project/` holds the living documentation the project
is actually run from (mostly in Portuguese, the maintainer's working language):

- `PHASE.md` — the phased build plan and what's done in each phase
- `PENDING.md` — every open decision and how it evolved, each with its own id
- `ARCHITECTURE.md` — technical decisions and the reasoning behind them
- `ROADMAP.md` — priority order and ideas that haven't become a formal plan yet

## License

MIT — see [LICENSE](LICENSE).
