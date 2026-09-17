//! Fase 9.1 (redefined) — finding a `warden-server` hub on the local network without already
//! knowing its address, instead of the Tailscale-integration reading of "9.1" the project
//! originally scoped. Tailscale (or any other network extension) stays entirely the operator's
//! own infra choice, outside the Warden codebase — this sweep just needs *an* IPv4 subnet to
//! probe, whichever interfaces the OS reports.
//!
//! Modeled directly on `warden-sync`'s vault-key LAN pairing (`pairing/join.rs`): reuse
//! `warden_truthid::lan::candidate_hosts()` for the /24-per-interface expansion, then probe every
//! `host:port` concurrently with a short per-host timeout. Two deliberate differences from that
//! precedent: (1) no secret code is exchanged — `Discover` carries no credential, so any host can
//! answer, and the reply only ever reveals a display name; (2) this collects *every* hub that
//! answers within one pass instead of stopping at the first — there can legitimately be more than
//! one `warden-server` on the same LAN (home hub + a friend's, say), and the operator picks.

use std::net::Ipv4Addr;
use std::time::Duration;

use futures_util::stream::{self, StreamExt};
use futures_util::SinkExt;
use tokio_tungstenite::tungstenite::Message;
use warden_truthid::lan::candidate_hosts;

use crate::protocol::{ClientMessage, ServerMessage};

const PROBE_TIMEOUT: Duration = Duration::from_millis(800);
const CONCURRENCY: usize = 50;

#[derive(Debug, Clone, PartialEq)]
pub struct DiscoveredHub {
    pub host: Ipv4Addr,
    pub port: u16,
    pub server_name: String,
}

/// Sweeps every local network for a hub listening on `port`. A single pass (~1-2s) — no
/// retry-until-timeout loop like `pairing::join`'s, since there's no code the hub might not have
/// shown yet; a caller wanting a fresher list just calls this again (e.g. a "Refresh" button).
pub async fn discover_hubs(port: u16) -> anyhow::Result<Vec<DiscoveredHub>> {
    discover_hubs_on(candidate_hosts()?, port).await
}

/// Same as `discover_hubs`, but sweeps only `hosts` — used by tests to avoid sweeping the real
/// LAN, same reasoning as `pairing::join_with_hosts`.
pub async fn discover_hubs_on(hosts: Vec<Ipv4Addr>, port: u16) -> anyhow::Result<Vec<DiscoveredHub>> {
    let mut hubs: Vec<DiscoveredHub> = stream::iter(hosts)
        .map(|host| probe_one(host, port))
        .buffer_unordered(CONCURRENCY)
        .filter_map(|hub| async { hub })
        .collect()
        .await;
    hubs.sort_by_key(|hub| (hub.host, hub.port));
    Ok(hubs)
}

/// One connect+Discover+DiscoverAck attempt against a single `host:port`. `None` covers every
/// "this wasn't a hub" outcome (nothing listening, timed out, malformed/unexpected reply) — none
/// of those should abort the sweep, since a different host on the LAN might still answer.
async fn probe_one(host: Ipv4Addr, port: u16) -> Option<DiscoveredHub> {
    let url = format!("ws://{host}:{port}");
    let (mut ws, _) = tokio::time::timeout(PROBE_TIMEOUT, tokio_tungstenite::connect_async(&url)).await.ok()?.ok()?;

    ws.send(Message::Text(serde_json::to_string(&ClientMessage::Discover).ok()?.into())).await.ok()?;

    let Message::Text(text) = tokio::time::timeout(PROBE_TIMEOUT, ws.next()).await.ok()??.ok()? else {
        return None;
    };
    match serde_json::from_str::<ServerMessage>(&text).ok()? {
        ServerMessage::DiscoverAck { server_name } => Some(DiscoveredHub { host, port, server_name }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn discover_hubs_on_finds_nothing_when_no_one_is_listening() {
        let hubs = discover_hubs_on(vec![Ipv4Addr::LOCALHOST], 65_500).await.unwrap();
        assert!(hubs.is_empty());
    }
}
