use std::sync::Arc;

use async_trait::async_trait;
use rmcp::model::{CallToolRequestParams, Tool as McpToolSpec};
use rmcp::service::RunningService;
use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
use rmcp::{RoleClient, ServiceExt};
use serde_json::Value;
use tokio::process::Command;

use crate::tool::{Tool, ToolProvider, ToolSpec};

/// Connects to one external MCP server over stdio — spawn `command args...` as a child process
/// and speak MCP over its stdin/stdout. This is the standard transport for local MCP servers
/// (the same `command`/`args` shape every other MCP client uses, e.g. Claude Desktop's config),
/// which covers the concrete case that motivated this: connecting Warden to a locally-run
/// server exposed by another of the user's own projects (see PENDING.md P11/P13). A remote
/// transport (HTTP) can be added later behind the same `ToolProvider` trait if a real need for
/// one comes up — `rmcp` supports it, nothing here is stdio-specific by design.
///
/// One provider = one server. A server can advertise any number of tools, so `tools()` is where
/// that set is actually discovered (`tools/list`) — the provider itself doesn't know the tool
/// names until then.
pub struct McpToolProvider {
    server_name: String,
    session: Arc<RunningService<RoleClient, ()>>,
}

impl McpToolProvider {
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
