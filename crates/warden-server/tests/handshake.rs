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
async fn a_second_hello_on_the_same_connection_is_ignored_not_fatal() {
    let addr = spin_up_server(MockProvider::replying("unused")).await;

    let mut conn = ServerConnection::connect(&format!("ws://{addr}"), "dev-1", "Test Device", "test-key")
        .await
        .unwrap();

    conn.send(&ClientMessage::Hello {
        device_id: "dev-1".to_string(),
        device_name: "Test Device".to_string(),
        auth_key: "test-key".to_string(),
    })
    .await
    .unwrap();
    // The connection should still be alive and answer a ping afterward.
    conn.ping(1).await.unwrap();
    assert!(matches!(conn.recv().await.unwrap(), Some(ServerMessage::Pong { nonce: 1 })));
}
