//! Real (not mocked) end-to-end test of the OAuth-authenticated MCP HTTP transport
//! (`tool/mcp_oauth.rs`, PENDING.md P26). A single local `axum` server plays both the
//! protected-resource role (the MCP endpoint itself, mirroring `tests/mcp_http.rs`'s
//! `EchoServer`) and the authorization-server role (discovery, Dynamic Client Registration,
//! `/authorize`, `/token`) — the same self-referential shape `rmcp`'s own
//! `tests/test_client_credentials.rs` uses for the simpler client_credentials grant.
//!
//! The one necessarily-faked step is `/authorize` auto-approving instead of showing a real human
//! a consent screen — nothing in CI can click "Allow". Everything downstream of that is exercised
//! for real over real HTTP: RFC 9728/8414 discovery, Dynamic Client Registration, the PKCE code
//! exchange, token persistence to a real file on disk, and the bearer-protected MCP call itself.
//! `authorize_interactively`'s `open_browser` callback stands in for the user's click with a real
//! HTTP client, landing on the module's own local callback listener exactly as production does.

use std::collections::HashMap;

use axum::extract::{Query, Request};
use axum::http::{header, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Json, Router};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{ServerCapabilities, ServerInfo};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{schemars, tool, tool_handler, tool_router, ServerHandler};
use serde::Deserialize;
use serde_json::json;
use tokio_util::sync::CancellationToken;
use warden_core::tool::mcp_oauth::{authorize_interactively, connect_http_oauth};
use warden_core::tool::ToolProvider;

const CLIENT_ID: &str = "test-oauth-client";
const AUTH_CODE: &str = "test-authorization-code";
const ACCESS_TOKEN: &str = "test-access-token";
const REFRESH_TOKEN: &str = "test-refresh-token";

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

/// Stands in for a real server's auth check on the resource itself — proves the token this test
/// drives all the way through discovery/DCR/PKCE exchange is what actually unlocks the MCP call,
/// not just accepted and ignored.
async fn require_bearer_token(req: Request, next: Next) -> Response {
    let expected = format!("Bearer {ACCESS_TOKEN}");
    let authorized = req.headers().get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()) == Some(expected.as_str());
    if authorized {
        next.run(req).await
    } else {
        StatusCode::UNAUTHORIZED.into_response()
    }
}

/// RFC 9728 protected resource metadata — `resource` is the server's own root, not the deeper
/// `/mcp` path `connect_http_oauth`/`authorize_interactively` are given, on purpose: rmcp accepts
/// any origin-matching ancestor path as the resource identifier, and serving discovery at the
/// canonical root path (rather than nested under `/mcp/.well-known/...`) is the simplest layout
/// that still exercises the exact fallback rmcp's own discovery logic documents as always tried.
async fn resource_metadata_handler(req: Request) -> impl IntoResponse {
    let base_url = base_url_of(&req);
    Json(json!({
        "resource": base_url,
        "authorization_servers": [base_url],
    }))
}

/// RFC 8414 authorization server metadata — same host plays both resource and authorization
/// server, mirroring `rmcp`'s own `test_client_credentials.rs`. Advertises exactly what
/// `AuthorizationManager::validate_server_metadata`/`register_client` require: the `"code"`
/// response type and an `S256` PKCE method.
async fn as_metadata_handler(req: Request) -> impl IntoResponse {
    let base_url = base_url_of(&req);
    Json(json!({
        "issuer": base_url,
        "authorization_endpoint": format!("{base_url}/authorize"),
        "token_endpoint": format!("{base_url}/token"),
        "registration_endpoint": format!("{base_url}/register"),
        "response_types_supported": ["code"],
        "code_challenge_methods_supported": ["S256"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "scopes_supported": ["mcp"],
    }))
}

fn base_url_of(req: &Request) -> String {
    let host = req.headers().get(header::HOST).and_then(|v| v.to_str().ok()).expect("Host header");
    format!("http://{host}")
}

/// Dynamic Client Registration (RFC 7591) — echoes back whatever `redirect_uris` the client
/// registered with (that's the exact URI `authorize_interactively`'s local callback listener is
/// bound to) alongside a fixed client_id; no client_secret, a public client, same as any native
/// app doing PKCE.
async fn register_handler(Json(body): Json<serde_json::Value>) -> impl IntoResponse {
    let redirect_uris = body.get("redirect_uris").cloned().unwrap_or_else(|| json!([]));
    Json(json!({
        "client_id": CLIENT_ID,
        "client_secret": null,
        "client_name": body.get("client_name"),
        "redirect_uris": redirect_uris,
    }))
}

#[derive(Deserialize)]
struct AuthorizeParams {
    redirect_uri: String,
    state: String,
}

/// The one necessarily-faked step: a real authorization server would show a human a consent
/// screen here. This test server auto-approves and redirects straight back with a code — nothing
/// automated can click "Allow", so this is the honest way to exercise everything around it for
/// real instead of mocking the whole OAuth exchange.
async fn authorize_handler(Query(params): Query<AuthorizeParams>) -> impl IntoResponse {
    Redirect::to(&format!("{}?code={AUTH_CODE}&state={}", params.redirect_uri, params.state))
}

/// Token endpoint (RFC 6749 §4.1.3) — doesn't re-derive the PKCE `code_verifier` against a stored
/// `code_challenge` (this test server never captured one; the client always sends it, proving the
/// real client-side PKCE machinery ran, but a from-scratch OAuth server isn't what's under test
/// here). Rejects anything that isn't the expected grant/code so the test still fails loudly if
/// the exchange sends something unexpected.
async fn token_handler(Form(params): Form<HashMap<String, String>>) -> impl IntoResponse {
    let grant_type = params.get("grant_type").map(String::as_str).unwrap_or_default();
    let code = params.get("code").map(String::as_str).unwrap_or_default();
    if grant_type != "authorization_code" || code != AUTH_CODE {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "invalid_grant"}))).into_response();
    }
    Json(json!({
        "access_token": ACCESS_TOKEN,
        "token_type": "Bearer",
        "expires_in": 3600,
        "refresh_token": REFRESH_TOKEN,
        "scope": "mcp",
    }))
    .into_response()
}

