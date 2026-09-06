//! Rust "requester" client for TruthID's cross-device `pin()` protocol
//! (`sdk/dart/lib/src/requester.dart` in the TruthID repo): show a QR, a TruthID mobile app
//! scans/approves/pays/publishes to Arweave with its own wallet, and the `ar://<tx_id>` comes
//! back over the LAN.
//!
//! Scope of this crate: the protocol client only (QR payload, LAN transport, the two crypto
//! layers). It does not decide what content to pin, does not render the QR, and does not
//! implement the dead-drop (IPFS/IPNS) fallback — see `project/PENDING.md`/`ARCHITECTURE.md` for
//! what's deliberately deferred.

pub mod crypto;
pub mod lan;
pub mod protocol;
pub mod requester;

pub use protocol::{PinResult, QrPayload};
pub use requester::PendingPin;
