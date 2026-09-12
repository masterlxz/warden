//! Fase 9.2 — the WebSocket + JSON protocol between a Warden "server" node and its clients
//! (mobile, desktop-as-client, browser extension): connection, auth handshake, and heartbeat.
//! As of Fase 7.3, `Server` also hosts a real `Orchestrator` and answers `Chat` messages with
//! it (one conversation per `device_id`, same pattern as the Telegram/WhatsApp channels). As of
//! Fase 7.4, a client can advertise tools in `Hello` that the server registers as `RemoteTool`s
//! on that connection's `Orchestrator` — always a round-trip back to the *same* device, not
//! cross-device routing yet. As of Fase 9.3/9.4, `Server` keeps a registry of every connected
//! device and `ClientMessage::CallDeviceTool` lets one connection route a tool call to a
//! *specific different* one, answered by `ServerMessage::DeviceToolResult`/`DeviceToolError` —
//! the server-side foundation P61's `RemoteNodeProvider` (a `StorageProvider` backed by another
//! machine's vault) sits on top of. Workspace management (9.6) and QR-mediated pairing (9.7)
//! still build on top of this later.
//!
//! The wire protocol and its reusable client-side pieces (`ClientMessage`/`ServerMessage`,
//! `ServerConnection`, `RemoteNodeProvider`) moved to `warden-server-protocol` (this session) so
//! `warden-bootstrap` could depend on `RemoteNodeProvider` without a cyclic crate dependency —
//! `warden-server` (this crate, the hub) already depends on `warden-bootstrap`. Re-exported below
//! for backward compatibility; only `RemoteTool`/`RemoteToolChannel` (Fase 7.4's own-connection
//! routing, used solely by `server.rs`) still live here.
//!
//! `vault_node` (this session) is the target side `RemoteNodeProvider` was missing — the
//! `warden-node` binary (`src/bin/warden-node.rs`) connects as a client and serves
//! `vault_read`/`vault_write`/`vault_list`/`vault_delete` against its own local `Vault`, so
//! `[remote_node]` in `config.toml` finally has something real to point at.
//!
//! `device_registry` (Fase 9.3) closes the gap that left: knowing the shared `auth_key` used to
//! be enough to route a `CallDeviceTool` to (or from) any connected device. Now a device must
//! also be explicitly `approve`d by the operator (`warden-server devices approve <id>`, `main.rs`)
//! before it can take part in routing — `Hello`/`Chat`/`Ping` are unaffected, only
//! `CallDeviceTool` checks pairing status. See `device_registry.rs`'s module docs for why the
//! store is a thin, stateless-between-calls wrapper over a JSON file instead of anything cached.

pub mod device_registry;
pub mod remote_tool;
pub mod server;
pub mod vault_node;

pub use device_registry::{PairedDevice, PairingStatus, PairingStore};
pub use remote_tool::{RemoteTool, RemoteToolChannel};
pub use server::Server;
pub use warden_server_protocol::{ClientMessage, RemoteNodeProvider, ServerConnection, ServerMessage};
