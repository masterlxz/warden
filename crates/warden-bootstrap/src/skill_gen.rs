//! "Describe a skill and the AI writes it" (P16) — one model call that turns a free-text
//! description into a draft `Skill`. Returns a draft, never saves: the desktop's Skills screen
//! fills its form with it so the user reviews (and can edit) before anything reaches the vault.

use serde::Deserialize;
use warden_core::model::{Message, ModelProvider};
use warden_core::skill::Skill;

const SYSTEM_PROMPT: &str = "You write reusable skills for an AI assistant. A skill is a saved set of \
instructions the assistant loads on demand when a user request matches it.\n\
Given the user's description of what the skill should do, reply with ONE JSON object and nothing else:\n\
{\"name\": \"...\", \"description\": \"...\", \"body\": \"...\"}\n\
- name: a short slug using only lowercase letters, digits and hyphens (e.g. \"review-pr\").\n\
- description: ONE sentence saying what the skill does and when to use it (max 300 characters).\n\
- body: the full instructions, in markdown, written as direct guidance to the assistant (steps, rules, \
output format). Be concrete and complete, but do not pad.\n\
Write description and body in the same language as the user's description.";

#[derive(Deserialize)]
struct RawDraft {
    name: String,
    description: String,
    body: String,
}

pub async fn generate_skill_draft(model: &dyn ModelProvider, request: &str) -> anyhow::Result<Skill> {
    if request.trim().is_empty() {
        anyhow::bail!("describe the skill you want first");
    }
    let response =
        model.chat(vec![Message::system(SYSTEM_PROMPT), Message::user(request.trim().to_string())], Vec::new()).await?;
    parse_draft(&response.content)
}

/// Tolerant of what models actually do around a JSON answer: a ```json fence, or a sentence of
/// preamble. Takes the outermost `{...}`, then slugifies the name (a model saying "Review PR"
/// is fixed, not rejected) and runs the same validation a hand-written skill goes through.
fn parse_draft(content: &str) -> anyhow::Result<Skill> {
    let (start, end) = match (content.find('{'), content.rfind('}')) {
        (Some(s), Some(e)) if s < e => (s, e),
        _ => anyhow::bail!("the model did not return a skill draft — try rephrasing"),
    };
    let raw: RawDraft = serde_json::from_str(&content[start..=end])
        .map_err(|e| anyhow::anyhow!("the model returned an invalid skill draft ({e}) — try again"))?;

    let skill = Skill { name: slugify(&raw.name), description: raw.description.trim().to_string(), body: raw.body.trim().to_string(), agents: Vec::new() };
    skill.validate()?;
    Ok(skill)
}

fn slugify(name: &str) -> String {
    let mut slug = String::new();
    for c in name.trim().to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            slug.push(c);
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    slug.trim_matches('-').chars().take(warden_core::skill::MAX_NAME_LEN).collect::<String>().trim_end_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use warden_core::model::{ChatStream, Response, response_stream};
    use warden_core::tool::ToolSpec;

    struct FixedReply(&'static str);

    #[async_trait]
    impl ModelProvider for FixedReply {
        async fn chat_stream(&self, _messages: Vec<Message>, _tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
            Ok(response_stream(Response { content: self.0.to_string(), tool_calls: Vec::new(), usage: None }))
        }
    }

    const PLAIN: &str = r#"{"name":"review-pr","description":"Reviews a PR.","body":"Step 1."}"#;

    #[test]
    fn parses_plain_json_fenced_json_and_json_with_preamble() {
        for reply in [PLAIN.to_string(), format!("```json\n{PLAIN}\n```"), format!("Sure! Here it is:\n{PLAIN}\nEnjoy.")] {
            let skill = parse_draft(&reply).unwrap();
            assert_eq!(skill.name, "review-pr");
            assert_eq!(skill.description, "Reviews a PR.");
            assert_eq!(skill.body, "Step 1.");
        }
    }

    #[test]
    fn slugifies_a_sloppy_name_instead_of_rejecting_it() {
        let skill = parse_draft(r#"{"name":" Review  PR! ","description":"d","body":"b"}"#).unwrap();
        assert_eq!(skill.name, "review-pr");
    }

    #[test]
    fn rejects_garbage_missing_fields_and_unusable_names() {
        assert!(parse_draft("no json here").is_err());
        assert!(parse_draft(r#"{"name":"x"}"#).is_err());
        assert!(parse_draft(r#"{"name":"!!!","description":"d","body":"b"}"#).is_err());
        assert!(parse_draft(r#"{"name":"x","description":"","body":"b"}"#).is_err());
        assert!(parse_draft(r#"{"name":"x","description":"d","body":"  "}"#).is_err());
    }

    #[tokio::test]
    async fn generate_skill_draft_runs_the_model_and_rejects_an_empty_request() {
        let skill = generate_skill_draft(&FixedReply(PLAIN), "a skill for reviewing PRs").await.unwrap();
        assert_eq!(skill.name, "review-pr");

        assert!(generate_skill_draft(&FixedReply(PLAIN), "   ").await.is_err());
    }
}
