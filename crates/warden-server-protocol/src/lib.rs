//! The Fase 9.2 WebSocket + JSON wire protocol and its reusable client-side pieces — a crate of its
//! own so `warden-bootstrap` can use the wire DTOs (the web settings, P78) without a cyclic crate
//! dependency (`warden-server`'s hub already depends on `warden-bootstrap`). `warden-server`
//! re-exports the main pieces.
//!
//! `RemoteTool`/`RemoteToolChannel` (Fase 7.4's own-connection tool routing) stay in
//! `warden-server` — only the hub's `server.rs` uses them, `warden-bootstrap` never does.

pub mod client;
pub mod discovery;
pub mod protocol;
pub mod tls;

pub use client::{AuthRejected, ServerConnection};
pub use discovery::{discover_hubs, discover_hubs_on, DiscoveredHub};
pub use protocol::{ClientMessage, ServerMessage};