/// Binds a server that's simultaneously the MCP endpoint (at `/mcp`, bearer-protected) and its
/// own OAuth authorization server (discovery + DCR + `/authorize` + `/token` at the root) to a
/// real local port. Returns the MCP endpoint URL plus a handle that shuts the server down when
/// cancelled.
async fn spawn_server() -> (String, CancellationToken, tokio::task::JoinHandle<()>) {
    let ct = CancellationToken::new();

    let service: StreamableHttpService<EchoServer, LocalSessionManager> = StreamableHttpService::new(
        || Ok(EchoServer::new()),
        Default::default(),
        StreamableHttpServerConfig::default().with_cancellation_token(ct.child_token()),
    );

    let mcp_route = Router::new().nest_service("/mcp", service).layer(middleware::from_fn(require_bearer_token));

    let router = Router::new()
        .route("/.well-known/oauth-protected-resource", get(resource_metadata_handler))
        .route("/.well-known/oauth-authorization-server", get(as_metadata_handler))
        .route("/register", post(register_handler))
        .route("/authorize", get(authorize_handler))
        .route("/token", post(token_handler))
        .merge(mcp_route);

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
async fn authorizes_persists_and_connects_to_a_real_oauth_mcp_server() {
    let (url, ct, handle) = spawn_server().await;
    let credential_store_path = std::env::temp_dir().join(format!(
        "warden-mcp-oauth-test-{}.json",
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    let _ = tokio::fs::remove_file(&credential_store_path).await;

    // Stands in for the user's browser click: a real HTTP GET against the authorization URL,
    // following the test server's auto-approval redirect straight to the module's own local
    // one-shot callback listener — proving that listener actually works, not just the OAuth
    // protocol exchange around it.
    let open_browser = {
        move |auth_url: &str| {
            let auth_url = auth_url.to_string();
            tokio::spawn(async move {
                let _ = reqwest::get(&auth_url).await;
            });
        }
    };

    authorize_interactively("oauth-echo-server", &url, &credential_store_path, open_browser)
        .await
        .expect("interactive OAuth authorization against a real (test) server");

    assert!(credential_store_path.is_file(), "authorize_interactively should have persisted a credential file");

    // Headless reconnect — the path every `bootstrap()` takes — must succeed purely from what
    // was just persisted, no browser involved.
    let provider = connect_http_oauth("oauth-echo-server", &url, &credential_store_path)
        .await
        .expect("headless reconnect using the persisted OAuth token");

    let tools = provider.tools().await.expect("list tools from the real OAuth-protected MCP server");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].spec().name, "echo");

    let result = tools[0].call(json!({ "text": "hello over oauth" })).await.expect("call the real MCP tool over OAuth");
    let result_str = result.to_string();
    assert!(result_str.contains("hello over oauth"), "unexpected result: {result_str}");

    let _ = tokio::fs::remove_file(&credential_store_path).await;
    ct.cancel();
    let _ = handle.await;
}

#[tokio::test]
async fn connect_http_oauth_fails_clearly_without_prior_authorization() {
    let (url, ct, handle) = spawn_server().await;
    let credential_store_path = std::env::temp_dir().join(format!(
        "warden-mcp-oauth-test-unauthorized-{}.json",
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    let _ = tokio::fs::remove_file(&credential_store_path).await;

    let err = match connect_http_oauth("oauth-echo-server", &url, &credential_store_path).await {
        Ok(_) => panic!("connecting without prior authorization must fail"),
        Err(err) => err,
    };
    assert!(err.to_string().contains("oauth-echo-server"), "error should name the server: {err}");

    ct.cancel();
    let _ = handle.await;
}
