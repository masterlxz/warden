//! Skills (P16) — reusable instruction packages the model loads on demand, in the spirit of
//! Claude's Skills. A skill is one markdown file, `skills/<name>.md` in the vault, with a small
//! frontmatter block:
//!
//! ```text
//! ---
//! name: review-pr
//! description: How to review a pull request (when to use it)
//! agents: writer, reviewer
//! ---
//! <the instructions, free-form markdown>
//! ```
//!
//! `agents` is optional (P72 c): a comma-separated list of agent ids the skill is restricted to.
//! Absent/empty means global — every agent sees it. With no active agent (Telegram, WhatsApp, the
//! server, mobile) only the global skills are visible.
//!
//! Each turn the orchestrator only shows the model the catalog (name + description); the body is
//! fetched through the `use_skill` tool when the model decides it applies, so unused skills cost
//! no tokens. Living in the vault means sync (`warden-sync`, git) and Obsidian editing come free.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, bail, Context};

use crate::memory::{Vault, SKILLS_DIR};

pub const MAX_NAME_LEN: usize = 64;
pub const MAX_DESCRIPTION_LEN: usize = 300;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub body: String,
    /// Agent ids this skill is restricted to; empty = available to everyone.
    pub agents: Vec<String>,
}

/// A skill name doubles as its filename, so it must be a plain slug — `Vault::write` does no
/// path-traversal check of its own, this is what keeps `../x` (or `a/b`) out of the vault.
pub fn validate_name(name: &str) -> anyhow::Result<()> {
    if name.is_empty() || name.len() > MAX_NAME_LEN {
        bail!("skill name must be 1-{MAX_NAME_LEN} characters");
    }
    if !name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
        bail!("skill name '{name}' must use only lowercase letters, digits and hyphens");
    }
    if name.starts_with('-') || name.ends_with('-') {
        bail!("skill name '{name}' must not start or end with a hyphen");
    }
    Ok(())
}

impl Skill {
    pub fn validate(&self) -> anyhow::Result<()> {
        validate_name(&self.name)?;
        if self.description.trim().is_empty() {
            bail!("skill description must not be empty");
        }
        if self.description.chars().count() > MAX_DESCRIPTION_LEN {
            bail!("skill description must be at most {MAX_DESCRIPTION_LEN} characters");
        }
        if self.body.trim().is_empty() {
            bail!("skill body must not be empty");
        }
        // The list is stored on one comma-separated frontmatter line, so an id with a comma or a
        // newline (or an empty one) wouldn't survive the round trip.
        for agent in &self.agents {
            if agent.trim().is_empty() || agent.contains(',') || agent.contains('\n') {
                bail!("invalid agent id '{agent}' in skill agents (empty, or contains a comma/newline)");
            }
        }
        Ok(())
    }

    /// Whether `agent` (the turn's active agent, `None` when there isn't one) may see this skill.
    pub fn is_available_to(&self, agent: Option<&str>) -> bool {
        self.agents.is_empty() || agent.is_some_and(|id| self.agents.iter().any(|a| a == id))
    }

    /// Serializes to the on-disk format. The description is collapsed to a single line, since the
    /// frontmatter is line-oriented.
    pub fn render(&self) -> String {
        let description = self.description.split_whitespace().collect::<Vec<_>>().join(" ");
        let agents = if self.agents.is_empty() { String::new() } else { format!("agents: {}\n", self.agents.join(", ")) };
        format!("---\nname: {}\ndescription: {}\n{}---\n{}\n", self.name, description, agents, self.body.trim())
    }

