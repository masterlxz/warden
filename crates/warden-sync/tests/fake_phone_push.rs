//! Proves `push::begin_push`/`push::run_push_with_hosts` end to end against a scripted "phone" —
//! same fake-phone idiom as `warden-truthid`'s own `tests/fake_phone.rs` (a tiny `axum` server
//! implementing the two single-shot `pin()` endpoints), reused here rather than re-testing
//! `warden-truthid`'s own crypto correctness (that's already covered over there).

use std::net::Ipv4Addr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::Path;
use axum::routing::{get, put};
use axum::Router;
use warden_core::memory::Vault;
use warden_sync::manifest::SyncManifest;
use warden_sync::push;

const FAKE_PHONE_PORT: u16 = warden_truthid::protocol::LAN_PORTS[0];

/// Accepts one `PUT .../content` (doesn't need to decrypt it — that wire format is already
/// covered by `warden-truthid`'s own tests), then serves a canned "pinned" result on `GET`,
/// ECIES-encrypted against whichever pubkey the requester's QR payload carried.
async fn spawn_fake_phone(requester_pub_hex: String) {
    let pushed: Arc<Mutex<bool>> = Arc::new(Mutex::new(false));

    let push_state = pushed.clone();
    let push_route = put(move |Path(_session_id): Path<String>, _body: axum::body::Bytes| {
        let push_state = push_state.clone();
        async move {
            *push_state.lock().unwrap() = true;
            axum::http::StatusCode::OK
        }
    });

    let get_route = get(move |Path(_session_id): Path<String>| {
        let pushed = pushed.clone();
        let requester_pub_hex = requester_pub_hex.clone();
        async move {
            loop {
                if *pushed.lock().unwrap() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            let result_json = r#"{"status":"pinned","cid":"ar://fake-tx-id","contentHash":null,"providersOk":["arweave"],"providersFailed":[]}"#;
            let encrypted = warden_truthid::crypto::ecies_encrypt(result_json.as_bytes(), &requester_pub_hex).unwrap();
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

fn temp_vault(suffix: &str) -> Vault {
    let dir = std::env::temp_dir().join(format!(
        "warden-sync-fake-phone-test-{suffix}-{}",
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    Vault::new(dir)
}

#[tokio::test]
async fn push_completes_a_full_round_trip_against_a_fake_phone() {
    let vault = temp_vault("push");
    vault.write("notes/todo.md", "buy milk").unwrap();
    let secrets = warden_sync::manifest::generate_secrets();
    let manifest = SyncManifest::default();
    let config_path = std::env::temp_dir().join("warden-sync-fake-phone-test-no-config.toml");

    let begin = push::begin_push(&vault, &config_path, &secrets, &manifest).unwrap().unwrap();
    let qr: serde_json::Value = serde_json::from_str(&begin.pending.qr_payload_json().unwrap()).unwrap();
    let requester_pub_hex = qr["ephemeralPubKey"].as_str().unwrap().to_string();

    spawn_fake_phone(requester_pub_hex).await;

    let arweave = warden_sync::ArweaveClient::new("http://127.0.0.1:1/graphql", "http://127.0.0.1:1");
    let (outcome, new_manifest) =
        push::run_push_with_hosts(begin, &arweave, manifest, vec![Ipv4Addr::LOCALHOST]).await.unwrap();

    assert_eq!(outcome.tx_id, "fake-tx-id");
    assert_eq!(outcome.files_changed, 1);
    assert_eq!(new_manifest.last_tx_id.as_deref(), Some("fake-tx-id"));
    assert_eq!(new_manifest.vault_files.get("notes/todo.md"), Some(&warden_sync::diff::sha256_hex(b"buy milk")));
}
