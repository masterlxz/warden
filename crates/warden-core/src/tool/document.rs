use std::path::{Path, PathBuf};

use async_trait::async_trait;
use lopdf::content::{Content, Operation};
use lopdf::{dictionary, Document, Object, Stream};
use rust_xlsxwriter::{Color, Format, Workbook, Worksheet};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::tool::{Tool, ToolSpec};

/// `generate_document` (P64) grows one format at a time, cheapest first. TXT/MD/CSV are all
/// plain-text writes — the model already produces well-formed CSV as a string, no parsing/
/// validation needed here. PDF renders the content as a simple paginated document (see
/// `write_pdf`). XLSX (see `write_xlsx`) takes structured `sheets` instead of `content` — a flat
/// string can't carry cell types, formulas or per-column formatting.
const SUPPORTED_EXTENSIONS: [&str; 5] = ["txt", "md", "csv", "pdf", "xlsx"];

// XLSX layout (v1): one fixed header style (bold, white-on-accent), optional per-column width
// (autofit otherwise) and an optional per-column display format — no per-cell styling. Same
// "capricho sem virar motor de estilo" bar already accepted for the PDF slice.
const XLSX_HEADER_BG: u32 = 0x4472C4;

#[derive(Deserialize)]
struct SheetSpec {
    name: Option<String>,
    columns: Vec<ColumnSpec>,
    #[serde(default)]
    rows: Vec<Vec<Value>>,
}

