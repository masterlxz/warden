//! TLS for the hub (P36 fatia 2): serving `wss://` with a certificate that every client can verify
//! without pinning — meant to come from `tailscale cert` (Let's Encrypt, for the machine's
//! MagicDNS name), which works as-is in browsers and on Android/iOS.
//!
//! Two pieces:
//! - `HubTls` — PEM cert/key files → a `TlsAcceptor`. The files are re-read on the next handshake
//!   whenever their modification time changes (`ReloadingCertResolver`), so renewing the cert
//!   (Tailscale's expire after 90 days) never needs a restart — whether `tailscale_cert_renewal`
//!   does it or the operator's own cron/systemd timer does.
//! - Tailscale helpers — find this machine's MagicDNS name and (re)fetch its cert by shelling out
//!   to the `tailscale` CLI. No Tailscale API/library dependency: the CLI is already the operator's
//!   install, and `tailscale cert` handles the ACME exchange and its own renewal threshold.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use anyhow::Context;
use rustls::crypto::CryptoProvider;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use rustls::ServerConfig;
use tokio_rustls::TlsAcceptor;

/// A hub's TLS setup: what accepts the handshake, plus the URL advertised to plain-`ws://`
/// discovery probes so they know where to go instead (`DiscoverAck.secure_url`).
#[derive(Clone)]
pub struct HubTls {
    pub(crate) acceptor: TlsAcceptor,
    /// Host name the certificate is valid for, e.g. `hub.tail1234.ts.net` — combined with the
    /// bound port into `wss://{host}:{port}`. `None` = still TLS-only, just nothing to advertise.
    pub(crate) public_host: Option<String>,
}

impl HubTls {
    /// Loads `cert_path` (PEM chain, leaf first) and `key_path` (PEM private key). Fails right
    /// away on unreadable/invalid files — a later reload failure only logs and keeps the old cert.
    pub fn from_pem_files(cert_path: impl Into<PathBuf>, key_path: impl Into<PathBuf>, public_host: Option<String>) -> anyhow::Result<Self> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let resolver = ReloadingCertResolver::new(cert_path.into(), key_path.into(), provider.clone())?;
        let config = ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .context("TLS provider doesn't support the default protocol versions")?
            .with_no_client_auth()
            .with_cert_resolver(Arc::new(resolver));
        Ok(Self { acceptor: TlsAcceptor::from(Arc::new(config)), public_host })
    }

    pub fn secure_url(&self, port: u16) -> Option<String> {
        self.public_host.as_ref().map(|host| format!("wss://{host}:{port}"))
    }
}

/// Serves whatever is currently in the cert/key files, re-reading them when either one's
/// modification time changes. One `stat` pair per handshake — negligible next to the handshake.
#[derive(Debug)]
struct ReloadingCertResolver {
    cert_path: PathBuf,
    key_path: PathBuf,
    provider: Arc<CryptoProvider>,
    current: Mutex<LoadedCert>,
}

#[derive(Debug)]
struct LoadedCert {
    key: Arc<CertifiedKey>,
    mtimes: (SystemTime, SystemTime),
}

impl ReloadingCertResolver {
    fn new(cert_path: PathBuf, key_path: PathBuf, provider: Arc<CryptoProvider>) -> anyhow::Result<Self> {
        let current = load_cert(&cert_path, &key_path, &provider)?;
        Ok(Self { cert_path, key_path, provider, current: Mutex::new(current) })
    }
}

impl ResolvesServerCert for ReloadingCertResolver {
    fn resolve(&self, _client_hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        let mut current = self.current.lock().unwrap();
        let changed = match (modified(&self.cert_path), modified(&self.key_path)) {
            (Ok(cert), Ok(key)) => (cert, key) != current.mtimes,
            // Mid-rewrite (or deleted) — keep serving what's loaded rather than fail handshakes.
            _ => false,
        };
        if changed {
            match load_cert(&self.cert_path, &self.key_path, &self.provider) {
                Ok(fresh) => {
                    eprintln!("warden-server: reloaded TLS certificate from {}", self.cert_path.display());
                    *current = fresh;
                }
                // Also possible mid-rewrite (cert already new, key still old); the next handshake
                // tries again since `mtimes` wasn't updated.
                Err(err) => eprintln!("warden-server: TLS certificate changed but couldn't be reloaded, keeping the old one: {err:#}"),
            }
        }
        Some(current.key.clone())
    }
}

fn modified(path: &Path) -> std::io::Result<SystemTime> {
    std::fs::metadata(path)?.modified()
}

