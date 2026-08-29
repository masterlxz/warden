//! Real (not mocked) end-to-end test: spawns the actual `warden-mcp-server` binary as a child
//! process and talks to it over real stdio via `McpToolProvider` — the same MCP *client* code
//! `warden-bootstrap` uses to connect to third-party servers, here pointed at Warden's own
//! server binary. Proves the vault tools are reachable through the real MCP protocol from an
//! external client, not just internally through the orchestrator.

use warden_core::tool::mcp::McpToolProvider;
use warden_core::tool::ToolProvider;

fn unique_temp_dir(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "warden-mcp-server-test-{name}-{}",
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ))
}

#[tokio::test]
async fn exposes_the_vault_tools_over_real_stdio_mcp() {
    // Explicit HOME override, not just GEMINI_API_KEY — otherwise `bootstrap()` inside the
    // spawned process would fall back to the *real* HOME (glibc's getpwuid fallback when HOME
    // is merely unset, not cleared) and could pick up a real config.toml / real key sitting on
    // whatever machine runs this test. Same bug class fixed in Sessão 32 for the other channels'
    // process-level smoke tests.
    let home = unique_temp_dir("home");
    std::fs::create_dir_all(&home).unwrap();
    let vault = unique_temp_dir("vault");

    let exe = env!("CARGO_BIN_EXE_warden-mcp-server");
    let args = vec!["--vault-path".to_string(), vault.to_str().unwrap().to_string()];
    let env = [
        ("HOME".to_string(), home.to_str().unwrap().to_string()),
        ("GEMINI_API_KEY".to_string(), "fake-key-for-mcp-server-test".to_string()),
    ];

    let provider =
        McpToolProvider::connect_stdio("warden", exe, &args, &env).await.expect("connect to warden-mcp-server over stdio");

    let tools = provider.tools().await.expect("list tools from warden-mcp-server");
    let names: Vec<String> = tools.iter().map(|t| t.spec().name).collect();
    assert!(names.iter().any(|n| n == "read_file"), "tools were: {names:?}");
    assert!(names.iter().any(|n| n == "write_file"), "tools were: {names:?}");

    let write_tool = tools.iter().find(|t| t.spec().name == "write_file").expect("write_file tool");
    write_tool
        .call(serde_json::json!({ "path": "note.md", "content": "hello from mcp" }))
        .await
        .expect("write_file call over real MCP");

    let read_tool = tools.iter().find(|t| t.spec().name == "read_file").expect("read_file tool");
    let result = read_tool.call(serde_json::json!({ "path": "note.md" })).await.expect("read_file call over real MCP");

    let result_str = result.to_string();
    assert!(result_str.contains("hello from mcp"), "unexpected result: {result_str}");

    std::fs::remove_dir_all(&home).ok();
    std::fs::remove_dir_all(&vault).ok();
}

#[tokio::test]
async fn fails_clearly_without_a_gemini_key() {
    let home = unique_temp_dir("home-no-key");
    std::fs::create_dir_all(&home).unwrap();
    let vault = unique_temp_dir("vault-no-key");

    let exe = env!("CARGO_BIN_EXE_warden-mcp-server");
    let args = vec!["--vault-path".to_string(), vault.to_str().unwrap().to_string()];
    let env = [("HOME".to_string(), home.to_str().unwrap().to_string())];

    // No GEMINI_API_KEY/OPENAI_API_KEY — bootstrap() inside the child should fail before it
    // ever completes the MCP handshake, so the connection attempt itself fails.
    let result = McpToolProvider::connect_stdio("warden", exe, &args, &env).await;
    assert!(result.is_err(), "expected connect_stdio to fail when the child process has no model API key");

    std::fs::remove_dir_all(&home).ok();
    std::fs::remove_dir_all(&vault).ok();
}
