//! Warden as an MCP *server* (the other direction from `warden_core::tool::mcp`, which makes
//! Warden an MCP *client*) — exposes every tool this orchestrator has registered (vault access,
//! shell if the user enabled it, whatever external MCP servers `bootstrap()` connected to) over
//! stdio, so any third-party MCP client (Claude Desktop, another agent, ...) can point its own
//! `mcpServers` config at this binary and use Warden's capabilities. Meant to be launched
//! on-demand by that client (same `command`/`args` shape MCP configs already use everywhere in
//! this project) — no persistent network listener, consistent with "servidor é opcional"
//! (see `ARCHITECTURE.md`).

use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ListToolsResult, PaginatedRequestParams, ServerCapabilities,
    ServerInfo, Tool as McpToolSpec,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler, ServiceExt};
use warden_bootstrap::{bootstrap, Overrides};
use warden_core::tool::Tool;

#[derive(Parser, Debug)]
#[command(name = "warden-mcp-server", version, about = "Warden — expose its own tools/vault as an MCP server for any MCP client")]
struct Cli {
    /// Path to the markdown vault (memory). Overrides the config file; defaults to
    /// ~/Warden/vault — this process is meant to be launched by another app's MCP client config,
    /// same as the desktop app and the other channels, so it has no predictable cwd either.
    #[arg(long)]
    vault_path: Option<String>,

    /// Path to the config file (TOML). Defaults to the OS config dir.
    #[arg(long)]
    config: Option<String>,
}

/// Same fallback every other headless entry point in this workspace uses (`warden-telegram`,
/// `warden-whatsapp`, the desktop app) — see their `default_vault_path`/`desktop_default_vault_path`.
fn default_vault_path() -> PathBuf {
    dirs::home_dir().unwrap_or_default().join("Warden").join("vault")
}

/// A thin bridge: no capability is added or removed here, this only changes *who* can call
/// Warden's tools (any MCP client, not just Warden's own orchestrator).
struct WardenMcpServer {
    tools: Vec<Arc<dyn Tool>>,
}

impl ServerHandler for WardenMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
    }

    async fn list_tools(&self, _request: Option<PaginatedRequestParams>, _context: RequestContext<RoleServer>) -> Result<ListToolsResult, McpError> {
        let tools = self
            .tools
            .iter()
            .map(|tool| {
                let spec = tool.spec();
                // Every `Tool::spec().parameters` in this codebase is a JSON Schema object
                // (`{"type": "object", ...}`) — `unwrap_or_default` only matters if a future
                // tool breaks that invariant, in which case an empty schema is a safer fallback
                // than failing the whole tool list.
                let schema = spec.parameters.as_object().cloned().unwrap_or_default();
                McpToolSpec::new(spec.name, spec.description, Arc::new(schema))
            })
            .collect();
        Ok(ListToolsResult::with_all_items(tools))
    }

    async fn call_tool(&self, request: CallToolRequestParams, _context: RequestContext<RoleServer>) -> Result<CallToolResponse, McpError> {
        let name = request.name.as_ref().to_string();
        let tool = self
            .tools
            .iter()
            .find(|t| t.spec().name == name)
            .ok_or_else(|| McpError::invalid_params(format!("unknown tool: {name}"), None))?;

        let args = request.arguments.map(serde_json::Value::Object).unwrap_or(serde_json::Value::Null);
        let result = tool.call(args).await.map_err(|err| McpError::internal_error(err.to_string(), None))?;

        // Tool results already get serialized to a plain string when fed back to a model
        // (`Message::tool_result`) — same treatment here, since MCP tool results are text
        // content blocks, not structured JSON.
        let text = serde_json::to_string(&result).unwrap_or_else(|_| result.to_string());
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]).into())
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let orchestrator = bootstrap(cli.config.as_deref(), Overrides { vault_path: cli.vault_path, ..Default::default() }, default_vault_path()).await?;
    let tools = orchestrator.tools().to_vec();

    // stderr, not stdout — stdout is the MCP JSON-RPC transport itself (same separation of
    // channels already used by the WhatsApp sidecar and every other stdio-transport process here).
    eprintln!("warden-mcp-server: exposing {} tool(s) over stdio", tools.len());

    let server = WardenMcpServer { tools }.serve(rmcp::transport::stdio()).await?;
    server.waiting().await?;
    Ok(())
}
