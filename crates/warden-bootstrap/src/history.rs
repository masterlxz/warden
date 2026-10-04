//! `search_history` (P104): the agent looks back through the conversations saved for whoever it is speaking
//! with — what they said and what was answered, weeks ago. The Hermes agent does the same with a full-text index
//! over its sessions; here the conversations are already files, so this reads them (decrypting a member's, which
//! fails as "locked" when the hub doesn't hold their key).
//!
//! Two ways of matching, merged (P115 c): the words of the query (a message matches the more of them it
//! contains), and meaning — an embedding of each message, compared with the query's, so a paraphrase or another
//! language still finds it. The embeddings live in an index next to the conversations (`.history-index`, sealed
//! like them in a member's folder, holding no text), are made a few hundred at a time as searches come in, and
//! when the embedding model isn't available the search is just the words, as before.
//!
//! The folder is the tool's own and the hub re-points it for every turn (`Tool::with_conversations_dir`): a person's
//! agent can only ever search that person's conversations.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use warden_core::memory::embed::{self, Kind};
use warden_core::tool::{Tool, ToolSpec};

use crate::{list_conversations, ChatRole};

const DEFAULT_LIMIT: usize = 8;
const MAX_LIMIT: usize = 20;
const MIN_WORD_LEN: usize = 3;
/// Characters of a message shown around the first match.
const SNIPPET_CHARS: usize = 240;
/// Shorter messages ("ok", "obrigado") say nothing worth finding by meaning.
const MIN_INDEXED_CHARS: usize = 20;
/// How many messages get embedded in one search; a big folder is indexed across several searches.
const MAX_NEW_EMBEDDINGS: usize = 300;
/// How many of the closest messages by meaning take part in the merge.
const SEMANTIC_TOP: usize = 50;
/// Below this cosine similarity a message isn't taken as related. Measured with the real model
/// (`warden-core/tests/history_embed.rs`): related text scored 0.34-0.77 and unrelated text up to 0.25.
const SIMILARITY_FLOOR: f32 = 0.30;
/// Reciprocal rank fusion constant: how fast a lower rank stops counting.
const RRF_K: f64 = 60.0;
const INDEX_FILE: &str = ".history-index";

/// How a batch of texts becomes embeddings — `warden_core::memory::embed::embed` in practice, swappable in tests.
pub type Embedder = fn(Kind, Vec<String>) -> anyhow::Result<Vec<Vec<f32>>>;

pub struct SearchHistoryTool {
    conversations_dir: Option<PathBuf>,
    embedder: Embedder,
}

impl SearchHistoryTool {
    pub fn new(conversations_dir: Option<PathBuf>) -> Self {
        Self { conversations_dir, embedder: embed::embed }
    }

    /// The same tool with another way of embedding (tests: a model that needs no download).
    pub fn with_embedder(mut self, embedder: Embedder) -> Self {
        self.embedder = embedder;
        self
    }
}

/// The words of `query` that count: lowercased, at least three letters, without repeats.
fn query_words(query: &str) -> Vec<String> {
    let mut words: Vec<String> = Vec::new();
    for word in query.split(|c: char| !c.is_alphanumeric()) {
        let word = word.to_lowercase();
        if word.chars().count() >= MIN_WORD_LEN && !words.contains(&word) {
            words.push(word);
        }
    }
    words
}

/// Around the first occurrence of any of `words`, at most `SNIPPET_CHARS` characters, on one line.
fn snippet(text: &str, words: &[String]) -> String {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let lower = flat.to_lowercase();
    let first = words.iter().filter_map(|w| lower.find(w.as_str())).min().unwrap_or(0);
    // `find` on the lowercase copy can differ from `flat` in byte offsets for a few scripts, so walk in characters.
    let chars: Vec<char> = flat.chars().collect();
    let first_char = lower[..first.min(lower.len())].chars().count().min(chars.len());
    let start = first_char.saturating_sub(SNIPPET_CHARS / 4);
    let end = (start + SNIPPET_CHARS).min(chars.len());
    let mut out: String = chars[start..end].iter().collect();
    if start > 0 {
        out.insert(0, '…');
    }
    if end < chars.len() {
        out.push('…');
    }
    out
}

