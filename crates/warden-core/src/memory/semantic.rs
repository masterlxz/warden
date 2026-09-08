//! Semantic search over the vault (Fase 4.5) — chunking, hashing and the on-disk index format.
//! `TextEmbedding` itself (the ONNX model) is owned by `Vault`, not this module, since it needs to
//! be loaded once and reused across calls.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;

/// Identifies which embedding model produced an index — bumped whenever the model choice changes,
/// so a stale index (different dimensions) is discarded instead of mixed with new embeddings.
pub const MODEL_ID: &str = "AllMiniLML6V2";

/// Fixed-size, non-overlapping line window per chunk. Not paragraph-aware — good enough for
/// typically-short personal notes (v1 scope); revisit if long documents make chunks too coarse.
pub const CHUNK_WINDOW_LINES: usize = 40;

/// Dot-prefixed so `Vault::list_files`/`list_all_files` (and therefore `warden-sync`) skip it —
/// same `is_dotfile` check that already excludes `.git`/`.DS_Store` from both.
pub const INDEX_DIR: &str = ".warden";
pub const INDEX_FILE: &str = "semantic_index.json";

const PREVIEW_MAX_CHARS: usize = 200;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexedChunk {
    pub path: String,
    pub start_line: usize,
    pub hash: String,
    pub preview: String,
    pub embedding: Vec<f32>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct SemanticIndex {
    pub model_id: String,
    pub chunks: Vec<IndexedChunk>,
}

impl SemanticIndex {
    pub fn new() -> Self {
        Self { model_id: MODEL_ID.to_string(), chunks: Vec::new() }
    }

    /// Missing file, unreadable JSON, or an index built with a different model all collapse to a
    /// fresh empty index — the caller repopulates it from scratch, same cost as a first run.
    pub fn load(path: &Path) -> Self {
        let loaded = std::fs::read_to_string(path)
            .ok()
            .and_then(|content| serde_json::from_str::<Self>(&content).ok());
        match loaded {
            Some(index) if index.model_id == MODEL_ID => index,
            _ => Self::new(),
        }
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Ok(std::fs::write(path, serde_json::to_string(self)?)?)
    }
}

/// Splits `content` into `(start_line, text)` windows of up to `window` lines each (1-based line
/// numbers, no overlap). Empty/whitespace-only content yields no chunks.
pub fn chunk_file(content: &str, window: usize) -> Vec<(usize, String)> {
    if content.trim().is_empty() {
        return Vec::new();
    }
    let lines: Vec<&str> = content.lines().collect();
    lines.chunks(window).enumerate().map(|(i, group)| (i * window + 1, group.join("\n"))).collect()
}

pub fn hash_chunk(text: &str) -> String {
    Sha256::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// Short excerpt shown as `SearchHit::line` for a semantic hit — chunks can span many lines, so
/// (unlike a grep hit) this is never the full matched text, just enough to recognize it.
pub fn preview(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= PREVIEW_MAX_CHARS {
        trimmed.to_string()
    } else {
        let truncated: String = trimmed.chars().take(PREVIEW_MAX_CHARS).collect();
        format!("{truncated}…")
    }
}

/// Fastembed already L2-normalizes its output, so this reduces to a dot product in practice — kept
/// as full cosine similarity so correctness doesn't silently depend on that upstream detail.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm_a = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_file_splits_into_fixed_windows_with_correct_start_lines() {
        let content = (1..=100).map(|n| format!("line{n}")).collect::<Vec<_>>().join("\n");
        let chunks = chunk_file(&content, 40);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].0, 1);
        assert_eq!(chunks[1].0, 41);
        assert_eq!(chunks[2].0, 81);
        assert!(chunks[0].1.starts_with("line1\n"));
        assert!(chunks[2].1.starts_with("line81\n"));
    }

    #[test]
    fn chunk_file_empty_or_blank_yields_no_chunks() {
        assert!(chunk_file("", 40).is_empty());
        assert!(chunk_file("   \n\n  ", 40).is_empty());
    }

    #[test]
    fn hash_chunk_is_stable_and_sensitive_to_content() {
        assert_eq!(hash_chunk("hello"), hash_chunk("hello"));
        assert_ne!(hash_chunk("hello"), hash_chunk("hello!"));
    }

    #[test]
    fn preview_truncates_long_text_with_ellipsis() {
        let short = "buy milk";
        assert_eq!(preview(short), "buy milk");

        let long = "a".repeat(300);
        let result = preview(&long);
        assert_eq!(result.chars().count(), PREVIEW_MAX_CHARS + 1);
        assert!(result.ends_with('…'));
    }

    #[test]
    fn cosine_similarity_identical_vectors_is_one() {
        let v = vec![1.0, 2.0, 3.0];
        assert!((cosine_similarity(&v, &v) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_similarity_orthogonal_vectors_is_zero() {
        assert!((cosine_similarity(&[1.0, 0.0], &[0.0, 1.0])).abs() < 1e-6);
    }

    #[test]
    fn cosine_similarity_mismatched_or_empty_is_zero() {
        assert_eq!(cosine_similarity(&[1.0], &[1.0, 2.0]), 0.0);
        assert_eq!(cosine_similarity(&[], &[]), 0.0);
    }

    #[test]
    fn semantic_index_load_missing_file_yields_fresh_index() {
        let path = std::env::temp_dir().join(format!(
            "warden-semantic-index-test-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        let index = SemanticIndex::load(&path);
        assert_eq!(index.model_id, MODEL_ID);
        assert!(index.chunks.is_empty());
    }

    #[test]
    fn semantic_index_save_then_load_roundtrips() {
        let path = std::env::temp_dir().join(format!(
            "warden-semantic-index-test-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        let mut index = SemanticIndex::new();
        index.chunks.push(IndexedChunk {
            path: "a.md".to_string(),
            start_line: 1,
            hash: "abc".to_string(),
            preview: "hello".to_string(),
            embedding: vec![0.1, 0.2],
        });
        index.save(&path).unwrap();

        let loaded = SemanticIndex::load(&path);
        assert_eq!(loaded.chunks.len(), 1);
        assert_eq!(loaded.chunks[0].path, "a.md");
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn semantic_index_load_discards_index_from_a_different_model() {
        let path = std::env::temp_dir().join(format!(
            "warden-semantic-index-test-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        let mut stale = SemanticIndex::new();
        stale.model_id = "SomeOtherModel".to_string();
        stale.chunks.push(IndexedChunk {
            path: "a.md".to_string(),
            start_line: 1,
            hash: "abc".to_string(),
            preview: "hello".to_string(),
            embedding: vec![0.1, 0.2],
        });
        stale.save(&path).unwrap();

        let loaded = SemanticIndex::load(&path);
        assert_eq!(loaded.model_id, MODEL_ID);
        assert!(loaded.chunks.is_empty());
        std::fs::remove_file(&path).unwrap();
    }
}
