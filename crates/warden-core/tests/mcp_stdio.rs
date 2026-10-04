//! Real (not mocked) end-to-end test of `McpToolProvider`: a tiny MCP server, speaking the
//! actual MCP stdio wire protocol via `rmcp`'s own server machinery, is spawned as a real child
//! process — same shape as `sh -c '<cmd>'` for a real npx/node-based server, just without the
//! network/npm dependency that would make this test flaky in CI. Same self-re-exec trick rmcp's
//! own test suite uses (see `test_stdio_response_concurrency.rs` upstream): the test binary
//! re-invokes itself with an env var set, and the child instance runs only the helper test,
//! which becomes the server side of the stdio pipe.

use std::sync::Arc;

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool as McpToolSpec,
};
use rmcp::service::{RequestContext, RunningService};
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler, ServiceExt};
use serde_json::json;
use warden_core::tool::mcp::McpToolProvider;
use warden_core::tool::ToolProvider;

const HELPER_ENV: &str = "WARDEN_MCP_STDIO_TEST_HELPER";

struct EchoServer;

impl ServerHandler for EchoServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let schema = json!({
            "type": "object",
            "properties": { "text": { "type": "string" } },
            "required": ["text"]
        })
        .as_object()
        .unwrap()
        .clone();

        Ok(ListToolsResult::with_all_items(vec![McpToolSpec::new(
            "echo",
            "Echoes the given text back",
            Arc::new(schema),
        )]))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        assert_eq!(request.name.as_ref(), "echo");
        let text = request
            .arguments
            .as_ref()
            .and_then(|args| args.get("text"))
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]).into())
    }
}

/// Not a real test on its own — a no-op unless `HELPER_ENV` is set, in which case it blocks
/// serving `EchoServer` over real stdin/stdout until the parent process closes the pipe. The
/// parent test below re-execs the test binary targeting just this test to get a genuine
/// out-of-process MCP server.
#[tokio::test]
async fn mcp_stdio_test_helper() -> anyhow::Result<()> {
    if std::env::var(HELPER_ENV).as_deref() != Ok("1") {
        return Ok(());
    }
    let server: RunningService<RoleServer, EchoServer> = EchoServer.serve(rmcp::transport::stdio()).await?;
    server.waiting().await?;
    std::process::exit(0);
}

// Note on stderr noise: without the `exit(0)` in the helper above, this test printed `error: io
// error when listing tests: ... Broken pipe` to stderr on every run (the child's stderr is
// inherited, since `TokioChildProcess` only pipes stdin/stdout). Cause: once the parent test is
// done and the transport closes, the child's `waiting()` returns and its own libtest harness
// prints the one-test summary to its stdout — the pipe back to us, already closed — and that
// write fails (50 runs out of 50, so not a race). The JSON-RPC exchange was long finished and
// the result never changed. The helper now exits right after `waiting()`, before libtest gets
// to write anything (0 runs out of 50 print the error).

#[tokio::test]
async fn connects_lists_and_calls_tools_on_a_real_stdio_mcp_server() {
    // `mcp_stdio_test_helper` runs as an ordinary test too (in this same process, concurrently
    // with this one, under the default multi-threaded test runner) — it's a no-op there because
    // HELPER_ENV is unset for it. Scoping the var to just this spawned child (rather than
    // mutating the whole process's env) is what keeps that safe: a shared process-wide env var
    // would race with that other test actually running as itself in parallel.
    let exe = std::env::current_exe().expect("current test exe");
    let args: Vec<String> =
        vec!["--exact".to_string(), "mcp_stdio_test_helper".to_string(), "--quiet".to_string(), "--test-threads".to_string(), "1".to_string()];
    let env = [(HELPER_ENV.to_string(), "1".to_string())];

    let provider =
        McpToolProvider::connect_stdio("echo-server", exe.to_str().expect("exe path is valid utf-8"), &args, &env)
            .await
            .expect("connect to real stdio MCP server");

    let tools = provider.tools().await.expect("list tools from real server");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].spec().name, "echo");
    assert_eq!(tools[0].spec().description, "Echoes the given text back");

    let result = tools[0].call(json!({ "text": "hello from warden" })).await.expect("call real MCP tool");

    let result_str = result.to_string();
    assert!(result_str.contains("hello from warden"), "unexpected result: {result_str}");
}