/// What the embedding index remembers of one message: which one, a fingerprint of its text (so an edited message is
/// embedded again) and its embedding. No text.
#[derive(Serialize, Deserialize, Clone)]
struct IndexedMessage {
    conversation_id: String,
    message_id: String,
    fingerprint: u64,
    embedding: Vec<f32>,
}

#[derive(Serialize, Deserialize, Default)]
struct HistoryIndex {
    model_id: String,
    items: Vec<IndexedMessage>,
}

/// FNV-1a: stable across runs and Rust versions (unlike `DefaultHasher`), which is all a change check needs.
fn fingerprint(text: &str) -> u64 {
    text.bytes().fold(0xcbf29ce484222325, |hash, byte| (hash ^ byte as u64).wrapping_mul(0x100000001b3))
}

/// The index as stored in `dir` — opened with the folder's key when it's a member's — or an empty one when there is
/// none, it can't be read or another model made it.
fn load_index(dir: &Path) -> HistoryIndex {
    let Ok(bytes) = std::fs::read(dir.join(INDEX_FILE)) else {
        return HistoryIndex::default();
    };
    let bytes = match crate::member_crypto::dir_state(dir) {
        crate::member_crypto::DirState::Unlocked(cipher) if warden_core::memory::VaultCipher::is_sealed(&bytes) => match cipher.open(&bytes) {
            Ok(opened) => opened,
            Err(_) => return HistoryIndex::default(),
        },
        crate::member_crypto::DirState::Locked => return HistoryIndex::default(),
        _ => bytes,
    };
    serde_json::from_slice::<HistoryIndex>(&bytes).ok().filter(|index| index.model_id == embed::HISTORY_MODEL_ID).unwrap_or_default()
}

/// Writes the index sealed like the folder, through a temporary file so a search running at the same moment never reads
/// half of it. A failure only means the next search embeds again.
fn store_index(dir: &Path, index: &HistoryIndex) {
    let Ok(plain) = serde_json::to_vec(index) else { return };
    let bytes = match crate::member_crypto::dir_state(dir) {
        crate::member_crypto::DirState::Plain => plain,
        crate::member_crypto::DirState::Unlocked(cipher) => cipher.seal(&plain),
        crate::member_crypto::DirState::Locked => return,
    };
    let temp = dir.join(format!("{INDEX_FILE}.tmp"));
    if std::fs::write(&temp, bytes).is_ok() && std::fs::rename(&temp, dir.join(INDEX_FILE)).is_err() {
        let _ = std::fs::remove_file(&temp);
    }
}

/// What the meaning side of a search came to.
struct Semantic {
    /// `(conversation id, message id)` of the closest messages, closest first.
    ranked: Vec<(String, String)>,
    /// Messages with an embedding in the index now, and those still waiting for one.
    indexed: usize,
    pending: usize,
}

