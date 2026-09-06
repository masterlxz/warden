//! Stands in for a real TruthID mobile app's `RemoteSignerLanServer` — implements the same two
//! single-shot HTTP endpoints (`PUT /session/:id/content`, `GET /session/:id`) so the full
//! `PendingPin` flow (phase 1 push, LAN sweep, phase 2 fetch, ECIES decrypt) can be proven
//! end to end without a real phone. Interop with the *actual* TruthID app remains a separate,
//! accepted gap (see `project/PENDING.md`) — this only proves our own requester and a peer that
//! speaks the documented wire format agree with each other.
//!
//! Bound to `127.0.0.1` on one of the real `LAN_PORTS`, and the requester is pointed at it via
//! `run_with_hosts` instead of the real network sweep (`candidate_hosts`) — sweeping the actual
//! LAN in an automated test would be slow and nondeterministic (hundreds of hosts on whatever
//! network this happens to run on).

use std::net::Ipv4Addr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::Path;
use axum::routing::{get, put};
use axum::Router;
use warden_truthid::crypto;

const FAKE_PHONE_PORT: u16 = warden_truthid::protocol::LAN_PORTS[0];

/// Spawns a fake phone bound to `127.0.0.1:{FAKE_PHONE_PORT}` that: waits for one
/// `PUT .../content`, decrypts it with the phase-1 cipher, checks it matches `expected_content`,
/// then re-encrypts `result_json` with ECIES against the requester's pubkey (read straight out of
/// the pushed session) and serves it on the next `GET` — mirroring the real phone's single-shot
/// endpoints and the fact that phase 2 only starts answering once phase 1 has landed.
async fn spawn_fake_phone(expected_content: &'static [u8], result_json: &'static str) {
    let pushed: Arc<Mutex<Option<Vec<u8>>>> = Arc::new(Mutex::new(None));

    let push_state = pushed.clone();
    let push_route = put(
        move |Path(_session_id): Path<String>, body: axum::body::Bytes| {
            let push_state = push_state.clone();
            async move {
                *push_state.lock().unwrap() = Some(body.to_vec());
                axum::http::StatusCode::OK
            }
        },
    );

    let get_route = get(move |Path(session_id): Path<String>| {
        let pushed = pushed.clone();
        async move {
            let content_key = crypto::derive_pin_content_key(&session_id).unwrap();
            loop {
                let maybe_blob = pushed.lock().unwrap().clone();
                if let Some(blob) = maybe_blob {
                    let plaintext = crypto::decrypt_pin_content(&blob, &content_key).unwrap();
                    assert_eq!(plaintext, expected_content);
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }

            // The requester's ephemeral pubkey isn't visible to this fake phone from the pushed
            // content alone (the real phone gets it from the QR, out of band) — the test wires
            // it in directly via a shared cell instead, set right after the request is built.
            let requester_pub_hex = REQUESTER_PUB_HEX.lock().unwrap().clone().unwrap();
            let encrypted = crypto::ecies_encrypt(result_json.as_bytes(), &requester_pub_hex).unwrap();
            let blob_b64 =
                base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &encrypted);
            axum::Json(serde_json::json!({ "blob": blob_b64 }))
        }
    });

    let router = Router::new()
        .route("/session/{id}/content", push_route)
        .route("/session/{id}", get_route);

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", FAKE_PHONE_PORT))
        .await
        .expect("FAKE_PHONE_PORT free for the test");
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    // Give the listener a moment to actually accept before the requester starts sweeping.
    tokio::time::sleep(Duration::from_millis(50)).await;
}

static REQUESTER_PUB_HEX: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

#[tokio::test]
async fn pending_pin_completes_a_full_round_trip_against_a_fake_phone() {
    let content = b"warden vault blob";
    let result_json = r#"{"status":"pinned","cid":"ar://fake-tx-id","contentHash":"0xdead","providersOk":["arweave"],"providersFailed":[]}"#;

    let pending = warden_truthid::PendingPin::begin("Warden", Duration::from_secs(20)).unwrap();
    let qr: serde_json::Value = serde_json::from_str(&pending.qr_payload_json().unwrap()).unwrap();
    assert_eq!(qr["action"], "truthid-pin");
    *REQUESTER_PUB_HEX.lock().unwrap() = Some(qr["ephemeralPubKey"].as_str().unwrap().to_string());

    spawn_fake_phone(content, result_json).await;

    let result = pending
        .run_with_hosts(content, vec![Ipv4Addr::LOCALHOST])
        .await
        .unwrap();

    assert_eq!(result.status, "pinned");
    assert_eq!(result.cid.as_deref(), Some("ar://fake-tx-id"));
    assert_eq!(result.providers_ok.as_deref(), Some(&["arweave".to_string()][..]));
}