fn load_cert(cert_path: &Path, key_path: &Path, provider: &CryptoProvider) -> anyhow::Result<LoadedCert> {
    // mtimes read *before* the contents: if a write lands in between, the next handshake sees a
    // newer mtime than the one stored and loads again, instead of missing it.
    let mtimes = (modified(cert_path)?, modified(key_path)?);
    let certs = CertificateDer::pem_file_iter(cert_path)
        .with_context(|| format!("failed to read TLS certificate {}", cert_path.display()))?
        .collect::<Result<Vec<_>, _>>()
        .with_context(|| format!("invalid PEM in {}", cert_path.display()))?;
    anyhow::ensure!(!certs.is_empty(), "no certificate found in {}", cert_path.display());
    let key = PrivateKeyDer::from_pem_file(key_path).with_context(|| format!("failed to read TLS private key {}", key_path.display()))?;
    let signing_key = provider.key_provider.load_private_key(key).context("unsupported TLS private key")?;
    let certified = CertifiedKey::new(certs, signing_key);
    certified.keys_match().context("TLS certificate and private key don't match")?;
    Ok(LoadedCert { key: Arc::new(certified), mtimes })
}

/// How often `tailscale_cert_renewal` re-runs `tailscale cert` — it's a no-op until the cert is
/// close to expiring (Tailscale decides), so daily is plenty for a 90-day cert.
pub const TAILSCALE_RENEWAL_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// This machine's MagicDNS name (e.g. `hub.tail1234.ts.net`), from `tailscale status --json`.
/// Fails when Tailscale isn't installed/running or MagicDNS/HTTPS certs aren't enabled.
pub async fn tailscale_dns_name() -> anyhow::Result<String> {
    let output = tokio::process::Command::new("tailscale")
        .args(["status", "--json"])
        .output()
        .await
        .context("failed to run `tailscale status` — is Tailscale installed and on PATH?")?;
    anyhow::ensure!(output.status.success(), "`tailscale status` failed: {}", String::from_utf8_lossy(&output.stderr).trim());
    parse_dns_name(&output.stdout)
}

fn parse_dns_name(status_json: &[u8]) -> anyhow::Result<String> {
    let status: serde_json::Value = serde_json::from_slice(status_json).context("`tailscale status --json` returned invalid JSON")?;
    let name = status["Self"]["DNSName"].as_str().unwrap_or_default().trim_end_matches('.');
    anyhow::ensure!(!name.is_empty(), "this machine has no MagicDNS name — enable MagicDNS in the Tailscale admin console");
    Ok(name.to_string())
}

/// Runs `tailscale cert` for `domain`, writing the PEM files to `cert_path`/`key_path`. Needs
/// HTTPS certificates enabled for the tailnet, and root or `tailscale set --operator=$USER`.
pub async fn fetch_tailscale_cert(domain: &str, cert_path: &Path, key_path: &Path) -> anyhow::Result<()> {
    if let Some(dir) = cert_path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("failed to create {}", dir.display()))?;
    }
    let output = tokio::process::Command::new("tailscale")
        .arg("cert")
        .arg("--cert-file")
        .arg(cert_path)
        .arg("--key-file")
        .arg(key_path)
        .arg(domain)
        .output()
        .await
        .context("failed to run `tailscale cert`")?;
    anyhow::ensure!(
        output.status.success(),
        "`tailscale cert {domain}` failed: {} (HTTPS certificates must be enabled for the tailnet, and a non-root user needs `sudo tailscale set --operator=$USER`)",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(())
}

/// Re-runs `fetch_tailscale_cert` every `TAILSCALE_RENEWAL_INTERVAL`, forever. Failures only log:
/// the current cert stays valid for weeks after the first renewal attempt, so a transient error
/// (Tailscale restarting, no network) isn't worth taking the hub down over.
pub async fn tailscale_cert_renewal(domain: String, cert_path: PathBuf, key_path: PathBuf) {
    let mut interval = tokio::time::interval(TAILSCALE_RENEWAL_INTERVAL);
    interval.tick().await;
    loop {
        interval.tick().await;
        if let Err(err) = fetch_tailscale_cert(&domain, &cert_path, &key_path).await {
            eprintln!("warden-server: TLS certificate renewal failed, will retry in a day: {err:#}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_dns_name_trims_the_trailing_dot() {
        let json = br#"{"Self":{"DNSName":"hub.tail1234.ts.net.","HostName":"hub"}}"#;
        assert_eq!(parse_dns_name(json).unwrap(), "hub.tail1234.ts.net");
    }

    #[test]
    fn parse_dns_name_fails_without_magic_dns() {
        assert!(parse_dns_name(br#"{"Self":{"DNSName":""}}"#).is_err());
        assert!(parse_dns_name(br#"{"BackendState":"Stopped"}"#).is_err());
    }
}
