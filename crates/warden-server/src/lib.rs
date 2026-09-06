//! Fase 9.2 — the WebSocket + JSON protocol between a Warden "server" node and its clients
//! (mobile, desktop-as-client, browser extension): connection, auth handshake, and heartbeat.
//! As of Fase 7.3, `Server` also hosts a real `Orchestrator` and answers `Chat` messages with
//! it (one conversation per `device_id`, same pattern as the Telegram/WhatsApp channels). As of
//! Fase 7.4, a client can advertise tools in `Hello` that the server registers as `RemoteTool`s
//! on that connection's `Orchestrator` — the first concrete instance of routing a tool call to a
//! *specific* connected client, which Fase 9.4/9.5 will generalize further. Client
//! registration/pairing (9.3/9.6/9.7) still builds on top of this later.

pub mod client;
pub mod protocol;
pub mod remote_tool;
pub mod server;

pub use client::ServerConnection;
pub use protocol::{ClientMessage, ServerMessage};
pub use remote_tool::{RemoteTool, RemoteToolChannel};
pub use server::Server;
