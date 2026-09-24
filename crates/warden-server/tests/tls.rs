//! P36 fatia 2 — a TLS-only hub, over real sockets with real certificates (a throwaway CA from
//! `rcgen` standing in for Let's Encrypt/`tailscale cert`).

mod support;

use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::sync::Arc;

use rcgen::{BasicConstraints, CertificateParams, CertifiedIssuer, IsCa, KeyPair};
use rustls::pki_types::CertificateDer;
use rustls::{ClientConfig, RootCertStore};
use support::{http_get, raw_http, spin_up_server_with_web_ui, spin_up_tls_server, test_web_ui, MockProvider};
use warden_server::{discover_hubs_on, HubTls, ServerConnection, ServerMessage};
use warden_server_protocol::tls::client_config_with_roots;

/// A CA plus a `localhost` leaf it signed — what a hub serves, and what a client must trust.
struct TestCa {
    ca_der: CertificateDer<'static>,
    leaf_pem: String,
    leaf_key_pem: String,
}

impl TestCa {
    fn new() -> Self {
        let mut ca_params = CertificateParams::new(Vec::<String>::new()).unwrap();
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let ca = CertifiedIssuer::self_signed(ca_params, KeyPair::generate().unwrap()).unwrap();

        let leaf_key = KeyPair::generate().unwrap();
        let leaf = CertificateParams::new(vec!["localhost".to_string()]).unwrap().signed_by(&leaf_key, &ca).unwrap();
        Self { ca_der: ca.der().clone(), leaf_pem: leaf.pem(), leaf_key_pem: leaf_key.serialize_pem() }
    }

    fn client_config(&self) -> Arc<ClientConfig> {
        let mut roots = RootCertStore::empty();
        roots.add(self.ca_der.clone()).unwrap();
        client_config_with_roots(roots)
    }

    /// Writes the leaf's cert/key PEM files into `dir` (overwriting), returning their paths.
    fn write_to(&self, dir: &std::path::Path) -> (PathBuf, PathBuf) {
        std::fs::create_dir_all(dir).unwrap();
        let (cert, key) = (dir.join("hub.crt"), dir.join("hub.key"));
        std::fs::write(&cert, &self.leaf_pem).unwrap();
        std::fs::write(&key, &self.leaf_key_pem).unwrap();
        (cert, key)
    }
}

fn temp_dir() -> PathBuf {
    std::env::temp_dir().join(format!(
        "warden-server-tls-test-{}",
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ))
}

async fn hello(url: &str, tls: Arc<ClientConfig>) -> anyhow::Result<ServerConnection> {
    Ok(ServerConnection::handshake_with_tls(url, "dev-1", "Test Device", "test-key", None, Vec::new(), tls).await?.0)
}

#[tokio::test]
async fn wss_client_trusting_the_ca_completes_hello_and_chats() {
    let ca = TestCa::new();
    let (cert, key) = ca.write_to(&temp_dir());
    let addr = spin_up_tls_server(MockProvider::replying("hi over tls"), HubTls::from_pem_files(cert, key, None).unwrap()).await;

    let mut conn = hello(&format!("wss://localhost:{}", addr.port()), ca.client_config()).await.unwrap();
    conn.send(&warden_server::ClientMessage::Chat { message: "hello".into() }).await.unwrap();
    match conn.recv().await.unwrap() {
        Some(ServerMessage::ChatResponse { content, .. }) => assert_eq!(content, "hi over tls"),
        other => panic!("expected ChatResponse, got {other:?}"),
    }
}

#[tokio::test]
async fn wss_client_with_the_default_web_roots_rejects_an_untrusted_certificate() {
    let ca = TestCa::new();
    let (cert, key) = ca.write_to(&temp_dir());
    let addr = spin_up_tls_server(MockProvider::replying("unused"), HubTls::from_pem_files(cert, key, None).unwrap()).await;

    let err = ServerConnection::connect(&format!("wss://localhost:{}", addr.port()), "dev-1", "Test Device", "test-key").await.err().expect("an untrusted cert must fail");
    assert!(format!("{err:#}").to_lowercase().contains("certificate"), "unexpected error: {err:#}");
}

#[tokio::test]
async fn plain_ws_hello_to_a_tls_hub_is_refused_before_the_upgrade() {
    let ca = TestCa::new();
    let (cert, key) = ca.write_to(&temp_dir());
    let addr = spin_up_tls_server(MockProvider::replying("unused"), HubTls::from_pem_files(cert, key, None).unwrap()).await;

    let err = ServerConnection::connect(&format!("ws://{addr}"), "dev-1", "Test Device", "test-key").await.err().expect("plain ws:// must be refused");
    assert!(err.to_string().contains("only accepts encrypted connections"), "unexpected error: {err:#}");
}

