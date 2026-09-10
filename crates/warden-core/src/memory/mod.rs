#[cfg(feature = "semantic-search")]
mod semantic;

#[cfg(feature = "semantic-search")]
use fastembed::{EmbeddingModel, TextEmbedding, TextInitOptions};
#[cfg(feature = "semantic-search")]
use std::collections::HashMap;
use std::path::{Path, PathBuf};
#[cfg(feature = "semantic-search")]
use std::sync::Mutex;

/// Markdown vault on local disk (Obsidian-compatible). IPFS mirroring lands in Phase 4.
pub struct Vault {
    root: PathBuf,
    /// Lazily loaded on first semantic search (downloads the ONNX model on first-ever use) and
    /// reused after that — `Vault` is held behind `Arc` for the life of the process, so this loads
    /// at most once per process, not once per turn. Behind the `semantic-search` feature (default
    /// on) — off for crates that only need `Vault` for file I/O (e.g. `warden-sync`), since `ort`
    /// (fastembed's ONNX runtime) has no prebuilt binary for some cross-compile targets those
    /// crates build for (Android's `armv7-linux-androideabi`, via `warden-mobile-bridge`).
    #[cfg(feature = "semantic-search")]
    embedder: Mutex<Option<TextEmbedding>>,
    /// Guards the read-refresh-write cycle of the on-disk semantic index (`.warden/semantic_index.json`)
    /// against two turns (e.g. two channels sharing one `warden-server` vault) racing each other.
    #[cfg(feature = "semantic-search")]
    index_lock: Mutex<()>,
}

/// One matching line from `Vault::search`, with enough location info to cite it.
pub struct SearchHit {
    pub path: String,
    pub line_number: usize,
    pub line: String,
}

/// Reserved vault-root filenames for the "fixed/standard" memory (P52) — always injected via
/// `Vault::standing_memory`, never surfaced through `search`/`search_semantic` (would otherwise
/// double up with the standing-memory block and eat into the free-form notes' hit budget).
/// `warden-bootstrap::seed_default_vault_files` seeds these with a starter template on first use.
pub const FIXED_VAULT_FILES: [&str; 3] = ["_profile.md", "_behavior.md", "_feedback.md"];

impl Vault {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let _ = std::fs::create_dir_all(&root);
        Self {
            root,
            #[cfg(feature = "semantic-search")]
            embedder: Mutex::new(None),
            #[cfg(feature = "semantic-search")]
            index_lock: Mutex::new(()),
        }
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

    /// Removes a file from the vault. Added for `StorageProvider`/`LocalFSProvider` (P61) — before
    /// this, the only deletion path (`warden-sync`'s bundle-apply, P37) reached past `Vault`
    /// straight into `std::fs::remove_file`; that call site now goes through here instead.
    pub fn delete(&self, relative_path: &str) -> anyhow::Result<()> {
        Ok(std::fs::remove_file(self.root.join(relative_path))?)
    }

    /// All markdown files in the vault, relative to its root.
    pub fn list_files(&self) -> anyhow::Result<Vec<PathBuf>> {
        let mut files = Vec::new();
        collect_markdown_files(&self.root, &self.root, &mut files)?;
        Ok(files)
    }

