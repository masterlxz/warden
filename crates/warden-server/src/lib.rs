//! Fase 9.2 — the WebSocket + JSON protocol between a Warden "server" node and its clients
//! (mobile, desktop-as-client, browser extension, the web UI): connection, auth handshake, and
//! heartbeat. As of Fase 7.3, `Server` also hosts a real `Orchestrator` and answers `Chat` messages
//! with it. As of Fase 7.4, a client can advertise tools in `Hello` that the server registers as
//! `RemoteTool`s on that connection's `Orchestrator` — a round-trip back to the *same* device. As
//! of Fase 9.3/9.4, `Server` keeps a registry of every connected device and
//! `ClientMessage::CallDeviceTool` lets one connection route a tool call to a *specific different*
//! one, answered by `ServerMessage::DeviceToolResult`/`DeviceToolError`. No client uses that
//! routing today (its only one, P61's `RemoteNodeProvider`, was removed in Sessão 105); it stays
//! as the generic piece a cross-device tool would build on.
//!
//! The wire protocol and its reusable client-side pieces (`ClientMessage`/`ServerMessage`,
//! `ServerConnection`) live in `warden-server-protocol`, re-exported below; only
//! `RemoteTool`/`RemoteToolChannel` (Fase 7.4's own-connection routing, used solely by
//! `server.rs`) live here.
//!
//! `device_registry` (Fase 9.3): knowing the shared `auth_key` used to be enough to route a
//! `CallDeviceTool` to (or from) any connected device. Now a device must also be explicitly
//! `approve`d by the operator (`warden-server devices approve <id>`, `main.rs`, or the web's
//! Aparelhos tab) before it can take part in routing — `Hello`/`Chat`/`Ping` are unaffected, only
//! `CallDeviceTool` checks pairing status. See `device_registry.rs`'s module docs for why the
//! store is a thin, stateless-between-calls wrapper over a JSON file instead of anything cached.

pub mod api_key_admin;
pub mod api_keys;
pub mod approval;
pub mod bot_admin;
pub mod chat_input;
pub mod conversations;
pub mod device_registry;
pub mod openai_api;
pub mod people;
pub mod devices;
pub mod member_backup;
pub mod node_client;
pub mod node_tools;
pub mod nodes;
pub mod remote_tool;
pub mod scheduler;
pub mod server;
pub mod settings;
pub mod skills;
pub mod sync;
pub mod task_admin;
pub mod tls;
pub mod truthid_login;
pub mod usage;
pub mod user_admin;
pub mod vault;
pub mod web_ui;

pub use device_registry::{AuthRejection, HelloOutcome, PairedDevice, PairingStatus, PairingStore};
pub use remote_tool::{RemoteTool, RemoteToolChannel};
pub use server::{resolve_server_name, Server};
pub use settings::{SettingsHost, SharedOrchestrator};
pub use tls::{HubTls, TailscaleCert};
pub use web_ui::{EmbeddedWebUi, StaticWebUi, WebAssets};
pub use warden_server_protocol::{discover_hubs, discover_hubs_on, ClientMessage, DiscoveredHub, ServerConnection, ServerMessage};
