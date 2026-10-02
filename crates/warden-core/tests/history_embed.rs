//! The multilingual embedder behind `search_history` (P115 c), against the real model. `#[ignore]`d like
//! `semantic_search.rs`: the first run downloads the ONNX model (~120 MB) from Hugging Face. Run with
//!   cargo test -p warden-core --test history_embed -- --ignored --nocapture

use warden_core::memory::embed::{cosine_similarity, embed, Kind};

fn score(query: &str, passage: &str) -> f32 {
    let q = embed(Kind::Query, vec![query.to_string()]).unwrap().remove(0);
    let p = embed(Kind::Passage, vec![passage.to_string()]).unwrap().remove(0);
    cosine_similarity(&q, &p)
}

const PASSAGES: &[(&str, &str)] = &[
    ("smtp", "Montei o SMTP do servidor usando a porta 587 com STARTTLS e a senha de aplicativo."),
    ("bolo", "Receita de bolo de cenoura com cobertura de chocolate, 40 minutos no forno."),
    ("hotel", "Quero reservar um hotel pequeno em Lisboa perto do rio Tejo para a viagem de maio."),
    ("release", "Separe as notas de versão em Adicionado, Corrigido e Removido, sem emoji."),
    ("small-talk", "Oi, tudo bem? Só queria dar bom dia para você hoje."),
];

#[test]
#[ignore = "downloads a real ONNX model from Hugging Face on first run — needs network"]
fn related_text_scores_clearly_above_unrelated_text_across_languages() {
    let queries: &[(&str, &str)] = &[
        ("como configurei o e-mail do servidor?", "smtp"),
        ("how did I set up outgoing mail on the server", "smtp"),
        ("onde eu ia me hospedar na capital portuguesa", "hotel"),
        ("qual era o formato que eu queria para as notas de lançamento", "release"),
        ("receita de sobremesa com cenoura", "bolo"),
        ("astronomia e buracos negros", ""),
        ("qual a capital da Austrália", ""),
    ];
    println!();
    let (mut related, mut unrelated) = (Vec::new(), Vec::new());
    for (query, wanted) in queries {
        let scores: Vec<(&str, f32)> = PASSAGES.iter().map(|(name, text)| (*name, score(query, text))).collect();
        println!("{query:<62} {}", scores.iter().map(|(n, s)| format!("{n}={s:.3}")).collect::<Vec<_>>().join("  "));
        for (name, s) in scores {
            if name == *wanted {
                related.push(s);
            } else {
                unrelated.push(s);
            }
        }
    }
    let lowest_related = related.iter().cloned().fold(f32::MAX, f32::min);
    let highest_unrelated = unrelated.iter().cloned().fold(f32::MIN, f32::max);
    println!("lowest related {lowest_related:.3}, highest unrelated {highest_unrelated:.3}");
    assert!(lowest_related > highest_unrelated, "the model should separate them: related {lowest_related}, unrelated {highest_unrelated}");
}
