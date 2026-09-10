//! The Fase 9.2 WebSocket + JSON wire protocol and its reusable client-side pieces — split out of
//! `warden-server` (this session) so `warden-bootstrap` could depend on `RemoteNodeProvider`
//! without a cyclic crate dependency (`warden-server`'s hub already depends on
//! `warden-bootstrap`). `warden-server` re-exports everything here for backward compatibility —
//! nothing outside these two crates should need to depend on this one directly yet.
//!
//! `RemoteTool`/`RemoteToolChannel` (Fase 7.4's own-connection tool routing) stay in
//! `warden-server` — only the hub's `server.rs` uses them, `warden-bootstrap` never does.

pub mod client;
pub mod protocol;
pub mod remote_node;

pub use client::ServerConnection;
pub use protocol::{ClientMessage, ServerMessage};
pub use remote_node::RemoteNodeProvider;
