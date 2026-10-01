//! `search_history` (P104): the agent looks back through the conversations saved for whoever it is speaking
//! with — what they said and what was answered, weeks ago. The Hermes agent does the same with a full-text index
//! over its sessions; here the conversations are already files, so this reads them (decrypting a member's, which
//! fails as "locked" when the hub doesn't hold their key) and scores the messages by how many of the query's
//! words they contain.
//!
//! The folder is the tool's own and the hub re-points it for every turn (`Tool::with_conversations_dir`): a person's
//! agent can only ever search that person's conversations.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use warden_core::tool::{Tool, ToolSpec};

use crate::{list_conversations, ChatRole};

const DEFAULT_LIMIT: usize = 8;
const MAX_LIMIT: usize = 20;
const MIN_WORD_LEN: usize = 3;
/// Characters of a message shown around the first match.
const SNIPPET_CHARS: usize = 240;

pub struct SearchHistoryTool {
    conversations_dir: Option<PathBuf>,
}

impl SearchHistoryTool {
    pub fn new(conversations_dir: Option<PathBuf>) -> Self {
        Self { conversations_dir }
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

#[async_trait]
impl Tool for SearchHistoryTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "search_history".to_string(),
            description: "Search the earlier conversations saved for the person you're talking with: what they said and what was answered, in this and other conversations. Use it when they refer to something from before (\"like last time\", \"what did we decide about…\"). Give a few distinctive words; each result names the conversation, the day and who said it, with a short excerpt.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "The words to look for (3+ letters each); a message matches the more of them it contains" },
                    "limit": { "type": "integer", "description": "How many excerpts to return, 1-20 (default 8)" }
                },
                "required": ["query"]
            }),
        }
    }

    fn with_conversations_dir(&self, dir: &Path) -> Option<Arc<dyn Tool>> {
        Some(Arc::new(Self::new(Some(dir.to_path_buf()))))
    }

    async fn call(&self, args: Value) -> anyhow::Result<Value> {
        let query = args.get("query").and_then(Value::as_str).ok_or_else(|| anyhow::anyhow!("missing required 'query' argument"))?;
        let words = query_words(query);
        anyhow::ensure!(!words.is_empty(), "give at least one word of {MIN_WORD_LEN} or more letters to look for");
        let limit = args.get("limit").and_then(Value::as_u64).map(|n| (n as usize).clamp(1, MAX_LIMIT)).unwrap_or(DEFAULT_LIMIT);
        let Some(dir) = self.conversations_dir.clone() else {
            anyhow::bail!("there are no saved conversations to search on this channel");
        };

        let (results, searched) = tokio::task::spawn_blocking(move || -> anyhow::Result<(Vec<(usize, i64, Value)>, usize)> {
            let conversations = list_conversations(&dir)?;
            let searched = conversations.len();
            let mut hits = Vec::new();
            for conversation in &conversations {
                for message in &conversation.messages {
                    let lower = message.content.to_lowercase();
                    let score = words.iter().filter(|w| lower.contains(w.as_str())).count();
                    if score == 0 {
                        continue;
                    }
                    hits.push((
                        score,
                        message.created_at,
                        json!({
                            "conversation_id": conversation.id,
                            "title": conversation.title,
                            "at": message.created_at,
                            "from": match message.role { ChatRole::User => "person", ChatRole::Assistant => "assistant" },
                            "excerpt": snippet(&message.content, &words),
                        }),
                    ));
                }
            }
            // Most of the query's words first, then the most recent.
            hits.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
            Ok((hits, searched))
        })
        .await??;

        let shown: Vec<Value> = results.into_iter().take(limit).map(|(_, _, hit)| hit).collect();
        Ok(json!({ "conversations_searched": searched, "results": shown }))
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
        ConversationMessage { id: format!("m{at}"), role, content: content.into(), created_at: at, usage: None, attachments: Vec::new(), generated_files: Vec::new() }
    }

    fn conversation(id: &str, title: &str, messages: Vec<ConversationMessage>) -> Conversation {
        let updated_at = messages.last().map(|m| m.created_at).unwrap_or(0);
        Conversation { id: id.into(), title: title.into(), messages, created_at: 1, updated_at, agent_id: None, provider_id: None }
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
        let tool = SearchHistoryTool::new(Some(dir.clone()));

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
        let tool = SearchHistoryTool::new(Some(dir.clone()));

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

        let base = SearchHistoryTool::new(None);
        assert!(base.call(json!({ "query": "tulipa" })).await.unwrap_err().to_string().contains("no saved conversations"));

        let for_bruno = base.with_conversations_dir(&bruno).unwrap();
        assert!(for_bruno.call(json!({ "query": "tulipa" })).await.unwrap()["results"].as_array().unwrap().is_empty(), "Bruno's agent doesn't see Ana's");
        assert_eq!(for_bruno.call(json!({ "query": "futebol" })).await.unwrap()["results"][0]["conversation_id"], "b1");
        let for_ana = for_bruno.with_conversations_dir(&ana).unwrap();
        assert_eq!(for_ana.call(json!({ "query": "tulipa" })).await.unwrap()["results"][0]["conversation_id"], "a1");
        std::fs::remove_dir_all(ana).ok();
        std::fs::remove_dir_all(bruno).ok();
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
