use std::path::{Path, PathBuf};

use async_trait::async_trait;
use lopdf::content::{Content, Operation};
use lopdf::{dictionary, Document, Object, Stream};
use serde_json::{json, Value};

use crate::tool::{Tool, ToolSpec};

/// `generate_document` (P64) grows one format at a time, cheapest first. TXT/MD/CSV are all
/// plain-text writes — the model already produces well-formed CSV as a string, no parsing/
/// validation needed here. PDF renders the content as a simple paginated document (see
/// `write_pdf`). XLSX-with-formulas is planned but needs its own new dependency decision, out of
/// scope for this slice.
const SUPPORTED_EXTENSIONS: [&str; 4] = ["txt", "md", "csv", "pdf"];

// PDF layout (v1): A4, plain wrapped/paginated text, no Markdown-aware styling — the same
// "cheapest that isn't raw text" bar already accepted for CSV/TXT/MD (P64's "capricho" bar is for
// XLSX, not this slice).
const PDF_PAGE_WIDTH: f32 = 595.0;
const PDF_PAGE_HEIGHT: f32 = 842.0;
const PDF_MARGIN: f32 = 50.0;
const PDF_FONT_SIZE: f32 = 11.0;
const PDF_LEADING: f32 = 14.0;
// Helvetica has no fixed glyph width; 0.5 * font size is a conservative average-width estimate
// (real average is a bit narrower, so this wraps a little early rather than overflowing the page)
// used only to size-wrap lines without pulling in AFM glyph metrics for a v1 slice.
const PDF_AVG_CHAR_WIDTH: f32 = PDF_FONT_SIZE * 0.5;

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
                Supports .txt, .md, .csv and .pdf filenames; XLSX-with-formulas is not implemented yet."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "filename": {
                        "type": "string",
                        "description": "File name with extension, e.g. 'relatorio.md'. Only .txt, .md, .csv and .pdf are supported today."
                    },
                    "content": {
                        "type": "string",
                        "description": "Full file content to write. For .csv, this must already be well-formed CSV text (header row + comma-separated values, quoted as needed). For .pdf, plain text — it is laid out as a simple wrapped/paginated document, not rendered as Markdown/HTML."
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
            anyhow::bail!("unsupported file extension for 'generate_document' — only .txt, .md, .csv and .pdf are supported today (XLSX-with-formulas is planned but not implemented yet)");
        }

        std::fs::create_dir_all(&self.root)?;
        let path = self.root.join(filename);
        if extension.as_deref() == Some("pdf") {
            write_pdf(&path, content)?;
        } else {
            std::fs::write(&path, content)?;
        }
        Ok(json!({ "status": "ok", "path": path.display().to_string() }))
    }
}

/// Renders `content` as a simple paginated PDF (A4, Helvetica base-14 font, no embedding — see
/// module-level layout constants) and writes it to `path`.
fn write_pdf(path: &Path, content: &str) -> anyhow::Result<()> {
    let wrap_columns = ((PDF_PAGE_WIDTH - 2.0 * PDF_MARGIN) / PDF_AVG_CHAR_WIDTH) as usize;
    let lines = wrap_lines(content, wrap_columns.max(1));
    let lines_per_page = (((PDF_PAGE_HEIGHT - 2.0 * PDF_MARGIN) / PDF_LEADING) as usize).max(1);
    let pages: Vec<&[String]> = if lines.is_empty() { vec![&[]] } else { lines.chunks(lines_per_page).collect() };

    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();

    let font_id = doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica",
        "Encoding" => "WinAnsiEncoding",
    });
    let resources_id = doc.add_object(dictionary! {
        "Font" => dictionary! { "F1" => font_id },
    });

    let mut page_ids = Vec::with_capacity(pages.len());
    for page_lines in &pages {
        let mut operations = vec![Operation::new("BT", vec![]), Operation::new("Tf", vec!["F1".into(), PDF_FONT_SIZE.into()])];
        let start_y = PDF_PAGE_HEIGHT - PDF_MARGIN - PDF_FONT_SIZE;
        operations.push(Operation::new("Td", vec![PDF_MARGIN.into(), start_y.into()]));
        for (i, line) in page_lines.iter().enumerate() {
            if i > 0 {
                operations.push(Operation::new("Td", vec![0.into(), (-PDF_LEADING).into()]));
            }
            operations.push(Operation::new("Tj", vec![Object::string_literal(encode_winansi_lossy(line))]));
        }
        operations.push(Operation::new("ET", vec![]));

        let content_id = doc.add_object(Stream::new(dictionary! {}, Content { operations }.encode()?));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Contents" => content_id,
        });
        page_ids.push(page_id.into());
    }

    let pages_dict = dictionary! {
        "Type" => "Pages",
        "Count" => page_ids.len() as i64,
        "Kids" => page_ids,
        "Resources" => resources_id,
        "MediaBox" => vec![0.into(), 0.into(), PDF_PAGE_WIDTH.into(), PDF_PAGE_HEIGHT.into()],
    };
    doc.objects.insert(pages_id, Object::Dictionary(pages_dict));

    let catalog_id = doc.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    doc.trailer.set("Root", catalog_id);
    doc.compress();
    doc.save(path)?;
    Ok(())
}