#[derive(Deserialize)]
struct ColumnSpec {
    header: String,
    width: Option<f64>,
    format: Option<String>,
}

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
                Supports .txt, .md, .csv, .pdf and .xlsx filenames. For .txt/.md/.csv/.pdf, pass 'content' as a string. \
                For .xlsx, pass 'sheets' (structured spreadsheet data) instead — 'content' is not used for spreadsheets."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "filename": {
                        "type": "string",
                        "description": "File name with extension, e.g. 'relatorio.md' or 'vendas.xlsx'. Only .txt, .md, .csv, .pdf and .xlsx are supported today."
                    },
                    "content": {
                        "type": "string",
                        "description": "Full file content to write — required for .txt/.md/.csv/.pdf, not used for .xlsx. For .csv, this must already be well-formed CSV text (header row + comma-separated values, quoted as needed). For .pdf, plain text — it is laid out as a simple wrapped/paginated document, not rendered as Markdown/HTML."
                    },
                    "sheets": {
                        "type": "array",
                        "description": "Required for .xlsx (ignored for every other extension). One entry per worksheet, in order.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "name": {
                                    "type": "string",
                                    "description": "Sheet tab name, max 31 characters. Defaults to Sheet1, Sheet2, ..."
                                },
                                "columns": {
                                    "type": "array",
                                    "description": "Defines the header row (row 1 — always bold, white text on a highlight background) and per-column formatting.",
                                    "items": {
                                        "type": "object",
                                        "properties": {
                                            "header": { "type": "string", "description": "Column header text." },
                                            "width": { "type": "number", "description": "Column width in characters. Omit to auto-fit to the widest cell in the column." },
                                            "format": {
                                                "type": "string",
                                                "enum": ["text", "number", "currency", "percent", "date"],
                                                "description": "How data cells in this column are displayed. Omit for plain text/default number display."
                                            }
                                        },
                                        "required": ["header"]
                                    }
                                },
                                "rows": {
                                    "type": "array",
                                    "description": "Data rows, starting at row 2 (row 1 is the header). Each row is an array with one value per column: a string, number, boolean, or null (blank cell). A string starting with '=' is written as a real Excel formula (e.g. '=SUM(B2:B3)') — Excel/LibreOffice computes it when the file is opened, this tool does not evaluate formulas itself.",
                                    "items": { "type": "array", "items": {} }
                                }
                            },
                            "required": ["columns", "rows"]
                        }
                    }
                },
                "required": ["filename"]
            }),
        }
    }

    async fn call(&self, args: Value) -> anyhow::Result<Value> {
        let filename = args.get("filename").and_then(Value::as_str).ok_or_else(|| anyhow::anyhow!("missing required 'filename' argument"))?;

        let extension = std::path::Path::new(filename).extension().and_then(|e| e.to_str()).map(str::to_lowercase);
        if !extension.as_deref().is_some_and(|e| SUPPORTED_EXTENSIONS.contains(&e)) {
            anyhow::bail!("unsupported file extension for 'generate_document' — only .txt, .md, .csv, .pdf and .xlsx are supported today");
        }

        std::fs::create_dir_all(&self.root)?;
        let path = self.root.join(filename);

        if extension.as_deref() == Some("xlsx") {
            let sheets_value = args.get("sheets").ok_or_else(|| anyhow::anyhow!("missing required 'sheets' argument for a .xlsx file"))?;
            let sheets: Vec<SheetSpec> = serde_json::from_value(sheets_value.clone()).map_err(|e| anyhow::anyhow!("invalid 'sheets' argument: {e}"))?;
            write_xlsx(&path, &sheets)?;
        } else {
            let content = args.get("content").and_then(Value::as_str).ok_or_else(|| anyhow::anyhow!("missing required 'content' argument"))?;
            if extension.as_deref() == Some("pdf") {
                write_pdf(&path, content)?;
            } else {
                std::fs::write(&path, content)?;
            }
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

/// Writes `sheets` as a real `.xlsx` workbook — one worksheet per entry, a fixed bold/highlighted
/// header row, an optional per-column display format, and real Excel formulas for any cell string
/// starting with `=` (written verbatim; Excel/LibreOffice computes the result on open — no Rust
/// crate evaluates formulas itself). See module-level `XLSX_HEADER_BG`.
fn write_xlsx(path: &Path, sheets: &[SheetSpec]) -> anyhow::Result<()> {
    let mut workbook = Workbook::new();
    let header_format = Format::new().set_bold().set_background_color(Color::RGB(XLSX_HEADER_BG)).set_font_color(Color::White);

    for sheet in sheets {
        let worksheet = workbook.add_worksheet();
        if let Some(name) = &sheet.name {
            worksheet.set_name(name)?;
        }

        for (col_idx, column) in sheet.columns.iter().enumerate() {
            worksheet.write_string_with_format(0, col_idx as u16, &column.header, &header_format)?;
        }

        let data_formats: Vec<Option<Format>> = sheet.columns.iter().map(|c| c.format.as_deref().map(xlsx_num_format)).collect();

        for (row_idx, row) in sheet.rows.iter().enumerate() {
            let row_num = (row_idx + 1) as u32;
            for (col_idx, cell) in row.iter().enumerate() {
                let format = data_formats.get(col_idx).and_then(|f| f.as_ref());
                write_xlsx_cell(worksheet, row_num, col_idx as u16, cell, format)?;
            }
        }

        worksheet.autofit();
        for (col_idx, column) in sheet.columns.iter().enumerate() {
            if let Some(width) = column.width {
                worksheet.set_column_width(col_idx as u16, width)?;
            }
        }
    }

    workbook.save(path)?;
    Ok(())
}

/// Maps a `columns[].format` hint to an Excel display format. Not localized (e.g. `currency`
/// always renders with `$`) — a known v1 limitation, same posture as the WinAnsi-only accent
/// handling accepted for PDF.
fn xlsx_num_format(format: &str) -> Format {
    let pattern = match format {
        "currency" => "$#,##0.00",
        "percent" => "0.00%",
        "date" => "yyyy-mm-dd",
        "number" => "#,##0.00",
        _ => "General",
    };
    Format::new().set_num_format(pattern)
}

fn write_xlsx_cell(worksheet: &mut Worksheet, row: u32, col: u16, cell: &Value, format: Option<&Format>) -> anyhow::Result<()> {
    match cell {
        Value::Null => {}
        Value::Bool(b) => {
            match format {
                Some(f) => worksheet.write_boolean_with_format(row, col, *b, f),
                None => worksheet.write_boolean(row, col, *b),
            }?;
        }
        Value::Number(n) => {
            let num = n.as_f64().ok_or_else(|| anyhow::anyhow!("invalid numeric cell value in 'sheets'"))?;
            match format {
                Some(f) => worksheet.write_number_with_format(row, col, num, f),
                None => worksheet.write_number(row, col, num),
            }?;
        }
        Value::String(s) if s.starts_with('=') => {
            match format {
                Some(f) => worksheet.write_formula_with_format(row, col, s.as_str(), f),
                None => worksheet.write_formula(row, col, s.as_str()),
            }?;
        }
        Value::String(s) => {
            match format {
                Some(f) => worksheet.write_string_with_format(row, col, s, f),
                None => worksheet.write_string(row, col, s),
            }?;
        }
        other => anyhow::bail!("unsupported cell value in 'sheets': {other}"),
    }
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
    async fn writes_an_xlsx_file() {
        use calamine::{open_workbook, Data, Reader, Xlsx};

        let root = temp_root();
        let tool = GenerateDocumentTool::new(root.clone());

        let result = tool
            .call(json!({
                "filename": "vendas.xlsx",
                "sheets": [{
                    "name": "Vendas",
                    "columns": [
                        { "header": "Produto", "width": 24.0 },
                        { "header": "Preço", "format": "currency" },
                        { "header": "Qtd" },
                        { "header": "Total", "format": "currency" }
                    ],
                    "rows": [
                        ["Caneta", 2.5, 100, "=B2*C2"],
                        ["Total geral", null, null, "=SUM(D2:D2)"]
                    ]
                }]
            }))
            .await
            .unwrap();

        let path = root.join("vendas.xlsx");
        assert_eq!(result, json!({ "status": "ok", "path": path.display().to_string() }));

        let mut workbook: Xlsx<_> = open_workbook(&path).unwrap();
        let range = workbook.worksheet_range("Vendas").unwrap();
        assert_eq!(range.get_value((0, 0)), Some(&Data::String("Produto".to_string())));
        assert_eq!(range.get_value((1, 0)), Some(&Data::String("Caneta".to_string())));
        assert_eq!(range.get_value((1, 1)), Some(&Data::Float(2.5)));
        assert_eq!(range.get_value((2, 0)), Some(&Data::String("Total geral".to_string())));

        let formulas = workbook.worksheet_formula("Vendas").unwrap();
        assert_eq!(formulas.get_value((1, 3)).map(String::as_str), Some("B2*C2"));
        assert_eq!(formulas.get_value((2, 3)).map(String::as_str), Some("SUM(D2:D2)"));
    }

    #[tokio::test]
    async fn writes_an_xlsx_with_multiple_sheets() {
        use calamine::{open_workbook, Reader, Xlsx};

        let root = temp_root();
        let tool = GenerateDocumentTool::new(root.clone());

        tool.call(json!({
            "filename": "relatorio.xlsx",
            "sheets": [
                { "name": "Um", "columns": [{ "header": "A" }], "rows": [["x"]] },
                { "name": "Dois", "columns": [{ "header": "B" }], "rows": [["y"]] }
            ]
        }))
        .await
        .unwrap();

        let workbook: Xlsx<_> = open_workbook(root.join("relatorio.xlsx")).unwrap();
        assert_eq!(workbook.sheet_names(), vec!["Um".to_string(), "Dois".to_string()]);
    }

    #[tokio::test]
    async fn rejects_missing_sheets_for_xlsx() {
        let tool = GenerateDocumentTool::new(temp_root());

        let err = tool.call(json!({ "filename": "vendas.xlsx" })).await.unwrap_err();

        assert!(err.to_string().contains("'sheets'"), "unexpected error: {err}");
    }

    #[tokio::test]
    async fn rejects_unsupported_extension() {
        let tool = GenerateDocumentTool::new(temp_root());

        let err = tool.call(json!({ "filename": "relatorio.docx", "content": "x" })).await.unwrap_err();

        assert!(err.to_string().contains("only .txt, .md, .csv, .pdf and .xlsx"));
    }

    #[tokio::test]
    async fn rejects_missing_extension() {
        let tool = GenerateDocumentTool::new(temp_root());

        let err = tool.call(json!({ "filename": "relatorio", "content": "x" })).await.unwrap_err();

        assert!(err.to_string().contains("only .txt, .md, .csv, .pdf and .xlsx"));
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
