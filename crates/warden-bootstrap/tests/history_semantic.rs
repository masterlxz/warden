//! `search_history` finding earlier messages by meaning, with the real multilingual model (P115 c). `#[ignore]`d: the
//! first run downloads the ONNX model (~120 MB). Run with
//!   cargo test -p warden-bootstrap --test history_semantic -- --ignored --nocapture

use std::time::Instant;

use serde_json::json;
use warden_bootstrap::history::SearchHistoryTool;
use warden_bootstrap::{save_conversation, ChatRole, Conversation, ConversationMessage};
use warden_core::tool::Tool;

fn message(role: ChatRole, content: &str, at: i64) -> ConversationMessage {
    ConversationMessage { id: format!("m{at}"), role, content: content.into(), created_at: at, usage: None, attachments: Vec::new(), generated_files: Vec::new(), tools_used: Vec::new() }
}

fn conversation(id: &str, title: &str, messages: Vec<ConversationMessage>) -> Conversation {
    let updated_at = messages.last().map(|m| m.created_at).unwrap_or(0);
    Conversation { id: id.into(), title: title.into(), messages, created_at: 1, updated_at, agent_id: None, provider_id: None, project_id: None, engine_session_id: None }
}

#[tokio::test]
#[ignore = "downloads a real ONNX model from Hugging Face on first run — needs network"]
async fn a_paraphrase_in_portuguese_or_english_finds_the_message_that_says_it_differently() {
    let dir = std::env::temp_dir().join(format!("warden-history-semantic-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir_all(&dir).unwrap();
    let topics: &[(&str, &str)] = &[
        ("smtp", "Montei o SMTP do servidor usando a porta 587 com STARTTLS e a senha de aplicativo."),
        ("bolo", "Receita de bolo de cenoura com cobertura de chocolate, quarenta minutos no forno."),
        ("hotel", "Quero reservar um hotel pequeno em Lisboa perto do rio Tejo para a viagem de maio."),
        ("release", "Separe as notas de versão em Adicionado, Corrigido e Removido, sem emoji."),
        ("backup", "O backup noturno do banco roda às 3h e guarda sete cópias antes de apagar a mais antiga."),
        ("small", "Oi, tudo bem? Só queria dar bom dia para você hoje."),
    ];
    for (i, (id, text)) in topics.iter().enumerate() {
        save_conversation(&dir, &conversation(id, id, vec![message(ChatRole::User, text, i as i64 * 10)])).unwrap();
    }
    let tool = SearchHistoryTool::new(Some(dir.clone()));

    let started = Instant::now();
    let first = tool.call(json!({ "query": "como configurei o e-mail de saída do servidor?" })).await.unwrap();
    println!("\nfirst search (loads the model and indexes {} messages): {:?}", topics.len(), started.elapsed());
    assert_eq!(first["semantic"], true, "{first}");
    assert_eq!(first["results"][0]["conversation_id"], "smtp", "{first}");

    let asked: &[(&str, &str)] = &[
        ("where was I going to stay in the Portuguese capital", "hotel"),
        ("qual era o formato que eu queria para as notas de lançamento", "release"),
        ("receita de sobremesa com cenoura", "bolo"),
        ("de quanto em quanto tempo o banco é copiado e quantas cópias ficam", "backup"),
    ];
    let mut right = 0;
    for (query, wanted) in asked {
        let started = Instant::now();
        let found = tool.call(json!({ "query": query })).await.unwrap();
        let top = found["results"][0]["conversation_id"].as_str().unwrap_or("-");
        println!("{query:<66} -> {top:<8} ({:?}, {} results)", started.elapsed(), found["results"].as_array().unwrap().len());
        right += (top == *wanted) as usize;
    }
    assert!(right >= 3, "at least 3 of 4 paraphrases should find their message, got {right}");

    let nothing = tool.call(json!({ "query": "astronomia e buracos negros" })).await.unwrap();
    println!("unrelated question: {} results", nothing["results"].as_array().unwrap().len());
    assert!(nothing["results"].as_array().unwrap().is_empty(), "an unrelated question finds nothing: {nothing}");
    std::fs::remove_dir_all(dir).ok();
}
