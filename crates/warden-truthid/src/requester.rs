use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures_util::stream::{self, StreamExt};
use rand_core::{OsRng, RngCore};

use crate::crypto::{self, EciesKeyPair};
use crate::lan::{self, candidate_hosts};
use crate::protocol::{PinResult, QrPayload, DEFAULT_TIMEOUT, LAN_PORTS};

/// How long to wait for one host:port probe to answer before moving to the next sweep pass.
const SWEEP_RETRY_INTERVAL: Duration = Duration::from_secs(2);

/// A `truthid-pin` request in flight — mirrors the Dart SDK's `PendingRequest<PinResult>`:
/// the QR payload is ready immediately (`qr_payload_json`), the actual network exchange
/// (`run`) is a separate, awaitable step so a caller can render the QR before blocking.
pub struct PendingPin {
    payload: QrPayload,
    ecies: EciesKeyPair,
    deadline: SystemTime,
}

impl PendingPin {
    /// Starts a new pin session: generates a session id + ephemeral ECIES keypair and builds
    /// the QR payload. Does no network I/O yet.
    pub fn begin(app_name: &str, timeout: Duration) -> anyhow::Result<Self> {
        let mut session_id_bytes = [0u8; 16];
        OsRng.fill_bytes(&mut session_id_bytes);
        let session_id = hex::encode(session_id_bytes);

        let ecies = crypto::generate_ecies_keypair();
        let expires_at = SystemTime::now() + timeout;
        let expires_at_ms = expires_at
            .duration_since(UNIX_EPOCH)?
            .as_millis()
            .try_into()?;

        let payload = QrPayload {
            action: "truthid-pin",
            v: 1,
            session_id,
            ephemeral_pub_key: ecies.public_hex.clone(),
            expires_at: expires_at_ms,
            app_name: app_name.to_string(),
        };

        Ok(Self {
            payload,
            ecies,
            deadline: expires_at,
        })
    }

    pub fn begin_with_default_timeout(app_name: &str) -> anyhow::Result<Self> {
        Self::begin(app_name, DEFAULT_TIMEOUT)
    }

    /// The JSON a caller renders as a QR code for the user to scan with the TruthID app.
    pub fn qr_payload_json(&self) -> anyhow::Result<String> {
        Ok(serde_json::to_string(&self.payload)?)
    }

    /// Pushes `content` (already encrypted by the caller, if it's meant to stay confidential —
    /// this layer's own cipher is just transport confidentiality for the LAN hop) to whichever
    /// phone answers the sweep, then waits for and decrypts the phone's result. Sweeps every
    /// candidate host on the machine's local networks — see `run_with_hosts` to target a
    /// specific, already-known set of hosts instead (also what the test suite uses, to avoid
    /// sweeping the real LAN in automated tests).
    pub async fn run(self, content: &[u8]) -> anyhow::Result<PinResult> {
        self.run_with_host_source(content, candidate_hosts).await
    }

    /// Same as `run`, but sweeps only `hosts` instead of discovering them via `candidate_hosts`.
    pub async fn run_with_hosts(
        self,
        content: &[u8],
        hosts: Vec<std::net::Ipv4Addr>,
    ) -> anyhow::Result<PinResult> {
        self.run_with_host_source(content, move || Ok(hosts.clone()))
            .await
    }

    async fn run_with_host_source(
        self,
        content: &[u8],
        host_source: impl Fn() -> anyhow::Result<Vec<std::net::Ipv4Addr>>,
    ) -> anyhow::Result<PinResult> {
        let session_id = self.payload.session_id.clone();
        let content_key = crypto::derive_pin_content_key(&session_id)?;
        let encrypted_content = crypto::encrypt_pin_content(content, &content_key)?;

        self.sweep_until(
            |host, port| {
                let session_id = session_id.clone();
                let blob = encrypted_content.clone();
                async move {
                    lan::push_content(host, port, &session_id, &blob)
                        .await
                        .map(|ok| ok.then_some(()))
                }
            },
            &host_source,
        )
        .await?
        .ok_or_else(|| anyhow::anyhow!("timed out waiting for a phone to accept the pin content"))?;

        let result_blob = self
            .sweep_until(
                |host, port| {
                    let session_id = session_id.clone();
                    async move { lan::fetch_result(host, port, &session_id).await }
                },
                &host_source,
            )
            .await?
            .ok_or_else(|| anyhow::anyhow!("timed out waiting for the phone's pin result"))?;

        let plaintext = crypto::ecies_decrypt(&result_blob, &self.ecies.secret)?;
        Ok(serde_json::from_slice(&plaintext)?)
    }

    /// Repeatedly sweeps every host `host_source` returns × the fixed LAN port block
    /// concurrently (mirrors the Dart client's batched sweep, `lan_sweep_client.dart`), calling
    /// `probe` on each, until one returns `Some(_)` or the session's deadline passes.
    async fn sweep_until<T, F, Fut>(
        &self,
        probe: F,
        host_source: &impl Fn() -> anyhow::Result<Vec<std::net::Ipv4Addr>>,
    ) -> anyhow::Result<Option<T>>
    where
        F: Fn(std::net::Ipv4Addr, u16) -> Fut,
        Fut: std::future::Future<Output = anyhow::Result<Option<T>>>,
    {
        const CONCURRENCY: usize = 50;
        loop {
            let hosts = host_source()?;
            let targets: Vec<(std::net::Ipv4Addr, u16)> = hosts
                .iter()
                .flat_map(|host| LAN_PORTS.iter().map(move |port| (*host, *port)))
                .collect();

            let mut attempts = stream::iter(targets)
                .map(|(host, port)| probe(host, port))
                .buffer_unordered(CONCURRENCY);
            while let Some(outcome) = attempts.next().await {
                if let Some(value) = outcome? {
                    return Ok(Some(value));
                }
            }

            if SystemTime::now() >= self.deadline {
                return Ok(None);
            }
            tokio::time::sleep(SWEEP_RETRY_INTERVAL).await;
        }
    }
}
