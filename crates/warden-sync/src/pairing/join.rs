use std::net::Ipv4Addr;
use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use futures_util::stream::{self, StreamExt};
use futures_util::SinkExt;
use tokio_tungstenite::tungstenite::Message;
use warden_truthid::crypto::{ecies_decrypt, generate_ecies_keypair};
use warden_truthid::lan::candidate_hosts;

use super::protocol::{code_proof, derive_code_key, KeyMaterialPayload, PairingMessage, PAIRING_PORTS, PAIRING_TIMEOUT};

const CONNECT_TIMEOUT: Duration = Duration::from_millis(800);
const SWEEP_RETRY_INTERVAL: Duration = Duration::from_secs(2);
const CONCURRENCY: usize = 50;

pub struct JoinedKeyMaterial {
    pub vault_key: [u8; 32],
    pub owner_address: Option<String>,
}

/// Sweeps every local network for a `PairingHost` listening with this `code`, joins it, and
/// returns the vault key it hands over.
pub async fn join(code: &str) -> anyhow::Result<JoinedKeyMaterial> {
    join_with_hosts(code, candidate_hosts()?).await
}

/// Same as `join`, but sweeps only `hosts` — used by tests to avoid sweeping the real LAN, same
/// reasoning as `warden_truthid::requester::PendingPin::run_with_hosts`.
pub async fn join_with_hosts(code: &str, hosts: Vec<Ipv4Addr>) -> anyhow::Result<JoinedKeyMaterial> {
    let code_key = derive_code_key(code)?;
    let proof = code_proof(&code_key);
    let deadline = Instant::now() + PAIRING_TIMEOUT;

    loop {
        let targets: Vec<(Ipv4Addr, u16)> =
            hosts.iter().flat_map(|host| PAIRING_PORTS.iter().map(move |port| (*host, *port))).collect();

        let mut attempts =
            stream::iter(targets).map(|(host, port)| try_join_one(host, port, proof.clone())).buffer_unordered(CONCURRENCY);
        while let Some(outcome) = attempts.next().await {
            if let Some(material) = outcome? {
                return Ok(material);
            }
        }

        if Instant::now() >= deadline {
            anyhow::bail!("pairing timed out — no device answered with this code on the local network");
        }
        tokio::time::sleep(SWEEP_RETRY_INTERVAL).await;
    }
}

/// One connect+Hello+KeyMaterial+Ack attempt against a single `host:port`. `Ok(None)` covers
/// every "this wasn't it" outcome (nothing listening, wrong code, malformed reply) — none of
/// those should abort the sweep, since a different host on the LAN might still be the real host.
async fn try_join_one(host: Ipv4Addr, port: u16, proof: String) -> anyhow::Result<Option<JoinedKeyMaterial>> {
    let url = format!("ws://{host}:{port}");
    let Ok(Ok((ws_stream, _))) = tokio::time::timeout(CONNECT_TIMEOUT, tokio_tungstenite::connect_async(&url)).await
    else {
        return Ok(None);
    };
    let (mut sink, mut stream) = ws_stream.split();

    let ecies = generate_ecies_keypair();
    let hello = PairingMessage::Hello {
        v: 1,
        code_proof: proof,
        joiner_ecies_pub: ecies.public_hex.clone(),
        joiner_device_name: device_name(),
    };
    if sink.send(Message::Text(serde_json::to_string(&hello)?.into())).await.is_err() {
        return Ok(None);
    }

    let Ok(Some(Ok(Message::Text(text)))) = tokio::time::timeout(CONNECT_TIMEOUT, stream.next()).await else {
        return Ok(None);
    };
    let material = match serde_json::from_str::<PairingMessage>(&text) {
        Ok(PairingMessage::KeyMaterial { payload_b64, .. }) => {
            let encrypted = BASE64.decode(payload_b64.as_bytes())?;
            let plaintext = ecies_decrypt(&encrypted, &ecies.secret)?;
            let payload: KeyMaterialPayload = serde_json::from_slice(&plaintext)?;
            payload
        }
        _ => return Ok(None),
    };

    let ack = PairingMessage::Ack { ok: true };
    let _ = sink.send(Message::Text(serde_json::to_string(&ack)?.into())).await;

    let vault_key: [u8; 32] = BASE64
        .decode(material.vault_key_b64.as_bytes())?
        .try_into()
        .map_err(|_| anyhow::anyhow!("pairing host sent a vault key that isn't 32 bytes"))?;

    Ok(Some(JoinedKeyMaterial { vault_key, owner_address: material.owner_address }))
}

fn device_name() -> String {
    std::env::var("HOSTNAME").or_else(|_| std::env::var("COMPUTERNAME")).unwrap_or_else(|_| "warden-device".to_string())
}