/// Brings the index up to date (at most `MAX_NEW_EMBEDDINGS` new messages, the most recent first) and ranks the
/// messages by closeness to `query`.
fn semantic_ranking(dir: &Path, conversations: &[crate::Conversation], query: &str, embedder: Embedder) -> anyhow::Result<Semantic> {
    // Newest conversation first (that's how `list_conversations` sorts), newest message first within it.
    let mut wanted: Vec<(&str, &str, u64, &str)> = Vec::new();
    for conversation in conversations {
        for message in conversation.messages.iter().rev().filter(|m| m.content.trim().chars().count() >= MIN_INDEXED_CHARS) {
            wanted.push((&conversation.id, &message.id, fingerprint(&message.content), &message.content));
        }
    }

    let index = load_index(dir);
    let mut known: HashMap<(&str, &str), &IndexedMessage> = index.items.iter().map(|i| ((i.conversation_id.as_str(), i.message_id.as_str()), i)).collect();
    let mut kept: Vec<IndexedMessage> = Vec::new();
    let mut missing: Vec<(&str, &str, u64, &str)> = Vec::new();
    for entry @ (conversation_id, message_id, print, _) in &wanted {
        match known.remove(&(*conversation_id, *message_id)) {
            Some(item) if item.fingerprint == *print && !item.embedding.is_empty() => kept.push(item.clone()),
            _ => missing.push(*entry),
        }
    }

    let batch = &missing[..missing.len().min(MAX_NEW_EMBEDDINGS)];
    let pending = missing.len() - batch.len();
    let mut changed = !index.items.is_empty() && kept.len() != index.items.len();
    if !batch.is_empty() {
        let embeddings = embedder(Kind::Passage, batch.iter().map(|(_, _, _, text)| text.to_string()).collect())?;
        anyhow::ensure!(embeddings.len() == batch.len(), "the embedder returned {} embeddings for {} messages", embeddings.len(), batch.len());
        for ((conversation_id, message_id, print, _), embedding) in batch.iter().zip(embeddings) {
            kept.push(IndexedMessage { conversation_id: conversation_id.to_string(), message_id: message_id.to_string(), fingerprint: *print, embedding });
        }
        changed = true;
    }
    if changed {
        store_index(dir, &HistoryIndex { model_id: embed::HISTORY_MODEL_ID.to_string(), items: kept.clone() });
    }

    let query_embedding = embedder(Kind::Query, vec![query.to_string()])?.into_iter().next().unwrap_or_default();
    let mut scored: Vec<(f32, &IndexedMessage)> = kept.iter().map(|item| (embed::cosine_similarity(&query_embedding, &item.embedding), item)).filter(|(score, _)| *score >= SIMILARITY_FLOOR).collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    Ok(Semantic {
        ranked: scored.into_iter().take(SEMANTIC_TOP).map(|(_, item)| (item.conversation_id.clone(), item.message_id.clone())).collect(),
        indexed: kept.len(),
        pending,
    })
}

