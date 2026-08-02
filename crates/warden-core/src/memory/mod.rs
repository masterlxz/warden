use std::path::{Path, PathBuf};

/// Markdown vault on local disk (Obsidian-compatible). IPFS mirroring lands in Phase 4.
pub struct Vault {
    root: PathBuf,
}

/// One matching line from `Vault::search`, with enough location info to cite it.
pub struct SearchHit {
    pub path: String,
    pub line_number: usize,
    pub line: String,
}

impl Vault {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let _ = std::fs::create_dir_all(&root);
        Self { root }
    }

    pub fn root(&self) -> &PathBuf {
        &self.root
    }

    pub fn read(&self, relative_path: &str) -> anyhow::Result<String> {
        Ok(std::fs::read_to_string(self.root.join(relative_path))?)
    }

    pub fn write(&self, relative_path: &str, content: &str) -> anyhow::Result<()> {
        let path = self.root.join(relative_path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Ok(std::fs::write(path, content)?)
    }

    /// All markdown files in the vault, relative to its root.
    pub fn list_files(&self) -> anyhow::Result<Vec<PathBuf>> {
        let mut files = Vec::new();
        collect_markdown_files(&self.root, &self.root, &mut files)?;
        Ok(files)
    }

    /// Naive grep: case-insensitive substring match on any query word (3+ chars)
    /// across every markdown file. Good enough for v1 memory retrieval — semantic
    /// search is Phase 4 territory (see ARCHITECTURE.md, PENDING.md P5/P6).
    pub fn search(&self, query: &str, max_hits: usize) -> anyhow::Result<Vec<SearchHit>> {
        let words: Vec<String> = query
            .split_whitespace()
            .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase())
            .filter(|w| w.len() >= 3)
            .collect();

        if words.is_empty() {
            return Ok(Vec::new());
        }

        let mut hits = Vec::new();
        for relative in self.list_files()? {
            if hits.len() >= max_hits {
                break;
            }
            let content = std::fs::read_to_string(self.root.join(&relative))?;
            for (i, line) in content.lines().enumerate() {
                if hits.len() >= max_hits {
                    break;
                }
                let lower = line.to_lowercase();
                if words.iter().any(|w| lower.contains(w.as_str())) {
                    hits.push(SearchHit {
                        path: relative.to_string_lossy().to_string(),
                        line_number: i + 1,
                        line: line.to_string(),
                    });
                }
            }
        }
        Ok(hits)
    }
}

fn collect_markdown_files(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_markdown_files(root, &path, out)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
            out.push(path.strip_prefix(root)?.to_path_buf());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_vault() -> Vault {
        let dir = std::env::temp_dir().join(format!(
            "warden-vault-test-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        Vault::new(dir)
    }

    #[test]
    fn write_then_read_roundtrip() {
        let vault = temp_vault();
        vault.write("notes/todo.md", "buy milk").unwrap();
        assert_eq!(vault.read("notes/todo.md").unwrap(), "buy milk");
    }

    #[test]
    fn list_files_finds_nested_markdown_only() {
        let vault = temp_vault();
        vault.write("a.md", "one").unwrap();
        vault.write("nested/b.md", "two").unwrap();
        vault.write("nested/notes.txt", "ignored").unwrap();

        let mut files: Vec<String> =
            vault.list_files().unwrap().into_iter().map(|p| p.to_string_lossy().to_string()).collect();
        files.sort();

        assert_eq!(files, vec!["a.md".to_string(), "nested/b.md".to_string()]);
    }

    #[test]
    fn search_matches_case_insensitively_and_caps_results() {
        let vault = temp_vault();
        vault.write("dentist.md", "Appointment with the Dentist on Friday").unwrap();
        vault.write("unrelated.md", "Grocery list: eggs, bread").unwrap();

        let hits = vault.search("dentist appointment", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "dentist.md");
        assert_eq!(hits[0].line_number, 1);

        let capped = vault.search("dentist appointment", 0).unwrap();
        assert!(capped.is_empty());
    }
}
