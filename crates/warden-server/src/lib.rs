//! Fase 9.2 — the WebSocket + JSON protocol between a Warden "server" node and its clients
//! (mobile, desktop-as-client, browser extension). Scope is deliberately narrow: connection,
//! auth handshake, and heartbeat only. Routing tool execution to a specific connected client
//! (Fase 9.4/9.5) and client registration/pairing (9.3/9.6/9.7) build on top of this later.

pub mod client;
pub mod protocol;
pub mod server;

pub use client::ServerConnection;
pub use protocol::{ClientMessage, ServerMessage};
pub use server::Server;
