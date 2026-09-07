use std::time::Instant;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;
use warden_truthid::crypto::ecies_encrypt;

use crate::manifest::{SyncManifest, SyncSecrets};

use super::protocol::{code_proof, derive_code_key, generate_pairing_code, KeyMaterialPayload, PairingMessage, PAIRING_PORTS, PAIRING_TIMEOUT};

type WsSink = SplitSink<WebSocketStream<TcpStream>, Message>;

/// The device *showing* the code — it already has the vault key, so it's the one anchored and
/// listening; the device the code gets typed into (`join`/`join_with_hosts`) is the one that
/// sweeps the LAN looking for this listener.
pub struct PairingHost {
    code: String,
    listener: TcpListener,
    code_key: [u8; 32],
    key_material: KeyMaterialPayload,
    deadline: Instant,
}

impl PairingHost {
    pub fn code(&self) -> &str {
        &self.code
    }

    /// The actual bound address — useful for tests that need to talk to this host directly
    /// instead of going through `join`'s LAN sweep.
    pub fn local_addr(&self) -> anyhow::Result<std::net::SocketAddr> {
        Ok(self.listener.local_addr()?)
    }

    /// Binds the first free port in `PAIRING_PORTS` and prepares the key material to hand over
    /// once a joiner proves it has the code. Does no accepting yet — call `wait_for_join` for
    /// that.
    pub async fn start(secrets: &SyncSecrets, manifest: &SyncManifest) -> anyhow::Result<Self> {
        let code = generate_pairing_code();
        let code_key = derive_code_key(&code)?;

        let mut bound = None;
        for port in PAIRING_PORTS {
            if let Ok(listener) = TcpListener::bind(("0.0.0.0", port)).await {
                bound = Some(listener);
                break;
            }
        }
        let listener = bound.ok_or_else(|| anyhow::anyhow!("no free port in the pairing range {PAIRING_PORTS:?}"))?;

        let key_material =
            KeyMaterialPayload { vault_key_b64: BASE64.encode(secrets.vault_key), owner_address: manifest.owner_address.clone() };

        Ok(Self { code, listener, code_key, key_material, deadline: Instant::now() + PAIRING_TIMEOUT })
    }

    /// Accepts connections until one completes a full Hello->KeyMaterial->Ack exchange with the
    /// right code, or the timeout elapses. A connection with the *wrong* code gets an `Error` and
    /// the host keeps listening — a mistyped code shouldn't force restarting the whole flow.
    pub async fn wait_for_join(self) -> anyhow::Result<()> {
        let PairingHost { listener, code_key, key_material, deadline, .. } = self;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                anyhow::bail!("pairing timed out waiting for a device to join");
            }
            let accepted = match tokio::time::timeout(remaining, listener.accept()).await {
                Ok(Ok(pair)) => pair,
                Ok(Err(_)) => continue,
                Err(_) => anyhow::bail!("pairing timed out waiting for a device to join"),
            };
            let (stream, _peer) = accepted;
            if handle_one_connection(stream, &code_key, &key_material).await.unwrap_or(false) {
                return Ok(());
            }
        }
    }
}

async fn handle_one_connection(
    stream: TcpStream,
    code_key: &[u8; 32],
    key_material: &KeyMaterialPayload,
) -> anyhow::Result<bool> {
    let ws = tokio_tungstenite::accept_async(stream).await?;
    let (mut sink, mut stream) = ws.split();

    let Some(first) = stream.next().await else {
        return Ok(false);
    };
    let Message::Text(text) = first? else {
        return Ok(false);
    };
    let (proof, joiner_pub) = match serde_json::from_str::<PairingMessage>(&text)? {
        PairingMessage::Hello { code_proof, joiner_ecies_pub, .. } => (code_proof, joiner_ecies_pub),
        _ => return Ok(false),
    };

    if proof != code_proof(code_key) {
        let _ = send(&mut sink, &PairingMessage::Error { reason: "wrong code".to_string() }).await;
        return Ok(false);
    }

    let payload_json = serde_json::to_vec(key_material)?;
    let encrypted = ecies_encrypt(&payload_json, &joiner_pub)?;
    send(&mut sink, &PairingMessage::KeyMaterial { v: 1, payload_b64: BASE64.encode(encrypted) }).await?;

    let Some(ack) = stream.next().await else {
        return Ok(false);
    };
    let Message::Text(text) = ack? else {
        return Ok(false);
    };
    Ok(matches!(serde_json::from_str::<PairingMessage>(&text)?, PairingMessage::Ack { ok: true }))
}

async fn send(sink: &mut WsSink, msg: &PairingMessage) -> anyhow::Result<()> {
    let json = serde_json::to_string(msg)?;
    sink.send(Message::Text(json.into())).await?;
    Ok(())
}
