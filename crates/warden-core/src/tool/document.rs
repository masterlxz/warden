use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::tool::{Tool, ToolSpec};

/// `generate_document` (P64) grows one format at a time, cheapest first. TXT/MD/CSV are all
/// plain-text writes — the model already produces well-formed CSV as a string, no parsing/
/// validation needed here. PDF/XLSX are planned but each needs its own new dependency decision,
/// out of scope for this slice.
const SUPPORTED_EXTENSIONS: [&str; 3] = ["txt", "md", "csv"];

/// Writes a standalone deliverable file — for the user to open outside the conversation, not a
/// note the orchestrator injects back into context — into a dedicated directory separate from the
/// memory vault (P64: mixing one-off outputs into the vault would inflate `warden-sync`/git-sync
/// pushes and confuse `search`/`search_semantic` with binary-flavored content).
pub struct GenerateDocumentTool {
    root: PathBuf,
}

impl GenerateDocumentTool {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }
}

#[async_trait]
impl Tool for GenerateDocumentTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "generate_document".to_string(),
            description: "Create a standalone document file for the user to open/download — not for the memory vault. \
                Supports .txt, .md and .csv filenames; PDF/XLSX are not implemented yet."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "filename": {
                        "type": "string",
                        "description": "File name with extension, e.g. 'relatorio.md'. Only .txt, .md and .csv are supported today."
                    },
                    "content": {
                        "type": "string",
                        "description": "Full file content to write. For .csv, this must already be well-formed CSV text (header row + comma-separated values, quoted as needed)."
                    }
                },
                "required": ["filename", "content"]
            }),
        }
    }

    async fn call(&self, args: Value) -> anyhow::Result<Value> {
        let filename = args.get("filename").and_then(Value::as_str).ok_or_else(|| anyhow::anyhow!("missing required 'filename' argument"))?;
        let content = args.get("content").and_then(Value::as_str).ok_or_else(|| anyhow::anyhow!("missing required 'content' argument"))?;

        let extension = std::path::Path::new(filename).extension().and_then(|e| e.to_str()).map(str::to_lowercase);
        if !extension.as_deref().is_some_and(|e| SUPPORTED_EXTENSIONS.contains(&e)) {
            anyhow::bail!("unsupported file extension for 'generate_document' — only .txt, .md and .csv are supported today (PDF/XLSX are planned but not implemented yet)");
        }

        std::fs::create_dir_all(&self.root)?;
        let path = self.root.join(filename);
        std::fs::write(&path, content)?;
        Ok(json!({ "status": "ok", "path": path.display().to_string() }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root() -> PathBuf {
        std::env::temp_dir().join(format!(
            "warden-generate-document-test-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ))
    }

    #[tokio::test]
    async fn writes_a_markdown_file() {
        let root = temp_root();
        let tool = GenerateDocumentTool::new(root.clone());

        let result = tool.call(json!({ "filename": "relatorio.md", "content": "# Olá" })).await.unwrap();

        let path = root.join("relatorio.md");
        assert_eq!(result, json!({ "status": "ok", "path": path.display().to_string() }));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# Olá");
    }

    #[tokio::test]
    async fn writes_a_txt_file() {
        let root = temp_root();
        let tool = GenerateDocumentTool::new(root.clone());

        tool.call(json!({ "filename": "notas.TXT", "content": "conteúdo" })).await.unwrap();

        assert_eq!(std::fs::read_to_string(root.join("notas.TXT")).unwrap(), "conteúdo");
    }

    #[tokio::test]
    async fn writes_a_csv_file() {
        let root = temp_root();
        let tool = GenerateDocumentTool::new(root.clone());

        let content = "nome,idade\nAna,30\nBia,25";
        tool.call(json!({ "filename": "pessoas.csv", "content": content })).await.unwrap();

        assert_eq!(std::fs::read_to_string(root.join("pessoas.csv")).unwrap(), content);
    }

    #[tokio::test]
    async fn rejects_unsupported_extension() {
        let tool = GenerateDocumentTool::new(temp_root());

        let err = tool.call(json!({ "filename": "relatorio.pdf", "content": "x" })).await.unwrap_err();

        assert!(err.to_string().contains("only .txt, .md and .csv"));
    }

    #[tokio::test]
    async fn rejects_missing_extension() {
        let tool = GenerateDocumentTool::new(temp_root());

        let err = tool.call(json!({ "filename": "relatorio", "content": "x" })).await.unwrap_err();

        assert!(err.to_string().contains("only .txt, .md and .csv"));
    }

    #[tokio::test]
    async fn requires_filename_argument() {
        let tool = GenerateDocumentTool::new(temp_root());

        let err = tool.call(json!({ "content": "x" })).await.unwrap_err();

        assert!(err.to_string().contains("filename"));
    }

    #[tokio::test]
    async fn requires_content_argument() {
        let tool = GenerateDocumentTool::new(temp_root());

        let err = tool.call(json!({ "filename": "a.md" })).await.unwrap_err();

        assert!(err.to_string().contains("content"));
    }
}
