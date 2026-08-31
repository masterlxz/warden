//! Real (not mocked) end-to-end test of `McpToolProvider::connect_http`: a tiny MCP server,
//! speaking the actual streamable-HTTP wire protocol via `rmcp`'s own server machinery, is bound
//! to a real local TCP port (`axum`/`tokio` — same shape as any real hosted MCP server, e.g. the
//! one PENDING.md P25 was written against). Also proves `connect_http`'s custom headers actually
//! reach the server (not just accepted client-side and silently dropped): the server requires a
//! specific header via an `axum` auth middleware, so a connection is only expected to succeed
//! when `McpToolProvider::connect_http` is given that header.

use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{ServerCapabilities, ServerInfo};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ServerHandler, schemars, tool, tool_handler, tool_router};
use serde_json::json;
use tokio_util::sync::CancellationToken;
use warden_core::tool::mcp::McpToolProvider;
use warden_core::tool::ToolProvider;

const REQUIRED_HEADER: &str = "x-warden-test-token";
const REQUIRED_VALUE: &str = "let-me-in";

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct EchoRequest {
    text: String,
}

#[derive(Clone)]
struct EchoServer {
    tool_router: ToolRouter<Self>,
}

impl EchoServer {
    fn new() -> Self {
        Self { tool_router: Self::tool_router() }
    }
}

#[tool_router]
impl EchoServer {
    #[tool(description = "Echoes the given text back")]
    fn echo(&self, Parameters(EchoRequest { text }): Parameters<EchoRequest>) -> String {
        text
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for EchoServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
    }
}

/// Rejects any request missing the expected header — stands in for a real remote server's auth
/// check (e.g. Slack's hosted MCP server, see PENDING.md P25), so the test can prove
/// `connect_http`'s `headers` argument is what makes the difference between success and failure.
async fn require_test_header(req: Request, next: Next) -> Response {
    let authorized = req.headers().get(REQUIRED_HEADER).and_then(|v| v.to_str().ok()) == Some(REQUIRED_VALUE);
    if authorized {
        next.run(req).await
    } else {
        StatusCode::UNAUTHORIZED.into_response()
    }
}

/// Binds the echo server to a real local port and returns its base URL plus a handle that shuts
/// the server down when dropped (cancelling `ct` and awaiting the serve task).
async fn spawn_server() -> (String, CancellationToken, tokio::task::JoinHandle<()>) {
    let ct = CancellationToken::new();

    let service: StreamableHttpService<EchoServer, LocalSessionManager> = StreamableHttpService::new(
        || Ok(EchoServer::new()),
        Default::default(),
        StreamableHttpServerConfig::default().with_cancellation_token(ct.child_token()),
    );

    let router = axum::Router::new().nest_service("/mcp", service).layer(middleware::from_fn(require_test_header));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind local test port");
    let addr = listener.local_addr().expect("local addr");

    let handle = tokio::spawn({
        let ct = ct.clone();
        async move {
            let _ = axum::serve(listener, router).with_graceful_shutdown(async move { ct.cancelled_owned().await }).await;
        }
    });

    (format!("http://{addr}/mcp"), ct, handle)
}

#[tokio::test]
async fn connects_lists_and_calls_tools_on_a_real_http_mcp_server() {
    let (url, ct, handle) = spawn_server().await;

    let headers = vec![(REQUIRED_HEADER.to_string(), REQUIRED_VALUE.to_string())];
    let provider = McpToolProvider::connect_http("echo-http-server", &url, &headers).await.expect("connect to real HTTP MCP server");

    let tools = provider.tools().await.expect("list tools from real server");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].spec().name, "echo");

    let result = tools[0].call(json!({ "text": "hello over http" })).await.expect("call real MCP tool over http");
    let result_str = result.to_string();
    assert!(result_str.contains("hello over http"), "unexpected result: {result_str}");

    ct.cancel();
    let _ = handle.await;
}

#[tokio::test]
async fn connect_http_fails_clearly_when_the_required_header_is_missing() {
    let (url, ct, handle) = spawn_server().await;

    let result = McpToolProvider::connect_http("echo-http-server", &url, &[]).await;
    let err = match result {
        Ok(_) => panic!("connection without the required header must fail"),
        Err(err) => err,
    };
    assert!(err.to_string().contains("echo-http-server"), "error should name the server: {err}");

    ct.cancel();
    let _ = handle.await;
}
