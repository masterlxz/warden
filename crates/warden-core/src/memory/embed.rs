//! A shared, multilingual text embedder (P115 c) — what `search_history` uses to find an earlier message by what it
//! means, not by its exact words. Separate from the vault's own embedder on purpose: the vault's model
//! (`AllMiniLML6V2`) is trained almost only on English, and the conversations this one reads are often in
//! Portuguese. One model per process, loaded on first use (the first-ever use downloads it, ~120 MB, once per machine).
//!
//! The model is a quantized `paraphrase-multilingual-MiniLM-L12-v2`, made to say how alike two sentences are: related
//! text scores 0.3-0.8 and unrelated text around 0 (measured in `tests/history_embed.rs`). `multilingual-e5-small` was
//! tried first and rejected: it scores everything 0.75-0.9, so nothing separates a match from noise.
//!
//! Behind the `semantic-search` feature like the vault's search; without it `embed` fails with a clear message and
//! callers fall back to matching words.

/// Identifies which model made an embedding, so a stored index made by another model is thrown away, not mixed in.
pub const HISTORY_MODEL_ID: &str = "ParaphraseMLMiniLML12V2Q";

/// Set (to anything) in the environment to keep the model from loading or downloading: callers fall back to words.
/// For tests and for a machine that is offline or short on memory.
pub const OFF_SWITCH: &str = "WARDEN_NO_SEMANTIC";

/// Which side of a search a text is. This model compares sentences symmetrically, so both are embedded the same way;
/// kept so a model that does tell them apart (the E5 family) can replace it without touching the callers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Query,
    Passage,
}

/// Embeds each of `texts`, in order. Slow the first time in a process (loads the model).
#[cfg(feature = "semantic-search")]
pub fn embed(_kind: Kind, texts: Vec<String>) -> anyhow::Result<Vec<Vec<f32>>> {
    use fastembed::{EmbeddingModel, TextEmbedding, TextInitOptions};
    use std::sync::Mutex;

    static MODEL: Mutex<Option<TextEmbedding>> = Mutex::new(None);

    if std::env::var_os(OFF_SWITCH).is_some_and(|v| !v.is_empty()) {
        anyhow::bail!("search by meaning is turned off ({OFF_SWITCH} is set)");
    }
    if texts.is_empty() {
        return Ok(Vec::new());
    }
    let mut guard = MODEL.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if guard.is_none() {
        *guard = Some(TextEmbedding::try_new(
            TextInitOptions::new(EmbeddingModel::ParaphraseMLMiniLML12V2Q).with_cache_dir(super::model_cache_dir()).with_show_download_progress(false),
        )?);
    }
    Ok(guard.as_mut().expect("loaded just above").embed(texts, None)?)
}

#[cfg(not(feature = "semantic-search"))]
pub fn embed(_kind: Kind, _texts: Vec<String>) -> anyhow::Result<Vec<Vec<f32>>> {
    anyhow::bail!("semantic search isn't built into this binary")
}

/// How alike two embeddings are, from -1 to 1 (1 = the same direction).
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let (norm_a, norm_b) = (a.iter().map(|x| x * x).sum::<f32>().sqrt(), b.iter().map(|x| x * x).sum::<f32>().sqrt());
    if norm_a == 0.0 || norm_b == 0.0 {
        0.0
    } else {
        dot / (norm_a * norm_b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_similarity_is_one_for_the_same_direction_and_zero_for_unrelated_or_mismatched() {
        assert!((cosine_similarity(&[1.0, 0.0], &[2.0, 0.0]) - 1.0).abs() < 1e-6);
        assert_eq!(cosine_similarity(&[1.0, 0.0], &[0.0, 1.0]), 0.0);
        assert_eq!(cosine_similarity(&[1.0], &[1.0, 0.0]), 0.0);
        assert_eq!(cosine_similarity(&[], &[]), 0.0);
        assert_eq!(cosine_similarity(&[0.0, 0.0], &[1.0, 1.0]), 0.0);
    }
}
