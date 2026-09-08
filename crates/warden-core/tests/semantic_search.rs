//! End-to-end coverage for `Vault::search_semantic` (Fase 4.5). The deterministic pieces
//! (chunking/hashing/cosine math) are unit-tested in `memory/semantic.rs` without a real model.
//! These tests exercise the real ONNX model via `fastembed`, which downloads it from Hugging Face
//! on first use — `#[ignore]`d by default so `cargo test`/CI stay hermetic and fast regardless of
//! network access, same posture already accepted for other real-external-service checks in this
//! project (see PENDING.md P29/P30/P31/P38). Run explicitly with:
//!   cargo test -p warden-core --test semantic_search -- --ignored

use warden_core::memory::Vault;

fn temp_vault() -> Vault {
    let dir = std::env::temp_dir().join(format!(
        "warden-semantic-e2e-{}",
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    Vault::new(dir)
}

#[test]
#[ignore = "downloads a real ONNX model from Hugging Face on first run — needs network"]
fn search_semantic_ranks_by_meaning_not_substring() {
    let vault = temp_vault();
    vault.write("dentist.md", "Appointment with the dentist on Friday morning.").unwrap();
    vault.write("groceries.md", "Buy eggs, bread and milk at the store.").unwrap();

    // No literal word overlap with either note, but "medical checkup" is semantically closest to
    // the dentist appointment — a substring grep would find nothing here at all.
    let hits = vault.search_semantic("do I have a medical checkup coming up?", 1).unwrap();

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].path, "dentist.md");
}

#[test]
#[ignore = "downloads a real ONNX model from Hugging Face on first run — needs network"]
fn search_semantic_is_incremental_across_calls() {
    let vault = temp_vault();
    vault.write("a.md", "The quick brown fox jumps over the lazy dog.").unwrap();

    let first = vault.search_semantic("fox", 5).unwrap();
    assert_eq!(first.len(), 1);

    // A second call with no vault changes should reuse the persisted index rather than fail or
    // duplicate entries.
    let second = vault.search_semantic("fox", 5).unwrap();
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].path, first[0].path);

    // Editing the file (as if from outside the app entirely, e.g. Obsidian) must be picked up.
    vault.write("a.md", "Completely different content about spreadsheets and budgets.").unwrap();
    let after_edit = vault.search_semantic("budgets", 5).unwrap();
    assert_eq!(after_edit.len(), 1);
    assert!(after_edit[0].line.contains("budgets"));
}
