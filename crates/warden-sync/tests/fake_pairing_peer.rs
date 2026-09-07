use std::net::{Ipv4Addr, SocketAddr};

use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;
use warden_sync::manifest::{generate_secrets, SyncManifest};
use warden_sync::pairing::protocol::{code_proof, derive_code_key, PairingMessage};
use warden_sync::pairing::{join_with_hosts, PairingHost};

#[tokio::test]
async fn correct_code_completes_the_handshake_and_hands_over_the_vault_key() {
    let secrets = generate_secrets();
    let manifest = SyncManifest {
        owner_address: Some("wallet-abc".to_string()),
        last_tx_id: Some("tx-1".to_string()),
        ..Default::default()
    };

    let host = PairingHost::start(&secrets, &manifest).await.unwrap();
    let code = host.code().to_string();
    let host_task = tokio::spawn(host.wait_for_join());

    let joined = join_with_hosts(&code, vec![Ipv4Addr::LOCALHOST]).await.unwrap();

    host_task.await.unwrap().unwrap();
    assert_eq!(joined.vault_key, secrets.vault_key);
    assert_eq!(joined.owner_address.as_deref(), Some("wallet-abc"));
}

#[tokio::test]
async fn wrong_code_is_rejected_and_the_host_keeps_listening_for_a_retry() {
    let secrets = generate_secrets();
    let manifest = SyncManifest::default();
    let host = PairingHost::start(&secrets, &manifest).await.unwrap();
    let code = host.code().to_string();
    let port = host.local_addr().unwrap().port();
    let host_task = tokio::spawn(host.wait_for_join());

    // Manually attempt the handshake with a wrong code — this bypasses `join`'s multi-minute
    // retry loop (which would treat the resulting `Error` as "not it" and keep sweeping for the
    // full timeout), so the test can assert the rejection quickly.
    let addr = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port);
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}")).await.unwrap();
    let ecies = warden_truthid::crypto::generate_ecies_keypair();
    let wrong_key = derive_code_key("WRONGONE").unwrap();
    let hello = PairingMessage::Hello {
        v: 1,
        code_proof: code_proof(&wrong_key),
        joiner_ecies_pub: ecies.public_hex,
        joiner_device_name: "test-wrong-code".to_string(),
    };
    ws.send(Message::Text(serde_json::to_string(&hello).unwrap().into())).await.unwrap();
    let reply = ws.next().await.unwrap().unwrap();
    let Message::Text(text) = reply else { panic!("expected a text frame") };
    let parsed: PairingMessage = serde_json::from_str(&text).unwrap();
    assert!(matches!(parsed, PairingMessage::Error { .. }));
    drop(ws);

    // The host should still be listening — a second attempt with the right code succeeds.
    let joined = join_with_hosts(&code, vec![Ipv4Addr::LOCALHOST]).await.unwrap();
    host_task.await.unwrap().unwrap();
    assert_eq!(joined.vault_key, secrets.vault_key);
}