    /// Every regular file in the vault, any extension — unlike `list_files`, which only returns
    /// `.md` (the memory `search` reads). Used by sync (Fase 4/P37), which mirrors the whole
    /// vault, not just the markdown subset. Skips dotfiles/dot-directories (OS/editor cruft like
    /// `.DS_Store`, `.git`) — same "good enough for v1" posture as `search`'s naive grep.
    pub fn list_all_files(&self) -> anyhow::Result<Vec<PathBuf>> {
        let mut files = Vec::new();
        collect_all_files(&self.root, &self.root, &mut files)?;
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

    /// The "fixed/standard" memory block (P52) — unlike `search`/`search_semantic`, not keyed off
    /// any query: always reads `FIXED_VAULT_FILES` in order and returns one combined block, with a
    /// heading per file, skipping any that's missing or blank (an unseeded vault, or one where the
    /// user cleared a section on purpose). Returns an empty string when all three are empty/absent,
    /// so callers can skip injecting an empty system message.
    pub fn standing_memory(&self) -> String {
        const HEADINGS: [&str; 3] = ["User profile", "AI behavior", "Feedback / lessons learned"];
        let mut sections = Vec::new();
        for (name, heading) in FIXED_VAULT_FILES.iter().zip(HEADINGS) {
            if let Ok(content) = self.read(name) {
                let content = content.trim();
                if !content.is_empty() {
                    sections.push(format!("## {heading}\n\n{content}"));
                }
            }
        }
        sections.join("\n\n")
    }

    #[cfg(feature = "semantic-search")]
    /// Semantic counterpart to `search` — ranks markdown chunks by embedding similarity to `query`
    /// instead of substring match. Self-healing: re-hashes every chunk on each call and only
    /// re-embeds what's new or changed since the last call (including edits made outside the
    /// Warden app entirely, since the vault is a plain Obsidian-compatible directory), rather than
    /// hooking every write path. Returns the same `SearchHit` shape as `search`, so callers don't
    /// need to change — `line` becomes a chunk preview rather than the literal matched line.
    pub fn search_semantic(&self, query: &str, max_hits: usize) -> anyhow::Result<Vec<SearchHit>> {
        if max_hits == 0 || query.trim().is_empty() {
            return Ok(Vec::new());
        }

        let _guard = self.index_lock.lock().unwrap();
        let index_path = self.root.join(semantic::INDEX_DIR).join(semantic::INDEX_FILE);
        let index = semantic::SemanticIndex::load(&index_path);

        let mut wanted: Vec<(String, usize, String, String)> = Vec::new();
        for relative in self.list_files()? {
            let path = relative.to_string_lossy().to_string();
            let content = std::fs::read_to_string(self.root.join(&relative))?;
            for (start_line, text) in semantic::chunk_file(&content, semantic::CHUNK_WINDOW_LINES) {
                let hash = semantic::hash_chunk(&text);
                wanted.push((path.clone(), start_line, hash, text));
            }
        }

        let existing: HashMap<(&str, usize), &semantic::IndexedChunk> =
            index.chunks.iter().map(|c| ((c.path.as_str(), c.start_line), c)).collect();

        let mut rebuilt = Vec::with_capacity(wanted.len());
        let mut pending: Vec<usize> = Vec::new();
        for (i, (path, start_line, hash, text)) in wanted.iter().enumerate() {
            match existing.get(&(path.as_str(), *start_line)) {
                Some(chunk) if chunk.hash == *hash => rebuilt.push((*chunk).clone()),
                _ => {
                    rebuilt.push(semantic::IndexedChunk {
                        path: path.clone(),
                        start_line: *start_line,
                        hash: hash.clone(),
                        preview: semantic::preview(text),
                        embedding: Vec::new(),
                    });
                    pending.push(i);
                }
            }
        }

        if !pending.is_empty() {
            let texts: Vec<&str> = pending.iter().map(|&i| wanted[i].3.as_str()).collect();
            let embeddings = self.with_embedder(|model| Ok(model.embed(texts, None)?))?;
            for (slot, embedding) in pending.into_iter().zip(embeddings) {
                rebuilt[slot].embedding = embedding;
            }
        }

        let mut index = index;
        index.chunks = rebuilt;
        let _ = index.save(&index_path);

        let query_embedding = self.with_embedder(|model| {
            Ok(model.embed(vec![query], None)?.into_iter().next().unwrap_or_default())
        })?;

        let mut scored: Vec<(f32, &semantic::IndexedChunk)> = index
            .chunks
            .iter()
            .filter(|c| !c.embedding.is_empty())
            .map(|c| (semantic::cosine_similarity(&query_embedding, &c.embedding), c))
            .collect();
        scored.sort_by(|a, b| b.0.total_cmp(&a.0));

        Ok(scored
            .into_iter()
            .take(max_hits)
            .map(|(_, c)| SearchHit { path: c.path.clone(), line_number: c.start_line, line: c.preview.clone() })
            .collect())
    }

    /// Lazily loads the local embedding model (downloads it on first-ever use — the one part of
    /// "local" search that needs network, exactly once per machine) and runs `f` against it.
    #[cfg(feature = "semantic-search")]
    fn with_embedder<T>(&self, f: impl FnOnce(&mut TextEmbedding) -> anyhow::Result<T>) -> anyhow::Result<T> {
        let mut guard = self.embedder.lock().unwrap();
        if guard.is_none() {
            let model = TextEmbedding::try_new(
                TextInitOptions::new(EmbeddingModel::AllMiniLML6V2)
                    .with_cache_dir(model_cache_dir())
                    .with_show_download_progress(false),
            )?;
            *guard = Some(model);
        }
        f(guard.as_mut().expect("just initialized above"))
    }
}

/// Where the (shared, not per-vault) ONNX model file lives once downloaded — not vault content,
/// so it must not live inside a vault directory or get duplicated per vault.
#[cfg(feature = "semantic-search")]
fn model_cache_dir() -> PathBuf {
    dirs::cache_dir().unwrap_or_else(std::env::temp_dir).join("warden").join("models")
}

fn collect_markdown_files(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_markdown_files(root, &path, out)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("md") && !is_fixed_vault_file(root, &path) {
            out.push(path.strip_prefix(root)?.to_path_buf());
        }
    }
    Ok(())
}