#[async_trait]
impl Tool for SearchHistoryTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "search_history".to_string(),
            description: "Search the earlier conversations saved for the person you're talking with: what they said and what was answered, in this and other conversations. Use it when they refer to something from before (\"like last time\", \"what did we decide about…\"). Ask it the way you'd put the question, or give the subject: it finds messages that use those words and messages that mean the same thing in other words or another language. Each result names the conversation, the day and who said it, with a short excerpt.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "What to look for: a few distinctive words, or a short question about the subject" },
                    "limit": { "type": "integer", "description": "How many excerpts to return, 1-20 (default 8)" }
                },
                "required": ["query"]
            }),
        }
    }

    fn with_conversations_dir(&self, dir: &Path) -> Option<Arc<dyn Tool>> {
        Some(Arc::new(Self { conversations_dir: Some(dir.to_path_buf()), embedder: self.embedder }))
    }

    async fn call(&self, args: Value) -> anyhow::Result<Value> {
        let query = args.get("query").and_then(Value::as_str).ok_or_else(|| anyhow::anyhow!("missing required 'query' argument"))?.trim().to_string();
        anyhow::ensure!(!query.is_empty(), "give a few words, or a question, to look for");
        let words = query_words(&query);
        let limit = args.get("limit").and_then(Value::as_u64).map(|n| (n as usize).clamp(1, MAX_LIMIT)).unwrap_or(DEFAULT_LIMIT);
        let Some(dir) = self.conversations_dir.clone() else {
            anyhow::bail!("there are no saved conversations to search on this channel");
        };
        let embedder = self.embedder;

        tokio::task::spawn_blocking(move || -> anyhow::Result<Value> {
            let conversations = list_conversations(&dir)?;
            let searched = conversations.len();

            // By meaning. Any failure (no network for the first download, no model in this build) leaves only the words.
            let semantic = semantic_ranking(&dir, &conversations, &query, embedder);
            if words.is_empty() && semantic.is_err() {
                anyhow::bail!("give at least one word of {MIN_WORD_LEN} or more letters to look for");
            }

            // By words: the most of the query's words first, then the most recent.
            let mut lexical: Vec<(usize, i64, usize, usize)> = Vec::new();
            for (c, conversation) in conversations.iter().enumerate() {
                for (m, message) in conversation.messages.iter().enumerate() {
                    let lower = message.content.to_lowercase();
                    let score = words.iter().filter(|w| lower.contains(w.as_str())).count();
                    if score > 0 {
                        lexical.push((score, message.created_at, c, m));
                    }
                }
            }
            lexical.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));

            // Merge the two rankings: a message high in either goes up, in both goes higher still.
            let position: HashMap<(&str, &str), (usize, usize)> = conversations
                .iter()
                .enumerate()
                .flat_map(|(c, conversation)| conversation.messages.iter().enumerate().map(move |(m, message)| ((conversation.id.as_str(), message.id.as_str()), (c, m))))
                .collect();
            let mut fused: HashMap<(usize, usize), (f64, bool, bool)> = HashMap::new();
            for (rank, (_, _, c, m)) in lexical.iter().enumerate() {
                let entry = fused.entry((*c, *m)).or_default();
                entry.0 += 1.0 / (RRF_K + rank as f64 + 1.0);
                entry.1 = true;
            }
            if let Ok(found) = &semantic {
                for (rank, (conversation_id, message_id)) in found.ranked.iter().enumerate() {
                    if let Some(&(c, m)) = position.get(&(conversation_id.as_str(), message_id.as_str())) {
                        let entry = fused.entry((c, m)).or_default();
                        entry.0 += 1.0 / (RRF_K + rank as f64 + 1.0);
                        entry.2 = true;
                    }
                }
            }
            let mut ordered: Vec<((usize, usize), (f64, bool, bool))> = fused.into_iter().collect();
            ordered.sort_by(|a, b| {
                b.1 .0.total_cmp(&a.1 .0).then_with(|| conversations[b.0 .0].messages[b.0 .1].created_at.cmp(&conversations[a.0 .0].messages[a.0 .1].created_at))
            });

            let results: Vec<Value> = ordered
                .into_iter()
                .take(limit)
                .map(|((c, m), (_, by_words, by_meaning))| {
                    let (conversation, message) = (&conversations[c], &conversations[c].messages[m]);
                    json!({
                        "conversation_id": conversation.id,
                        "title": conversation.title,
                        "at": message.created_at,
                        "from": match message.role { ChatRole::User => "person", ChatRole::Assistant => "assistant" },
                        "excerpt": snippet(&message.content, &words),
                        "matched": match (by_words, by_meaning) { (true, true) => "words and meaning", (true, false) => "words", _ => "meaning" },
                    })
                })
                .collect();

            let mut answer = json!({ "conversations_searched": searched, "results": results });
            match &semantic {
                Ok(found) => {
                    answer["semantic"] = json!(true);
                    answer["semantic_indexed"] = json!(found.indexed);
                    if found.pending > 0 {
                        answer["semantic_pending"] = json!(found.pending);
                        answer["note"] = json!(format!("{} older messages aren't indexed by meaning yet; searching again indexes more", found.pending));
                    }
                }
                Err(err) => {
                    answer["semantic"] = json!(false);
                    answer["semantic_unavailable"] = json!(format!("{err:#}").chars().take(160).collect::<String>());
                }
            }
            Ok(answer)
        })
        .await?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{save_conversation, Conversation, ConversationMessage};

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("warden-history-{name}-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn message(role: ChatRole, content: &str, at: i64) -> ConversationMessage {
        ConversationMessage { id: format!("m{at}"), role, content: content.into(), created_at: at, usage: None, attachments: Vec::new(), generated_files: Vec::new(), tools_used: Vec::new() }
    }

    fn conversation(id: &str, title: &str, messages: Vec<ConversationMessage>) -> Conversation {
        let updated_at = messages.last().map(|m| m.created_at).unwrap_or(0);
        Conversation { id: id.into(), title: title.into(), messages, created_at: 1, updated_at, agent_id: None, provider_id: None, project_id: None, engine_session_id: None }
    }

    fn saved(dir: &Path) {
        save_conversation(
            dir,
            &conversation(
                "viagem",
                "Viagem a Lisboa",
                vec![
                    message(ChatRole::User, "Quero reservar um hotel em Lisboa perto do rio", 100),
                    message(ChatRole::Assistant, "Sugiro o bairro de Alfama, com hotel pequeno e vista para o rio Tejo", 200),
                ],
            ),
        )
        .unwrap();
        save_conversation(dir, &conversation("receita", "Bolo", vec![message(ChatRole::User, "Receita de bolo de cenoura com cobertura", 300)])).unwrap();
    }

    async fn search(tool: &SearchHistoryTool, query: &str) -> Value {
        tool.call(json!({ "query": query })).await.unwrap()
    }

    #[tokio::test]
    async fn finds_what_was_said_ranked_by_how_many_words_match() {
        let dir = temp_dir("rank");
        saved(&dir);
        let tool = SearchHistoryTool::new(Some(dir.clone())).with_embedder(no_embedder);

        let found = search(&tool, "hotel Lisboa rio").await;
        assert_eq!(found["conversations_searched"], 2);
        let results = found["results"].as_array().unwrap();
        assert_eq!(results.len(), 2, "only the two messages of the trip mention them: {found}");
        assert_eq!(results[0]["conversation_id"], "viagem");
        assert_eq!((results[0]["from"].as_str(), results[0]["title"].as_str()), (Some("person"), Some("Viagem a Lisboa")), "three of the words beat two");
        assert_eq!(results[1]["from"], "assistant");
        assert!(results[0]["excerpt"].as_str().unwrap().contains("hotel em Lisboa"));

        let cake = search(&tool, "cenoura").await;
        assert_eq!(cake["results"][0]["conversation_id"], "receita");
        assert!(search(&tool, "astronomia").await["results"].as_array().unwrap().is_empty());
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn the_limit_caps_the_answer_and_short_words_alone_are_refused() {
        let dir = temp_dir("limit");
        let messages = (0..30).map(|i| message(ChatRole::User, &format!("nota numero {i} sobre o projeto"), i)).collect();
        save_conversation(&dir, &conversation("c", "Projeto", messages)).unwrap();
        let tool = SearchHistoryTool::new(Some(dir.clone())).with_embedder(no_embedder);

        assert_eq!(search(&tool, "projeto").await["results"].as_array().unwrap().len(), DEFAULT_LIMIT);
        let five = tool.call(json!({ "query": "projeto", "limit": 5 })).await.unwrap();
        assert_eq!(five["results"].as_array().unwrap().len(), 5);
        assert_eq!(five["results"][0]["at"], 29, "the most recent first among equals");
        let huge = tool.call(json!({ "query": "projeto", "limit": 500 })).await.unwrap();
        assert_eq!(huge["results"].as_array().unwrap().len(), MAX_LIMIT);

        let err = tool.call(json!({ "query": "a é ok" })).await.unwrap_err().to_string();
        assert!(err.contains("at least one word"), "{err}");
        std::fs::remove_dir_all(dir).ok();
    }

    /// The folder a tool reads is only what it was built or re-pointed with — never a neighbour's.
    #[tokio::test]
    async fn it_reads_only_the_folder_it_was_given() {
        let (ana, bruno) = (temp_dir("ana"), temp_dir("bruno"));
        save_conversation(&ana, &conversation("a1", "Segredo da Ana", vec![message(ChatRole::User, "minha senha do cofre é tulipa", 1)])).unwrap();
        save_conversation(&bruno, &conversation("b1", "Conversa do Bruno", vec![message(ChatRole::User, "falei de futebol ontem", 1)])).unwrap();

        let base = SearchHistoryTool::new(None).with_embedder(no_embedder);
        assert!(base.call(json!({ "query": "tulipa" })).await.unwrap_err().to_string().contains("no saved conversations"));

        let for_bruno = base.with_conversations_dir(&bruno).unwrap();
        assert!(for_bruno.call(json!({ "query": "tulipa" })).await.unwrap()["results"].as_array().unwrap().is_empty(), "Bruno's agent doesn't see Ana's");
        assert_eq!(for_bruno.call(json!({ "query": "futebol" })).await.unwrap()["results"][0]["conversation_id"], "b1");
        let for_ana = for_bruno.with_conversations_dir(&ana).unwrap();
        assert_eq!(for_ana.call(json!({ "query": "tulipa" })).await.unwrap()["results"][0]["conversation_id"], "a1");
        std::fs::remove_dir_all(ana).ok();
        std::fs::remove_dir_all(bruno).ok();
    }

    /// No embedding model: the search is only the words, as it was before meaning came in.
    fn no_embedder(_kind: Kind, _texts: Vec<String>) -> anyhow::Result<Vec<Vec<f32>>> {
        anyhow::bail!("no embedding model in this test")
    }

    /// A tiny stand-in for the model: each text becomes counts of a few concepts, so words that mean the same thing
    /// (`smtp`, `correio`) land on the same axis and a text about none of them is the zero vector.
    fn concept_embedder(_kind: Kind, texts: Vec<String>) -> anyhow::Result<Vec<Vec<f32>>> {
        const CONCEPTS: [&[&str]; 4] = [&["smtp", "e-mail", "email", "correio"], &["hotel", "hospedar", "hospedagem", "pousada"], &["bolo", "sobremesa", "cenoura"], &["futebol", "jogo", "gol"]];
        Ok(texts.iter().map(|t| CONCEPTS.iter().map(|words| words.iter().filter(|w| t.to_lowercase().contains(*w)).count() as f32).collect()).collect())
    }

    /// Like `concept_embedder` for a question, but refuses to embed any message: proves the index already has them.
    fn only_queries(kind: Kind, texts: Vec<String>) -> anyhow::Result<Vec<Vec<f32>>> {
        anyhow::ensure!(kind == Kind::Query, "a message was embedded again");
        concept_embedder(kind, texts)
    }

    fn mail_folder(name: &str) -> PathBuf {
        let dir = temp_dir(name);
        save_conversation(&dir, &conversation("mail", "Servidor", vec![message(ChatRole::Assistant, "Configurei o SMTP do servidor na porta 587 com STARTTLS", 100)])).unwrap();
        save_conversation(&dir, &conversation("bolo", "Bolo", vec![message(ChatRole::User, "Receita de bolo de cenoura com cobertura de chocolate", 200)])).unwrap();
        dir
    }

    #[tokio::test]
    async fn finds_by_meaning_what_shares_no_word_with_the_question() {
        let dir = mail_folder("meaning");
        let tool = SearchHistoryTool::new(Some(dir.clone())).with_embedder(concept_embedder);

        let found = search(&tool, "como ficou o correio de saída").await;
        assert_eq!(found["semantic"], true);
        let results = found["results"].as_array().unwrap();
        assert_eq!(results.len(), 1, "{found}");
        assert_eq!((results[0]["conversation_id"].as_str(), results[0]["matched"].as_str()), (Some("mail"), Some("meaning")));

        // The same question by words alone finds nothing.
        let words_only = SearchHistoryTool::new(Some(dir.clone())).with_embedder(no_embedder);
        assert!(search(&words_only, "como ficou o correio de saída").await["results"].as_array().unwrap().is_empty());
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn a_message_that_matches_by_words_and_by_meaning_comes_first() {
        let dir = temp_dir("both");
        save_conversation(&dir, &conversation("a", "Antiga", vec![message(ChatRole::User, "Preciso configurar o correio na empresa nova", 100)])).unwrap();
        save_conversation(&dir, &conversation("b", "Recente", vec![message(ChatRole::User, "O SMTP do correio da empresa já está funcionando", 200)])).unwrap();
        let tool = SearchHistoryTool::new(Some(dir.clone())).with_embedder(concept_embedder);

        let found = search(&tool, "correio empresa smtp").await;
        let results = found["results"].as_array().unwrap();
        assert_eq!(results.len(), 2, "{found}");
        assert_eq!((results[0]["conversation_id"].as_str(), results[0]["matched"].as_str()), (Some("b"), Some("words and meaning")), "{found}");
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn the_index_is_kept_next_to_the_conversations_holds_no_text_and_is_not_rebuilt() {
        let dir = mail_folder("index");
        SearchHistoryTool::new(Some(dir.clone())).with_embedder(concept_embedder).call(json!({ "query": "correio" })).await.unwrap();

        let raw = std::fs::read_to_string(dir.join(INDEX_FILE)).unwrap();
        assert!(raw.contains("embedding") && !raw.contains("SMTP") && !raw.contains("cenoura"), "no message text in the index");
        assert_eq!(list_conversations(&dir).unwrap().len(), 2, "the index isn't taken for a conversation");

        // Nothing new: a model that refuses to embed messages still answers by meaning.
        let again = SearchHistoryTool::new(Some(dir.clone())).with_embedder(only_queries).call(json!({ "query": "correio" })).await.unwrap();
        assert_eq!((again["semantic"].as_bool(), again["results"][0]["conversation_id"].as_str()), (Some(true), Some("mail")));

        // A new message needs an embedding the refusing model can't give: the answer falls back to the words.
        save_conversation(&dir, &conversation("novo", "Novo", vec![message(ChatRole::User, "Mais uma mensagem sobre o correio da empresa", 300)])).unwrap();
        let fallback = SearchHistoryTool::new(Some(dir.clone())).with_embedder(only_queries).call(json!({ "query": "correio" })).await.unwrap();
        assert_eq!(fallback["semantic"], false);
        assert!(fallback["results"][0]["conversation_id"] == "novo", "the words still find it: {fallback}");
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn a_big_folder_is_indexed_a_few_hundred_messages_at_a_time() {
        let dir = temp_dir("big");
        let messages = (0..(MAX_NEW_EMBEDDINGS as i64 + 20)).map(|i| message(ChatRole::User, &format!("mensagem comprida sobre o correio numero {i}"), i)).collect();
        save_conversation(&dir, &conversation("c", "Muitas", messages)).unwrap();
        let tool = SearchHistoryTool::new(Some(dir.clone())).with_embedder(concept_embedder);

        let first = search(&tool, "correio").await;
        assert_eq!((first["semantic_indexed"].as_u64(), first["semantic_pending"].as_u64()), (Some(MAX_NEW_EMBEDDINGS as u64), Some(20)), "{first}");
        let second = search(&tool, "correio").await;
        assert_eq!((second["semantic_indexed"].as_u64(), second["semantic_pending"].as_u64()), (Some(MAX_NEW_EMBEDDINGS as u64 + 20), None), "{second}");
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn without_a_model_short_words_are_refused_and_the_answer_says_meaning_was_unavailable() {
        let dir = mail_folder("nomodel");
        let tool = SearchHistoryTool::new(Some(dir.clone())).with_embedder(no_embedder);
        let found = search(&tool, "SMTP").await;
        assert_eq!(found["semantic"], false);
        assert!(found["semantic_unavailable"].as_str().unwrap().contains("no embedding model"), "{found}");
        assert_eq!(found["results"][0]["conversation_id"], "mail");

        // With a model, a question of only short words is a fine question.
        let with_model = SearchHistoryTool::new(Some(dir.clone())).with_embedder(concept_embedder);
        assert!(with_model.call(json!({ "query": "e-mail" })).await.is_ok());
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn a_members_index_is_sealed_with_their_key_and_unreadable_without_it() {
        let dir = temp_dir("sealed");
        std::fs::write(dir.join(crate::member_crypto::MARKER), b"").unwrap();
        crate::member_crypto::unlock(&[dir.as_path()], &crate::member_crypto::new_key());
        save_conversation(&dir, &conversation("mail", "Servidor", vec![message(ChatRole::Assistant, "Configurei o SMTP do servidor na porta 587 com STARTTLS", 100)])).unwrap();
        let tool = SearchHistoryTool::new(Some(dir.clone())).with_embedder(concept_embedder);

        assert_eq!(search(&tool, "correio").await["results"][0]["conversation_id"], "mail");
        let on_disk = std::fs::read(dir.join(INDEX_FILE)).unwrap();
        assert!(warden_core::memory::VaultCipher::is_sealed(&on_disk), "sealed like the conversations");
        assert!(!String::from_utf8_lossy(&on_disk).contains("embedding"));

        // Once the hub forgets the key, the folder is locked and so is the search.
        crate::member_crypto::lock(&[dir.as_path()]);
        assert!(tool.call(json!({ "query": "correio" })).await.is_err());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_snippet_is_one_line_around_the_match_and_survives_accents() {
        let text = format!("{} ação importante: decidimos usar o método B\n\nnovo parágrafo {}", "x".repeat(400), "y".repeat(400));
        let cut = snippet(&text, &["ação".to_string()]);
        assert!(cut.starts_with('…') && cut.ends_with('…') && cut.contains("ação importante") && !cut.contains('\n'), "{cut}");
        assert!(cut.chars().count() <= SNIPPET_CHARS + 2);
        assert_eq!(snippet("curto", &["curto".to_string()]), "curto");
    }
}
