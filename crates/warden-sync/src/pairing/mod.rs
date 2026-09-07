//! Warden-device-to-Warden-device pairing (code + LAN, no camera) — how the vault-encryption key
//! (`crate::manifest::SyncSecrets::vault_key`) spreads from the first device that generated it to
//! every other install of Warden the user owns. Distinct from `warden_truthid`'s QR pairing with
//! the TruthID phone app, which pays for and publishes to Arweave but never sees this key.

pub mod host;
pub mod join;
pub mod protocol;

pub use host::PairingHost;
pub use join::{join, join_with_hosts, JoinedKeyMaterial};
