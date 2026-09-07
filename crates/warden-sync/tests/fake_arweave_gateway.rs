use axum::extract::Path as AxumPath;
use axum::routing::{get, post};
use axum::Router;
use warden_sync::ArweaveClient;

async fn spawn_fake_gateway() -> String {
    let router = Router::new()
        .route(
            "/graphql",
            post(|body: axum::Json<serde_json::Value>| async move {
                let query = body["query"].as_str().unwrap_or_default();
                if query.contains("transactions(") {
                    axum::Json(serde_json::json!({
                        "data": { "transactions": { "edges": [ { "node": { "id": "real-tx-id" } } ] } }
                    }))
                } else {
                    axum::Json(serde_json::json!({
                        "data": { "transaction": { "owner": { "address": "real-owner-address" } } }
                    }))
                }
            }),
        )
        .route("/{tx_id}", get(|AxumPath(_tx_id): AxumPath<String>| async { b"encrypted-bytes".to_vec() }));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn latest_tx_by_owner_parses_a_real_http_response() {
    let base_url = spawn_fake_gateway().await;
    let client = ArweaveClient::new(format!("{base_url}/graphql"), base_url);

    let tx_id = client.latest_tx_by_owner("some-owner").await.unwrap();
    assert_eq!(tx_id.as_deref(), Some("real-tx-id"));
}

#[tokio::test]
async fn owner_of_tx_parses_a_real_http_response() {
    let base_url = spawn_fake_gateway().await;
    let client = ArweaveClient::new(format!("{base_url}/graphql"), base_url);

    let owner = client.owner_of_tx("real-tx-id").await.unwrap();
    assert_eq!(owner.as_deref(), Some("real-owner-address"));
}

#[tokio::test]
async fn fetch_tx_data_returns_the_raw_bytes() {
    let base_url = spawn_fake_gateway().await;
    let client = ArweaveClient::new(format!("{base_url}/graphql"), base_url);

    let bytes = client.fetch_tx_data("real-tx-id").await.unwrap();
    assert_eq!(bytes, b"encrypted-bytes".to_vec());
}