/// Splits `content` into lines that each fit within `width` columns — explicit newlines are
/// preserved as paragraph breaks, each resulting line is then greedily word-wrapped so it never
/// overflows the page.
fn wrap_lines(content: &str, width: usize) -> Vec<String> {
    content.lines().flat_map(|line| wrap_one_line(line, width)).collect()
}

fn wrap_one_line(line: &str, width: usize) -> Vec<String> {
    if line.is_empty() {
        return vec![String::new()];
    }

    let mut wrapped = Vec::new();
    let mut current = String::new();
    for word in line.split_whitespace() {
        let candidate_len = if current.is_empty() { word.len() } else { current.len() + 1 + word.len() };
        if candidate_len > width && !current.is_empty() {
            wrapped.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() {
        wrapped.push(current);
    }
    wrapped
}

/// Transcodes UTF-8 to WinAnsiEncoding bytes for the base-14 Helvetica font used by `write_pdf`.
/// Code points up to U+00FF (the whole Latin-1 Supplement — covers á/à/â/ã/ç/é/ê/í/ó/ô/õ/ú/ü and
/// uppercase) map byte-for-byte, since WinAnsi agrees with Latin-1 in that range. Anything above
/// that (emoji, curly quotes, …) becomes `?` — a known v1 limitation, not a crash.
fn encode_winansi_lossy(s: &str) -> Vec<u8> {
    s.chars().map(|c| if (c as u32) < 0x100 { c as u8 } else { b'?' }).collect()
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
    async fn writes_a_pdf_file() {
        let root = temp_root();
        let tool = GenerateDocumentTool::new(root.clone());

        let content = "Relatório\n\nç ã é";
        tool.call(json!({ "filename": "relatorio.pdf", "content": content })).await.unwrap();

        let path = root.join("relatorio.pdf");
        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes.starts_with(b"%PDF-"));

        let doc = lopdf::Document::load(&path).unwrap();
        assert_eq!(doc.get_pages().len(), 1);
        let text = doc.extract_text(&[1]).unwrap();
        assert!(text.contains("Relatório"), "extracted text was: {text:?}");
        assert!(text.contains("ç ã é"), "extracted text was: {text:?}");
    }

    #[tokio::test]
    async fn writes_a_pdf_with_multiple_pages() {
        let root = temp_root();
        let tool = GenerateDocumentTool::new(root.clone());

        let content = "linha de conteúdo pra forçar paginação\n".repeat(200);
        tool.call(json!({ "filename": "grande.pdf", "content": &content })).await.unwrap();

        let doc = lopdf::Document::load(root.join("grande.pdf")).unwrap();
        assert!(doc.get_pages().len() >= 2, "expected multiple pages, got {}", doc.get_pages().len());
    }

    #[tokio::test]
    async fn rejects_unsupported_extension() {
        let tool = GenerateDocumentTool::new(temp_root());

        let err = tool.call(json!({ "filename": "relatorio.xlsx", "content": "x" })).await.unwrap_err();

        assert!(err.to_string().contains("only .txt, .md, .csv and .pdf"));
    }

    #[tokio::test]
    async fn rejects_missing_extension() {
        let tool = GenerateDocumentTool::new(temp_root());

        let err = tool.call(json!({ "filename": "relatorio", "content": "x" })).await.unwrap_err();

        assert!(err.to_string().contains("only .txt, .md, .csv and .pdf"));
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
