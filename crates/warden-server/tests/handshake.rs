mod support;

use support::{spin_up_server, MockProvider};
use warden_server::{ClientMessage, ServerConnection, ServerMessage};

#[tokio::test]
async fn hello_handshake_and_heartbeat_round_trip() {
    let addr = spin_up_server(MockProvider::replying("unused")).await;

    let mut conn = ServerConnection::connect(&format!("ws://{addr}"), "dev-1", "Test Device", "test-key")
        .await
        .unwrap();

    conn.ping(42).await.unwrap();
    let reply = conn.recv().await.unwrap();
    assert!(matches!(reply, Some(ServerMessage::Pong { nonce: 42 })));
}

#[tokio::test]
async fn wrong_auth_key_is_rejected() {
    let addr = spin_up_server(MockProvider::replying("unused")).await;

    let result = ServerConnection::connect(&format!("ws://{addr}"), "dev-1", "Test Device", "wrong-key").await;

    match result {
        Ok(_) => panic!("expected the connection to be rejected"),
        Err(err) => assert!(err.to_string().contains("authentication rejected")),
    }
}

#[tokio::test]
async fn multiple_pings_on_the_same_connection_all_get_replies() {
    let addr = spin_up_server(MockProvider::replying("unused")).await;

    let mut conn = ServerConnection::connect(&format!("ws://{addr}"), "dev-1", "Test Device", "test-key")
        .await
        .unwrap();

    for nonce in [1_u64, 2, 3] {
        conn.ping(nonce).await.unwrap();
        assert!(matches!(
            conn.recv().await.unwrap(),
            Some(ServerMessage::Pong { nonce: n }) if n == nonce
        ));
    }
}

#[tokio::test]
async fn discover_gets_an_ack_with_the_hub_name_and_no_device_registration() {
    let (addr, devices_path) = support::spin_up_server_with_devices_path(MockProvider::replying("unused")).await;

    let hubs = warden_server::discover_hubs_on(vec![std::net::Ipv4Addr::LOCALHOST], addr.port()).await.unwrap();

    assert_eq!(hubs.len(), 1);
    assert_eq!(hubs[0].server_name, "Test Hub");
    // A discovery probe never becomes a "connected device" — the pairing registry (Fase 9.3)
    // stays empty, unlike a real Hello (see `hello_handshake_and_heartbeat_round_trip` and
    // `PairingStore::record_seen`).
    let store = warden_server::PairingStore::new(devices_path);
    assert!(store.list().unwrap().is_empty());
}

#[tokio::test]
async fn discover_finds_nothing_on_a_host_with_no_server() {
    let hubs = warden_server::discover_hubs_on(vec![std::net::Ipv4Addr::LOCALHOST], 1).await.unwrap();
    assert!(hubs.is_empty());
}

#[tokio::test]
async fn a_second_hello_on_the_same_connection_is_ignored_not_fatal() {
    let addr = spin_up_server(MockProvider::replying("unused")).await;

    let mut conn = ServerConnection::connect(&format!("ws://{addr}"), "dev-1", "Test Device", "test-key")
        .await
        .unwrap();

    conn.send(&ClientMessage::Hello {
        device_id: "dev-1".to_string(),
        device_name: "Test Device".to_string(),
        auth_key: "test-key".to_string(),
        tools: Vec::new(),
    })
    .await
    .unwrap();
    // The connection should still be alive and answer a ping afterward.
    conn.ping(1).await.unwrap();
    assert!(matches!(conn.recv().await.unwrap(), Some(ServerMessage::Pong { nonce: 1 })));
}