#[tokio::test]
async fn plain_discovery_still_finds_a_tls_hub_and_learns_its_wss_url() {
    let ca = TestCa::new();
    let (cert, key) = ca.write_to(&temp_dir());
    let tls = HubTls::from_pem_files(cert, key, Some("localhost".to_string())).unwrap();
    let addr = spin_up_tls_server(MockProvider::replying("unused"), tls).await;

    let hubs = discover_hubs_on(vec![Ipv4Addr::LOCALHOST], addr.port()).await.unwrap();
    assert_eq!(hubs.len(), 1);
    assert_eq!(hubs[0].server_name, "Test Hub");
    let secure_url = hubs[0].secure_url.clone().expect("a TLS hub advertises its wss:// URL");
    assert_eq!(secure_url, format!("wss://localhost:{}", addr.port()));

    // And that URL is the one that works.
    hello(&secure_url, ca.client_config()).await.unwrap();
}

#[tokio::test]
async fn replacing_the_cert_files_takes_effect_without_a_restart() {
    let dir = temp_dir();
    let (old_ca, new_ca) = (TestCa::new(), TestCa::new());
    let (cert, key) = old_ca.write_to(&dir);
    let addr = spin_up_tls_server(MockProvider::replying("unused"), HubTls::from_pem_files(cert, key, None).unwrap()).await;
    let url = format!("wss://localhost:{}", addr.port());

    assert!(hello(&url, new_ca.client_config()).await.is_err(), "the new CA can't verify the old cert yet");

    new_ca.write_to(&dir);
    hello(&url, new_ca.client_config()).await.expect("the renewed cert is served on the next handshake");
    assert!(hello(&url, old_ca.client_config()).await.is_err(), "the old cert is no longer served");
}

// P78 — a TLS hub that also serves the web UI.

#[tokio::test]
async fn https_serves_the_web_ui_and_wss_still_chats_on_the_same_port() {
    let ca = TestCa::new();
    let (cert, key) = ca.write_to(&temp_dir());
    let tls = HubTls::from_pem_files(cert, key, None).unwrap();
    let addr = spin_up_server_with_web_ui(MockProvider::replying("hi over tls"), test_web_ui(), Some(tls)).await;

    let connector = tokio_rustls::TlsConnector::from(ca.client_config());
    let tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
    let stream = connector.connect(rustls::pki_types::ServerName::try_from("localhost").unwrap(), tcp).await.unwrap();
    let response = raw_http(stream, "GET / HTTP/1.1\r\nHost: localhost\r\n\r\n").await;
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n") && response.contains("Warden test UI"), "{response}");

    let mut conn = hello(&format!("wss://localhost:{}", addr.port()), ca.client_config()).await.unwrap();
    conn.send(&warden_server::ClientMessage::Chat { message: "hello".into() }).await.unwrap();
    match conn.recv().await.unwrap() {
        Some(ServerMessage::ChatResponse { content, .. }) => assert_eq!(content, "hi over tls"),
        other => panic!("expected ChatResponse, got {other:?}"),
    }
}

#[tokio::test]
async fn plain_http_to_a_tls_hub_is_redirected_to_https() {
    let ca = TestCa::new();
    let (cert, key) = ca.write_to(&temp_dir());
    let tls = HubTls::from_pem_files(cert, key, Some("localhost".to_string())).unwrap();
    let addr = spin_up_server_with_web_ui(MockProvider::replying("unused"), test_web_ui(), Some(tls)).await;

    let response = http_get(addr, "GET", "/skills?x=1").await;
    assert!(response.starts_with("HTTP/1.1 308 Permanent Redirect\r\n"), "{response}");
    assert!(response.contains(&format!("Location: https://localhost:{}/skills?x=1\r\n", addr.port())), "{response}");
    assert!(!response.contains("Warden test UI"), "no page over plain http: {response}");
}

#[tokio::test]
async fn plain_http_to_a_tls_hub_without_a_public_name_needs_tls() {
    let ca = TestCa::new();
    let (cert, key) = ca.write_to(&temp_dir());
    let addr = spin_up_server_with_web_ui(MockProvider::replying("unused"), test_web_ui(), Some(HubTls::from_pem_files(cert, key, None).unwrap())).await;

    let response = http_get(addr, "GET", "/").await;
    assert!(response.starts_with("HTTP/1.1 426 Upgrade Required\r\n"), "{response}");
}

#[tokio::test]
async fn with_a_web_ui_plain_discovery_still_works_and_plain_hello_is_still_refused() {
    let ca = TestCa::new();
    let (cert, key) = ca.write_to(&temp_dir());
    let tls = HubTls::from_pem_files(cert, key, Some("localhost".to_string())).unwrap();
    let addr = spin_up_server_with_web_ui(MockProvider::replying("unused"), test_web_ui(), Some(tls)).await;

    let hubs = discover_hubs_on(vec![Ipv4Addr::LOCALHOST], addr.port()).await.unwrap();
    assert_eq!(hubs.len(), 1);
    assert_eq!(hubs[0].secure_url.as_deref(), Some(format!("wss://localhost:{}", addr.port()).as_str()));

    let err = ServerConnection::connect(&format!("ws://{addr}"), "dev-1", "Test Device", "test-key").await.err().expect("plain ws:// must be refused");
    assert!(err.to_string().contains("only accepts encrypted connections"), "unexpected error: {err:#}");
}
