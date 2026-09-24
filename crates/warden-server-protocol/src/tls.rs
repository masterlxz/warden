//! Client-side TLS for `wss://` hubs (P36 fatia 2).
//!
//! The certificate a hub serves is expected to be publicly trusted — the intended source is
//! `tailscale cert` (Let's Encrypt, for the machine's MagicDNS name) — so the default config
//! trusts the Mozilla root set (`webpki-roots`) and nothing else: no pinning, no per-hub trust
//! state to keep in sync across devices.
//!
//! The crypto provider is picked explicitly (`ring`) instead of relying on rustls's process-wide
//! default: this workspace compiles both `ring` and `aws-lc-rs` into rustls (via `reqwest`/`rmcp`),
//! and with both present rustls refuses to guess — `ClientConfig::builder()` panics.

use std::sync::{Arc, OnceLock};

use rustls::{ClientConfig, RootCertStore};

/// Path a plain `ws://` connection must upgrade on to be answered by a hub that requires TLS —
/// the only thing such a hub serves in the clear is `Discover`. Hubs without TLS answer
/// `Discover` on any path, so probes always use this one.
pub const DISCOVER_PATH: &str = "/discover";

/// The config every `wss://` connection uses unless a caller passes its own (tests, with a
/// throwaway CA). Built once per process.
pub fn default_client_config() -> Arc<ClientConfig> {
    static CONFIG: OnceLock<Arc<ClientConfig>> = OnceLock::new();
    CONFIG
        .get_or_init(|| {
            let roots = RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
            client_config_with_roots(roots)
        })
        .clone()
}

/// A client config trusting exactly `roots`.
pub fn client_config_with_roots(roots: RootCertStore) -> Arc<ClientConfig> {
    let config = ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .expect("ring supports the default TLS versions")
        .with_root_certificates(roots)
        .with_no_client_auth();
    Arc::new(config)
}
