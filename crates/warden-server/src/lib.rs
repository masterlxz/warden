//! Fase 9.2 — the WebSocket + JSON protocol between a Warden "server" node and its clients
//! (mobile, desktop-as-client, browser extension): connection, auth handshake, and heartbeat.
//! As of Fase 7.3, `Server` also hosts a real `Orchestrator` and answers `Chat` messages with
//! it (one conversation per `device_id`, same pattern as the Telegram/WhatsApp channels).
//! Routing tool execution to a *specific* connected client (Fase 9.4/9.5) and client
//! registration/pairing (9.3/9.6/9.7) still build on top of this later.

pub mod client;
pub mod protocol;
pub mod server;

pub use client::ServerConnection;
pub use protocol::{ClientMessage, ServerMessage};
pub use server::Server;
