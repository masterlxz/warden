//! End-to-end lifecycle test for `SyncEngine`, simulating two separate Warden installs ("device
//! A" and "device B") on two temp directories: A initializes sync and pushes, B pairs with A
//! (learning the vault key + owner address without ever touching TruthID/Arweave) and pulls,
//! ending up with a vault that matches A's. Uses the same fake-phone idiom as
//! `warden-truthid`'s own tests (`tests/fake_phone.rs`) plus a fake Arweave gateway, since no real
//! TruthID app or Arweave network is available in this environment (see `PENDING.md`, same
//! accepted gap as P38).
//!
//! Only one `#[tokio::test]` in this file, deliberately — it binds the real
//! `warden_truthid::protocol::LAN_PORTS[0]`, and cargo runs tests within one binary concurrently
//! by default; a second test here would race for that port.

use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::Path as AxumPath;
use axum::routing::{get, post, put};
use axum::Router;
use warden_sync::{ArweaveClient, SyncEngine};

const FAKE_PHONE_PORT: u16 = warden_truthid::protocol::LAN_PORTS[0];
const OWNER_ADDRESS: &str = "wallet-test-owner";
const TX_ID: &str = "tx-1";

struct TestDevice {
    engine: SyncEngine,
    vault_root: PathBuf,
}

fn make_device(name: &str, arweave: ArweaveClient) -> TestDevice {
    let base = std::env::temp_dir().join(format!(
        "warden-sync-engine-lifecycle-{name}-{}",
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    let vault_root = base.join("vault");
    let engine = SyncEngine::new(
        vault_root.clone(),
        base.join("config.toml"),
        base.join("sync_secrets.json"),
        base.join("sync_manifest.json"),
    )
    .with_arweave_client(arweave);
    TestDevice { engine, vault_root }
}

fn fake_arweave_client(base_url: &str) -> ArweaveClient {
    ArweaveClient::new(format!("{base_url}/graphql"), base_url.to_string())
}

/// Fake Arweave gateway: any "latest by owner" query answers `TX_ID`, any "owner of tx" query
/// answers `OWNER_ADDRESS`, and fetching `TX_ID`'s data returns whatever the fake phone below
/// "published" (i.e. the vault-encrypted bundle bytes A's push produced).
async fn spawn_fake_gateway(published: Arc<Mutex<Option<Vec<u8>>>>) -> String {
    let router = Router::new()
        .route(
            "/graphql",
            post(|body: axum::Json<serde_json::Value>| async move {
                let query = body["query"].as_str().unwrap_or_default();
                if query.contains("transactions(") {
                    axum::Json(serde_json::json!({
                        "data": { "transactions": { "edges": [ { "node": { "id": TX_ID } } ] } }
                    }))
                } else {
                    axum::Json(serde_json::json!({
                        "data": { "transaction": { "owner": { "address": OWNER_ADDRESS } } }
                    }))
                }
            }),
        )
        .route(
            "/{tx_id}",
            get(move |AxumPath(_tx_id): AxumPath<String>| {
                let published = published.clone();
                async move { published.lock().unwrap().clone().expect("push should have published by the time pull fetches") }
            }),
        );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    format!("http://{addr}")
}

/// Fake TruthID phone: decrypts the phase-1 LAN transport layer to recover our vault-encrypted
/// bundle bytes (stashing them in `published` for the fake gateway to serve later), then answers
/// with a canned "pinned" `PinResult` pointing at `TX_ID`.
async fn spawn_fake_phone(requester_pub_hex: String, published: Arc<Mutex<Option<Vec<u8>>>>) {
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
            let result_json = format!(
                r#"{{"status":"pinned","cid":"ar://{TX_ID}","contentHash":null,"providersOk":["arweave"],"providersFailed":[]}}"#
            );
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

#[tokio::test]
async fn a_pushes_b_pairs_and_pulls_and_ends_up_with_a_matching_vault() {
    let published: Arc<Mutex<Option<Vec<u8>>>> = Arc::new(Mutex::new(None));
    let gateway_url = spawn_fake_gateway(published.clone()).await;

    let device_a = make_device("a", fake_arweave_client(&gateway_url));
    device_a.engine.init_fresh().unwrap();
    std::fs::create_dir_all(&device_a.vault_root).unwrap();
    std::fs::write(device_a.vault_root.join("todo.md"), "buy milk").unwrap();

    let begin = device_a.engine.begin_push().unwrap().unwrap();
    let qr: serde_json::Value = serde_json::from_str(&begin.pending.qr_payload_json().unwrap()).unwrap();
    let requester_pub_hex = qr["ephemeralPubKey"].as_str().unwrap().to_string();

    spawn_fake_phone(requester_pub_hex, published.clone()).await;

    let push_outcome = device_a.engine.finish_push_with_hosts(begin, vec![Ipv4Addr::LOCALHOST]).await.unwrap();
    assert_eq!(push_outcome.tx_id, TX_ID);

    let status_a = device_a.engine.status().unwrap();
    assert_eq!(status_a.owner_address.as_deref(), Some(OWNER_ADDRESS));

    // B pairs with A — learns the vault key + owner address, no TruthID/Arweave involved.
    let device_b = make_device("b", fake_arweave_client(&gateway_url));
    let host = device_a.engine.pairing_host().await.unwrap();
    let code = host.code().to_string();
    let host_task = tokio::spawn(host.wait_for_join());
    device_b.engine.pairing_join_with_hosts(&code, vec![Ipv4Addr::LOCALHOST]).await.unwrap();
    host_task.await.unwrap().unwrap();

    let status_b_before_pull = device_b.engine.status().unwrap();
    assert!(status_b_before_pull.paired);
    assert_eq!(status_b_before_pull.owner_address.as_deref(), Some(OWNER_ADDRESS));

    let pull_outcome = device_b.engine.pull().await.unwrap();
    assert_eq!(pull_outcome.files_written, 1);
    assert_eq!(pull_outcome.tx_id.as_deref(), Some(TX_ID));

    assert_eq!(std::fs::read_to_string(device_b.vault_root.join("todo.md")).unwrap(), "buy milk");
}
