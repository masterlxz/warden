//! Proves `DecentralizedVaultProvider::import_all_interactive` (P61 follow-up) actually publishes
//! to Arweave — surfacing the QR payload via `on_qr` and completing a real push against a fake
//! TruthID phone — when this device is initialized. Same fake-phone idiom as `warden-truthid`'s
//! own `tests/fake_phone.rs` and this crate's `tests/engine_lifecycle.rs`.
//!
//! Only one `#[tokio::test]` in this file, deliberately — it binds the real
//! `warden_truthid::protocol::LAN_PORTS[0]`, same reasoning as `engine_lifecycle.rs`.

use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::Path as AxumPath;
use axum::routing::{get, put};
use axum::Router;
use warden_core::memory::Vault;
use warden_sync::manifest::{self, SyncManifest};
use warden_sync::{DecentralizedVaultProvider, SyncEngine};

const FAKE_PHONE_PORT: u16 = warden_truthid::protocol::LAN_PORTS[0];
const TX_ID: &str = "tx-import-interactive";

fn temp_base(suffix: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "warden-sync-import-interactive-{suffix}-{}",
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ))
}

/// Fake TruthID phone bound *before* the QR (and so the requester's ephemeral pubkey) is known —
/// unlike `engine_lifecycle.rs`'s version, `requester_pub_hex` is read from shared state at
/// request time, filled in synchronously by `on_qr` (which fires before any network I/O starts),
/// not captured at spawn time.
async fn spawn_fake_phone(requester_pub_hex: Arc<Mutex<Option<String>>>, published: Arc<Mutex<Option<Vec<u8>>>>) {
    let push_route = put(move |AxumPath(session_id): AxumPath<String>, body: axum::body::Bytes| {
        let published = published.clone();
        async move {
            let content_key = warden_truthid::crypto::derive_pin_content_key(&session_id).unwrap();
            let plaintext = warden_truthid::crypto::decrypt_pin_content(&body, &content_key).unwrap();
            *published.lock().unwrap() = Some(plaintext);
            axum::http::StatusCode::OK
        }
    });

    let get_route = get(move |AxumPath(_session_id): AxumPath<String>| {
        let requester_pub_hex = requester_pub_hex.clone();
        async move {
            let pub_hex = requester_pub_hex.lock().unwrap().clone().expect("on_qr should have set the pubkey before any network call");
            let result_json =
                format!(r#"{{"status":"pinned","cid":"ar://{TX_ID}","contentHash":null,"providersOk":["arweave"],"providersFailed":[]}}"#);
            let encrypted = warden_truthid::crypto::ecies_encrypt(result_json.as_bytes(), &pub_hex).unwrap();
            let blob_b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &encrypted);
            axum::Json(serde_json::json!({ "blob": blob_b64 }))
        }
    });

    let router = Router::new().route("/session/{id}/content", push_route).route("/session/{id}", get_route);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", FAKE_PHONE_PORT)).await.expect("FAKE_PHONE_PORT free for the test");
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test]
async fn import_all_interactive_publishes_for_real_against_a_fake_phone() {
    let base = temp_base("push-ok");
    let vault_root = base.join("vault");
    let config_path = base.join("config.toml");
    let secrets_path = base.join("sync_secrets.json");
    let manifest_path = base.join("sync_manifest.json");

    let sync = SyncEngine::new(vault_root.clone(), config_path, secrets_path.clone(), manifest_path.clone());
    sync.init_fresh().unwrap();
    let vault = Arc::new(Vault::new(vault_root));
    let provider = DecentralizedVaultProvider::new(vault, sync);

    let requester_pub_hex: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let published: Arc<Mutex<Option<Vec<u8>>>> = Arc::new(Mutex::new(None));
    spawn_fake_phone(requester_pub_hex.clone(), published.clone()).await;

    let qr_payloads: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let on_qr = {
        let requester_pub_hex = requester_pub_hex.clone();
        let qr_payloads = qr_payloads.clone();
        move |qr_json: String| {
            let qr: serde_json::Value = serde_json::from_str(&qr_json).expect("on_qr must receive valid QR payload JSON");
            let pubkey = qr["ephemeralPubKey"].as_str().expect("QR payload carries ephemeralPubKey").to_string();
            *requester_pub_hex.lock().unwrap() = Some(pubkey);
            qr_payloads.lock().unwrap().push(qr_json);
        }
    };

    let mut data = HashMap::new();
    data.insert("a.md".to_string(), b"hello".to_vec());
    provider.import_all_interactive_with_hosts(data, Some(&on_qr), vec![Ipv4Addr::LOCALHOST]).await.unwrap();

    // `on_qr` fired exactly once, with a real QR payload — proves the interactive path actually
    // surfaced the approval step instead of silently skipping it.
    assert_eq!(qr_payloads.lock().unwrap().len(), 1);
    // The fake phone actually received and decrypted the published bundle.
    assert!(published.lock().unwrap().is_some());
    // The manifest on disk reflects the completed push, same as a real `finish_push` would leave
    // it — proves this isn't just a local write with no Arweave side effect.
    let new_manifest: SyncManifest = manifest::load_manifest(&manifest_path).unwrap();
    assert_eq!(new_manifest.last_tx_id.as_deref(), Some(TX_ID));
}
