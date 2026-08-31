use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use http::{HeaderName, HeaderValue};
use rmcp::model::{CallToolRequestParams, Tool as McpToolSpec};
use rmcp::service::RunningService;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::{ConfigureCommandExt, StreamableHttpClientTransport, TokioChildProcess};
use rmcp::{RoleClient, ServiceExt};
use serde_json::Value;
use tokio::process::Command;

use crate::tool::{Tool, ToolProvider, ToolSpec};

/// Connects to one external MCP server, over either transport `rmcp` supports client-side:
/// stdio (spawn `command args...` as a child process, speak MCP over its stdin/stdout — the
/// standard for local MCP servers, the same `command`/`args` shape every other MCP client uses,
/// e.g. Claude Desktop's config) or streamable HTTP (`connect_http`, added for the concrete case
/// that motivated it: the official hosted Slack MCP server is remote-only, see PENDING.md P25).
/// Auth for the HTTP transport is a static set of request headers (typically a bearer token the
/// user pastes in) rather than a full OAuth dance — good enough for a server that hands out a
/// long-lived token, not a general OAuth client. See ARCHITECTURE.md for the trade-off.
///
/// One provider = one server. A server can advertise any number of tools, so `tools()` is where
/// that set is actually discovered (`tools/list`) — the provider itself doesn't know the tool
/// names until then.
pub struct McpToolProvider {
    server_name: String,
    session: Arc<RunningService<RoleClient, ()>>,
}

impl McpToolProvider {
    /// Wraps an already-running MCP session (any transport) as a `McpToolProvider`. Used by
    /// `connect_stdio`/`connect_http` below and by `mcp_oauth::connect_http_oauth` (a third
    /// transport-setup path — OAuth-authenticated streamable HTTP — that lives in its own module
    /// since it pulls in the `rmcp` `auth` feature, but produces the exact same provider type;
    /// `tools()`/`call()` don't care how the session was authenticated).
    pub(crate) fn from_session(server_name: impl Into<String>, session: RunningService<RoleClient, ()>) -> Self {
        Self { server_name: server_name.into(), session: Arc::new(session) }
    }

    /// Spawns the server process and performs the MCP `initialize` handshake. Fails fast if the
    /// process can't be spawned or never completes the handshake — the caller (bootstrap) treats
    /// a failed connection as "this one server is unavailable", not a fatal startup error, same
    /// as the existing Tavily/shell graceful-degradation pattern.
    ///
    /// `env` is scoped to the child process only (`Command::envs`, not the current process's
    /// environment) — needed for real servers that take secrets/config via env vars (the same
    /// shape as e.g. Claude Desktop's `mcpServers.<name>.env`), and also what keeps this safe to
    /// call from concurrent tests: mutating the whole process's env here would race with
    /// anything else running in the same test binary.
    pub async fn connect_stdio(
        server_name: impl Into<String>,
        command: &str,
        args: &[String],
        env: &[(String, String)],
    ) -> anyhow::Result<Self> {
        let server_name = server_name.into();
        let transport = TokioChildProcess::new(Command::new(command).configure(|cmd| {
            cmd.args(args);
            cmd.envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())));
        }))
        .map_err(|err| anyhow::anyhow!("failed to spawn MCP server '{server_name}' ({command}): {err}"))?;

        let session = ()
            .serve(transport)
            .await
            .map_err(|err| anyhow::anyhow!("failed to initialize MCP server '{server_name}': {err}"))?;

        Ok(Self { server_name, session: Arc::new(session) })
    }

    /// Connects to a remote MCP server over streamable HTTP (the transport the MCP spec calls
    /// for when a server isn't a locally-spawned process — see e.g. Slack's officially hosted
    /// server). `headers` is sent on every request (e.g. `[("Authorization", "Bearer <token>")]`
    /// for a server that authenticates that way) — same graceful-degradation treatment as
    /// `connect_stdio` at the call site: a failed connection disables just this one server.
    pub async fn connect_http(server_name: impl Into<String>, url: &str, headers: &[(String, String)]) -> anyhow::Result<Self> {
        let server_name = server_name.into();

        let mut custom_headers = HashMap::new();
        for (name, value) in headers {
            let header_name = HeaderName::try_from(name.as_str())
                .map_err(|err| anyhow::anyhow!("invalid header name '{name}' for MCP server '{server_name}': {err}"))?;
            let header_value = HeaderValue::try_from(value.as_str())
                .map_err(|err| anyhow::anyhow!("invalid header value for '{name}' on MCP server '{server_name}': {err}"))?;
            custom_headers.insert(header_name, header_value);
        }

        // `from_config` resolves to rmcp's own bundled reqwest client (gated behind the
        // `transport-streamable-http-client-reqwest` feature) — deliberately not our own
        // `reqwest` dependency (a different major version), so nothing here names that type.
        let config = StreamableHttpClientTransportConfig::with_uri(url).custom_headers(custom_headers);
        let transport = StreamableHttpClientTransport::from_config(config);

        let session = ()
            .serve(transport)
            .await
            .map_err(|err| anyhow::anyhow!("failed to initialize MCP server '{server_name}' at {url}: {err}"))?;

        Ok(Self { server_name, session: Arc::new(session) })
    }
}

#[async_trait]
impl ToolProvider for McpToolProvider {
    async fn tools(&self) -> anyhow::Result<Vec<Arc<dyn Tool>>> {
        let specs = self
            .session
            .list_all_tools()
            .await
            .map_err(|err| anyhow::anyhow!("failed to list tools from MCP server '{}': {err}", self.server_name))?;

        Ok(specs
            .into_iter()
            .map(|spec| Arc::new(McpTool { server_name: self.server_name.clone(), session: self.session.clone(), spec }) as Arc<dyn Tool>)
            .collect())
    }
}

/// Adapter: makes one tool advertised by an MCP server look like a plain `Tool` to the
/// orchestrator, forwarding `call()` as a `tools/call` request over the shared session.
struct McpTool {
    server_name: String,
    session: Arc<RunningService<RoleClient, ()>>,
    spec: McpToolSpec,
}

#[async_trait]
impl Tool for McpTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.spec.name.to_string(),
            description: self.spec.description.clone().map(|d| d.to_string()).unwrap_or_default(),
            parameters: Value::Object((*self.spec.input_schema).clone()),
        }
    }

    async fn call(&self, args: Value) -> anyhow::Result<Value> {
        let arguments = match args {
            Value::Object(map) => Some(map),
            Value::Null => None,
            other => anyhow::bail!("expected a JSON object of arguments for MCP tool '{}', got: {other}", self.spec.name),
        };

        let mut params = CallToolRequestParams::new(self.spec.name.clone());
        if let Some(arguments) = arguments {
            params = params.with_arguments(arguments);
        }

        let result = self
            .session
            .call_tool(params)
            .await
            .map_err(|err| {
                anyhow::anyhow!("MCP tool '{}' (server '{}') call failed: {err}", self.spec.name, self.server_name)
            })?;

        Ok(serde_json::to_value(result)?)
    }
}
