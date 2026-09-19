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
- **Sub-agents, named personas, voice, generated documents** — delegate sub-tasks to scoped sub-agents,
  configure named agents with their own personality, talk by voice, and have the model produce real
  PDF/CSV/XLSX/Markdown files as deliverables.

## Project layout

```
crates/                 Shared Rust: the agent's core logic and every non-UI backend
  warden-core             Orchestrator, memory (Vault), model/tool abstractions
  warden-bootstrap         Reads config.toml, wires a ready-to-use agent for any channel
  warden-server-protocol   Wire protocol + LAN discovery between hub and clients
  warden-server            The hub (binaries: warden-server, warden-node)
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