    /// Parses a skill file. `name` comes from the filename (the source of truth — a file renamed in
    /// Obsidian keeps working); frontmatter is optional, so a plain markdown file is a skill with
    /// an empty description and the whole file as its body.
    pub fn parse(name: &str, raw: &str) -> Skill {
        let raw = raw.replace("\r\n", "\n");
        let mut description = String::new();
        let mut agents: Vec<String> = Vec::new();
        let mut body = raw.as_str();

        if let Some(rest) = raw.strip_prefix("---\n") {
            if let Some(end) = rest.find("\n---") {
                let (front, after) = rest.split_at(end);
                for line in front.lines() {
                    if let Some((key, value)) = line.split_once(':') {
                        match key.trim() {
                            "description" => description = value.trim().to_string(),
                            "agents" => {
                                for id in value.split(',').map(str::trim).filter(|id| !id.is_empty()) {
                                    if !agents.iter().any(|a| a == id) {
                                        agents.push(id.to_string());
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
                body = after["\n---".len()..].trim_start_matches('\n');
            }
        }

        Skill { name: name.to_string(), description, body: body.trim().to_string(), agents }
    }
}

/// Reads and writes skills under `skills/` in a vault.
#[derive(Clone)]
pub struct SkillStore {
    vault: Arc<Vault>,
}

impl SkillStore {
    pub fn new(vault: Arc<Vault>) -> Self {
        Self { vault }
    }

    fn relative_path(name: &str) -> anyhow::Result<String> {
        validate_name(name)?;
        Ok(format!("{SKILLS_DIR}/{name}.md"))
    }

    /// Every skill, sorted by name. Unreadable files are skipped — one bad file must not hide the rest.
    pub fn list(&self) -> Vec<Skill> {
        let dir = self.vault.root().join(SKILLS_DIR);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return Vec::new();
        };
        let mut skills: Vec<Skill> = entries
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                let name = skill_name_from_path(&path)?;
                validate_name(&name).ok()?;
                let raw = std::fs::read_to_string(&path).ok()?;
                Some(Skill::parse(&name, &raw))
            })
            .collect();
        skills.sort_by(|a, b| a.name.cmp(&b.name));
        skills
    }

    pub fn get(&self, name: &str) -> anyhow::Result<Skill> {
        let path = Self::relative_path(name)?;
        let raw = self.vault.read(&path).map_err(|_| anyhow!("no skill named '{name}'"))?;
        Ok(Skill::parse(name, &raw))
    }

    /// Like `get`, but a skill restricted to other agents reads as nonexistent — same error, so the
    /// model can't tell "exists but not yours" from "no such skill".
    pub fn get_for(&self, name: &str, agent: Option<&str>) -> anyhow::Result<Skill> {
        let skill = self.get(name)?;
        if skill.is_available_to(agent) {
            Ok(skill)
        } else {
            Err(anyhow!("no skill named '{name}'"))
        }
    }

    /// Absolute path of a skill's file (the skill needn't exist yet) — for pointing the user at the
    /// file so they can edit a long body in their own editor or Obsidian.
    pub fn path_of(&self, name: &str) -> anyhow::Result<PathBuf> {
        Ok(self.vault.root().join(Self::relative_path(name)?))
    }

    pub fn exists(&self, name: &str) -> bool {
        Self::relative_path(name).is_ok_and(|p| self.vault.root().join(p).is_file())
    }

    pub fn save(&self, skill: &Skill) -> anyhow::Result<()> {
        skill.validate()?;
        let path = Self::relative_path(&skill.name)?;
        self.vault.write(&path, &skill.render()).with_context(|| format!("failed to write skill '{}'", skill.name))
    }

    pub fn delete(&self, name: &str) -> anyhow::Result<()> {
        let path = Self::relative_path(name)?;
        self.vault.delete(&path).map_err(|_| anyhow!("no skill named '{name}'"))
    }

    /// The per-turn catalog injected as a system message, or `None` when there are no skills
    /// visible to `agent` (the turn's active agent, `None` for turns without one).
    pub fn catalog(&self, agent: Option<&str>) -> Option<String> {
        let skills: Vec<Skill> = self.list().into_iter().filter(|s| s.is_available_to(agent)).collect();
        if skills.is_empty() {
            return None;
        }
        let lines: Vec<String> = skills
            .iter()
            .map(|s| {
                if s.description.is_empty() {
                    format!("- {}", s.name)
                } else {
                    format!("- {}: {}", s.name, s.description)
                }
            })
            .collect();
        Some(format!(
            "Available skills (reusable instructions). When one applies to the user's request, call the \
             `use_skill` tool with its name to load the full instructions before answering:\n{}",
            lines.join("\n")
        ))
    }
}

fn skill_name_from_path(path: &Path) -> Option<String> {
    if path.extension().and_then(|e| e.to_str()) != Some("md") {
        return None;
    }
    path.file_stem().and_then(|s| s.to_str()).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store() -> SkillStore {
        SkillStore::new(Arc::new(Vault::new(std::env::temp_dir().join(format!(
            "warden-skill-test-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        )))))
    }

    fn sample(name: &str) -> Skill {
        Skill { name: name.into(), description: "Reviews a PR".into(), body: "Step 1.\nStep 2.".into(), agents: Vec::new() }
    }

    #[test]
    fn validate_name_accepts_slugs_and_rejects_everything_else() {
        for ok in ["a", "review-pr", "v2", "a-b-c"] {
            assert!(validate_name(ok).is_ok(), "{ok}");
        }
        for bad in ["", "Review", "a b", "../x", "a/b", "a.md", "-a", "a-", "é", &"x".repeat(65)] {
            assert!(validate_name(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn path_of_points_into_the_skills_dir_and_rejects_bad_names() {
        let store = temp_store();
        let path = store.path_of("review-pr").unwrap();
        assert!(path.ends_with("skills/review-pr.md"));
        assert!(store.path_of("../x").is_err());
    }

    #[test]
    fn render_then_parse_roundtrips() {
        let skill = sample("review-pr");
        assert_eq!(Skill::parse("review-pr", &skill.render()), skill);
    }

    #[test]
    fn render_collapses_multiline_description() {
        let skill = Skill { description: "line one\nline two".into(), ..sample("x") };
        assert_eq!(Skill::parse("x", &skill.render()).description, "line one line two");
    }

    #[test]
    fn parse_without_frontmatter_uses_whole_file_as_body() {
        let skill = Skill::parse("plain", "just some notes\nmore");
        assert_eq!(skill.description, "");
        assert_eq!(skill.body, "just some notes\nmore");
    }

    #[test]
    fn parse_handles_crlf_and_keeps_body_dashes() {
        let raw = "---\r\nname: x\r\ndescription: d\r\n---\r\nbody\r\n---\r\nstill body";
        let skill = Skill::parse("x", raw);
        assert_eq!(skill.description, "d");
        assert_eq!(skill.body, "body\n---\nstill body");
    }

    #[test]
    fn save_get_list_delete() {
        let store = temp_store();
        assert!(store.list().is_empty());

        store.save(&sample("b-skill")).unwrap();
        store.save(&sample("a-skill")).unwrap();
        assert!(store.exists("a-skill"));
        assert_eq!(store.get("a-skill").unwrap(), sample("a-skill"));

        let names: Vec<String> = store.list().into_iter().map(|s| s.name).collect();
        assert_eq!(names, vec!["a-skill", "b-skill"]);

        store.delete("a-skill").unwrap();
        assert!(!store.exists("a-skill"));
        assert!(store.get("a-skill").is_err());
        assert!(store.delete("a-skill").is_err());
    }

    #[test]
    fn save_rejects_traversal_and_incomplete_skills() {
        let store = temp_store();
        assert!(store.save(&sample("../escape")).is_err());
        assert!(store.save(&Skill { description: "  ".into(), ..sample("ok") }).is_err());
        assert!(store.save(&Skill { body: "".into(), ..sample("ok") }).is_err());
        assert!(store.get("../escape").is_err());
        assert!(store.list().is_empty());
    }

    #[test]
    fn list_skips_non_markdown_and_badly_named_files() {
        let store = temp_store();
        store.save(&sample("good")).unwrap();
        store.vault.write("skills/notes.txt", "ignored").unwrap();
        store.vault.write("skills/Bad Name.md", "ignored").unwrap();

        let names: Vec<String> = store.list().into_iter().map(|s| s.name).collect();
        assert_eq!(names, vec!["good"]);
    }

    #[test]
    fn catalog_is_none_without_skills_and_lists_them_otherwise() {
        let store = temp_store();
        assert!(store.catalog(None).is_none());

        store.save(&sample("review-pr")).unwrap();
        let catalog = store.catalog(None).unwrap();
        assert!(catalog.contains("- review-pr: Reviews a PR"));
        assert!(catalog.contains("use_skill"));
    }

    fn restricted(name: &str, agents: &[&str]) -> Skill {
        Skill { agents: agents.iter().map(|a| a.to_string()).collect(), ..sample(name) }
    }

    #[test]
    fn agents_roundtrip_and_stay_out_of_the_file_when_empty() {
        let skill = restricted("review-pr", &["writer", "reviewer"]);
        assert!(skill.render().contains("agents: writer, reviewer\n"));
        assert_eq!(Skill::parse("review-pr", &skill.render()), skill);
        assert!(!sample("x").render().contains("agents"));
    }

    #[test]
    fn parse_agents_trims_dedups_and_drops_empties() {
        let skill = Skill::parse("x", "---\ndescription: d\nagents:  a , b,, a \n---\nbody");
        assert_eq!(skill.agents, vec!["a", "b"]);
    }

    #[test]
    fn validate_rejects_unstorable_agent_ids() {
        for bad in ["", "  ", "a,b", "a\nb"] {
            assert!(restricted("x", &[bad]).validate().is_err(), "{bad:?}");
        }
        assert!(restricted("x", &["writer"]).validate().is_ok());
    }

    #[test]
    fn availability_follows_the_agent_list() {
        let global = sample("g");
        let only_writer = restricted("w", &["writer"]);
        assert!(global.is_available_to(None) && global.is_available_to(Some("writer")));
        assert!(only_writer.is_available_to(Some("writer")));
        assert!(!only_writer.is_available_to(Some("reviewer")));
        assert!(!only_writer.is_available_to(None));
    }

    #[test]
    fn catalog_and_get_for_hide_skills_restricted_to_other_agents() {
        let store = temp_store();
        store.save(&sample("global")).unwrap();
        store.save(&restricted("only-writer", &["writer"])).unwrap();

        let none = store.catalog(None).unwrap();
        assert!(none.contains("global") && !none.contains("only-writer"));
        let writer = store.catalog(Some("writer")).unwrap();
        assert!(writer.contains("global") && writer.contains("only-writer"));
        let other = store.catalog(Some("reviewer")).unwrap();
        assert!(!other.contains("only-writer"));

        assert!(store.get_for("only-writer", Some("writer")).is_ok());
        let err = store.get_for("only-writer", Some("reviewer")).unwrap_err();
        assert!(err.to_string().contains("no skill named"));
    }

    #[test]
    fn catalog_is_none_when_every_skill_is_restricted_away() {
        let store = temp_store();
        store.save(&restricted("only-writer", &["writer"])).unwrap();
        assert!(store.catalog(None).is_none());
    }
}
