//! P78 fatia 1 — the hub serving its web interface on the WebSocket port, over real sockets.

mod support;

use support::{http_get, spin_up_server, spin_up_server_with_web_ui, test_web_ui, MockProvider};
use warden_server::{ClientMessage, EmbeddedWebUi, ServerConnection, ServerMessage, StaticWebUi};

#[tokio::test]
async fn root_serves_index_html_uncached() {
    let addr = spin_up_server_with_web_ui(MockProvider::replying("unused"), test_web_ui(), None).await;
    let response = http_get(addr, "GET", "/").await;
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(response.contains("Content-Type: text/html; charset=utf-8\r\n"), "{response}");
    assert!(response.contains("Cache-Control: no-cache\r\n"), "{response}");
    assert!(response.ends_with("<title>Warden test UI</title>"), "{response}");
}

#[tokio::test]
async fn hashed_assets_get_their_content_type_and_long_caching() {
    let addr = spin_up_server_with_web_ui(MockProvider::replying("unused"), test_web_ui(), None).await;
    let response = http_get(addr, "GET", "/assets/app-1a2b.js").await;
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(response.contains("Content-Type: text/javascript; charset=utf-8\r\n"), "{response}");
    assert!(response.contains("immutable"), "{response}");
    assert!(response.ends_with("console.log('warden')"), "{response}");
}

#[tokio::test]
async fn client_side_routes_fall_back_to_index_but_missing_files_are_404() {
    let addr = spin_up_server_with_web_ui(MockProvider::replying("unused"), test_web_ui(), None).await;
    let route = http_get(addr, "GET", "/skills").await;
    assert!(route.starts_with("HTTP/1.1 200 OK\r\n") && route.contains("Warden test UI"), "{route}");

    let missing = http_get(addr, "GET", "/assets/gone.js").await;
    assert!(missing.starts_with("HTTP/1.1 404 Not Found\r\n"), "{missing}");
}

#[tokio::test]
async fn traversal_is_refused() {
    let addr = spin_up_server_with_web_ui(MockProvider::replying("unused"), test_web_ui(), None).await;
    let response = http_get(addr, "GET", "/../Cargo.toml").await;
    assert!(response.starts_with("HTTP/1.1 404 Not Found\r\n"), "{response}");
}

#[tokio::test]
async fn only_get_and_head_are_allowed() {
    let addr = spin_up_server_with_web_ui(MockProvider::replying("unused"), test_web_ui(), None).await;
    let post = http_get(addr, "POST", "/").await;
    assert!(post.starts_with("HTTP/1.1 405 Method Not Allowed\r\n"), "{post}");

    let head = http_get(addr, "HEAD", "/").await;
    assert!(head.starts_with("HTTP/1.1 200 OK\r\n"), "{head}");
    assert!(head.ends_with("\r\n\r\n"), "HEAD must not carry a body: {head}");
}

#[tokio::test]
async fn an_empty_build_explains_itself_with_503() {
    let addr = spin_up_server_with_web_ui(MockProvider::replying("unused"), std::sync::Arc::new(StaticWebUi::default()), None).await;
    let response = http_get(addr, "GET", "/").await;
    assert!(response.starts_with("HTTP/1.1 503 Service Unavailable\r\n"), "{response}");
    assert!(response.contains("npm run build"), "{response}");
}

#[tokio::test]
async fn the_websocket_protocol_still_works_on_the_same_port() {
    let addr = spin_up_server_with_web_ui(MockProvider::replying("hi from the hub"), test_web_ui(), None).await;
    assert!(http_get(addr, "GET", "/").await.starts_with("HTTP/1.1 200 OK"));

    let mut conn = ServerConnection::connect(&format!("ws://{addr}"), "browser-1", "Browser", "test-key").await.unwrap();
    conn.send(&ClientMessage::chat("hello")).await.unwrap();
    match conn.recv().await.unwrap() {
        Some(ServerMessage::ChatResponse { content, .. }) => assert_eq!(content, "hi from the hub"),
        other => panic!("expected ChatResponse, got {other:?}"),
    }
}

#[tokio::test]
async fn a_hub_without_a_web_ui_serves_no_pages() {
    let addr = spin_up_server(MockProvider::replying("unused")).await;
    let response = http_get(addr, "GET", "/").await;
    assert!(!response.contains("200 OK"), "{response}");
}

#[test]
fn the_embedded_ui_compiles_even_without_a_web_build() {
    // Whether or not `web/dist` exists in this checkout, looking a file up must not panic.
    let _ = warden_server::WebAssets::get(&EmbeddedWebUi, "index.html");
}
