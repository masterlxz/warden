use warden_server::{Server, ServerConnection, ServerMessage};

#[tokio::test]
async fn hello_handshake_and_heartbeat_round_trip() {
    let server = Server::bind("127.0.0.1:0".parse().unwrap(), "test-key")
        .await
        .unwrap();
    let addr = server.local_addr().unwrap();
    tokio::spawn(server.serve());

    let mut conn = ServerConnection::connect(&format!("ws://{addr}"), "dev-1", "Test Device", "test-key")
        .await
        .unwrap();

    conn.ping(42).await.unwrap();
    let reply = conn.recv().await.unwrap();
    assert!(matches!(reply, Some(ServerMessage::Pong { nonce: 42 })));
}

#[tokio::test]
async fn wrong_auth_key_is_rejected() {
    let server = Server::bind("127.0.0.1:0".parse().unwrap(), "test-key")
        .await
        .unwrap();
    let addr = server.local_addr().unwrap();
    tokio::spawn(server.serve());

    let result = ServerConnection::connect(&format!("ws://{addr}"), "dev-1", "Test Device", "wrong-key")
        .await;

    match result {
        Ok(_) => panic!("expected the connection to be rejected"),
        Err(err) => assert!(err.to_string().contains("authentication rejected")),
    }
}

#[tokio::test]
async fn multiple_pings_on_the_same_connection_all_get_replies() {
    let server = Server::bind("127.0.0.1:0".parse().unwrap(), "test-key")
        .await
        .unwrap();
    let addr = server.local_addr().unwrap();
    tokio::spawn(server.serve());

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
