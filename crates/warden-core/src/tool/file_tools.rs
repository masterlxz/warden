use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::memory::Vault;
use crate::tool::{Tool, ToolSpec};

/// Reads a markdown file from the vault. First concrete `Tool` implementation (Phase 1.6).
pub struct ReadFileTool {
    vault: Arc<Vault>,
}

impl ReadFileTool {
    pub fn new(vault: Arc<Vault>) -> Self {
        Self { vault }
    }
}

#[async_trait]
impl Tool for ReadFileTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "read_file".to_string(),
            description: "Read a markdown file from the memory vault by its path relative to the vault root."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path relative to the vault root, e.g. 'notes/todo.md'"
                    }
                },
                "required": ["path"]
            }),
        }
    }

    async fn call(&self, args: Value) -> anyhow::Result<Value> {
        let path = args
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("missing required 'path' argument"))?;
        let content = self.vault.read(path)?;
        Ok(json!({ "content": content }))
    }
}

/// Writes (creates or overwrites) a markdown file in the vault.
pub struct WriteFileTool {
    vault: Arc<Vault>,
}

impl WriteFileTool {
    pub fn new(vault: Arc<Vault>) -> Self {
        Self { vault }
    }
}

#[async_trait]
impl Tool for WriteFileTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "write_file".to_string(),
            description:
                "Write (create or overwrite) a markdown file in the memory vault at the given path relative to the vault root."
                    .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path relative to the vault root, e.g. 'notes/todo.md'"
                    },
                    "content": {
                        "type": "string",
                        "description": "Full file content to write"
                    }
                },
                "required": ["path", "content"]
            }),
        }
    }

    async fn call(&self, args: Value) -> anyhow::Result<Value> {
        let path = args
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("missing required 'path' argument"))?;
        let content = args
            .get("content")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("missing required 'content' argument"))?;
        self.vault.write(path, content)?;
        Ok(json!({ "status": "ok" }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_vault() -> Arc<Vault> {
        Arc::new(Vault::new(std::env::temp_dir().join(format!(
            "warden-file-tools-test-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ))))
    }

    #[tokio::test]
    async fn write_then_read_via_tools() {
        let vault = temp_vault();
        let writer = WriteFileTool::new(vault.clone());
        let reader = ReadFileTool::new(vault.clone());

        writer.call(json!({ "path": "notes/todo.md", "content": "buy milk" })).await.unwrap();
        let result = reader.call(json!({ "path": "notes/todo.md" })).await.unwrap();

        assert_eq!(result, json!({ "content": "buy milk" }));
    }

    #[tokio::test]
    async fn read_file_requires_path_argument() {
        let vault = temp_vault();
        let reader = ReadFileTool::new(vault);

        let err = reader.call(json!({})).await.unwrap_err();
        assert!(err.to_string().contains("path"));
    }
}