/// True for `_profile.md`/`_behavior.md`/`_feedback.md` at the vault root specifically — a
/// same-named file nested under a subdirectory (e.g. a user's own `notes/_profile.md`) is a
/// regular note, not the reserved one, so only the root-level match is excluded from search.
fn is_fixed_vault_file(root: &Path, path: &Path) -> bool {
    path.parent() == Some(root)
        && path.file_name().and_then(|n| n.to_str()).is_some_and(|name| FIXED_VAULT_FILES.contains(&name))
}

fn is_dotfile(path: &Path) -> bool {
    path.file_name().and_then(|n| n.to_str()).map(|n| n.starts_with('.')).unwrap_or(false)
}

fn collect_all_files(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if is_dotfile(&path) {
            continue;
        }
        if path.is_dir() {
            collect_all_files(root, &path, out)?;
        } else {
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
    fn delete_removes_a_written_file() {
        let vault = temp_vault();
        vault.write("notes/todo.md", "buy milk").unwrap();
        vault.delete("notes/todo.md").unwrap();
        assert!(vault.read("notes/todo.md").is_err());
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
    fn list_all_files_finds_every_file_including_non_markdown() {
        let vault = temp_vault();
        vault.write("a.md", "one").unwrap();
        vault.write("nested/notes.txt", "ignored by list_files").unwrap();
        vault.write("nested/.hidden/secret.md", "excluded, dot-directory").unwrap();
        vault.write(".dotfile", "excluded, dotfile").unwrap();

        let mut files: Vec<String> =
            vault.list_all_files().unwrap().into_iter().map(|p| p.to_string_lossy().to_string()).collect();
        files.sort();

        assert_eq!(files, vec!["a.md".to_string(), "nested/notes.txt".to_string()]);
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

    #[test]
    fn fixed_vault_files_excluded_from_list_and_search_but_not_a_nested_same_name_file() {
        let vault = temp_vault();
        vault.write("_profile.md", "name: dentist appointment person").unwrap();
        vault.write("notes/_profile.md", "a real note that happens to share the name").unwrap();
        vault.write("a.md", "unrelated dentist appointment note").unwrap();

        let mut files: Vec<String> =
            vault.list_files().unwrap().into_iter().map(|p| p.to_string_lossy().to_string()).collect();
        files.sort();
        assert_eq!(files, vec!["a.md".to_string(), "notes/_profile.md".to_string()]);

        let hits = vault.search("dentist appointment", 10).unwrap();
        assert!(hits.iter().all(|h| h.path != "_profile.md"));
        assert!(hits.iter().any(|h| h.path == "a.md"));
    }

    #[test]
    fn standing_memory_skips_missing_and_blank_files() {
        let vault = temp_vault();
        assert_eq!(vault.standing_memory(), "");

        vault.write("_profile.md", "  \n").unwrap(); // blank after trim
        assert_eq!(vault.standing_memory(), "");

        vault.write("_behavior.md", "Be concise.").unwrap();
        assert_eq!(vault.standing_memory(), "## AI behavior\n\nBe concise.");
    }

    #[test]
    fn standing_memory_joins_present_sections_in_fixed_order() {
        let vault = temp_vault();
        vault.write("_feedback.md", "Prefers terse answers.").unwrap();
        vault.write("_profile.md", "Name: Ada.").unwrap();

        assert_eq!(
            vault.standing_memory(),
            "## User profile\n\nName: Ada.\n\n## Feedback / lessons learned\n\nPrefers terse answers."
        );
    }
}
