use async_trait::async_trait;
use serde_json::{json, Value};

use crate::skill::{Skill, SkillStore};
use crate::tool::{Tool, ToolSpec};

fn required_str<'a>(args: &'a Value, key: &str) -> anyhow::Result<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("missing required '{key}' argument"))
}

/// Loads a skill's full instructions. The per-turn catalog (`SkillStore::catalog`) only carries
/// name + description; this is how the model pulls in the body when a skill applies (P16).
pub struct UseSkillTool {
    store: SkillStore,
    /// The agent this tool serves (P72 c) — a skill restricted to other agents loads as "no such
    /// skill". `None` (no agent) only reaches the global skills.
    agent: Option<String>,
}

impl UseSkillTool {
    pub fn new(store: SkillStore) -> Self {
        Self { store, agent: None }
    }

    pub fn for_agent(mut self, agent: Option<String>) -> Self {
        self.agent = agent;
        self
    }
}

#[async_trait]
impl Tool for UseSkillTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "use_skill".to_string(),
            description: "Load the full instructions of a skill listed in the available-skills catalog. Call \
                          this before answering when a skill matches the user's request, then follow the \
                          returned instructions."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Skill name exactly as listed in the catalog" }
                },
                "required": ["name"]
            }),
        }
    }

    async fn call(&self, args: Value) -> anyhow::Result<Value> {
        let skill = self.store.get_for(required_str(&args, "name")?, self.agent.as_deref())?;
        Ok(json!({ "name": skill.name, "instructions": skill.body }))
    }
}

/// Lets the model create or update a skill from a conversation (P16). Deliberately has no
/// `delete` action: removing a skill the user wrote is left to the UI, so a model mistake can only
/// ever overwrite, never silently lose, one.
pub struct ManageSkillTool {
    store: SkillStore,
}

impl ManageSkillTool {
    pub fn new(store: SkillStore) -> Self {
        Self { store }
    }
}

#[async_trait]
impl Tool for ManageSkillTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "manage_skill".to_string(),
            description: "Create or update a reusable skill (a saved set of instructions the assistant can \
                          load later with use_skill). Only use this when the user asks to create, save or \
                          change a skill. 'update' replaces the whole skill."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["create", "update"] },
                    "name": {
                        "type": "string",
                        "description": "Short slug: lowercase letters, digits and hyphens only, e.g. 'review-pr'"
                    },
                    "description": {
                        "type": "string",
                        "description": "One sentence on what the skill does and when to use it"
                    },
                    "body": {
                        "type": "string",
                        "description": "The full instructions, in markdown"
                    },
                    "agents": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional: ids of the agents this skill is restricted to. Omit to keep \
                                        the current restriction on update (none on create = every agent); \
                                        pass [] to make it available to everyone."
                    }
                },
                "required": ["action", "name", "description", "body"]
            }),
        }
    }

    async fn call(&self, args: Value) -> anyhow::Result<Value> {
        let action = required_str(&args, "action")?;
        let name = required_str(&args, "name")?.to_string();
        let agents = match args.get("agents") {
            None | Some(Value::Null) => None,
            Some(Value::Array(items)) => Some(
                items
                    .iter()
                    .map(|v| v.as_str().map(str::to_string).ok_or_else(|| anyhow::anyhow!("'agents' must be an array of strings")))
                    .collect::<anyhow::Result<Vec<String>>>()?,
            ),
            Some(_) => anyhow::bail!("'agents' must be an array of strings"),
        };
        let exists = self.store.exists(&name);
        // Editing a skill without mentioning `agents` must not silently drop its restriction.
        let agents = match (agents, exists) {
            (Some(agents), _) => agents,
            (None, true) => self.store.get(&name).map(|s| s.agents).unwrap_or_default(),
            (None, false) => Vec::new(),
        };
        let skill = Skill {
            name,
            description: required_str(&args, "description")?.to_string(),
            body: required_str(&args, "body")?.to_string(),
            agents,
        };
        skill.validate()?;

        match action {
            "create" if exists => anyhow::bail!("a skill named '{}' already exists; use action 'update'", skill.name),
            "update" if !exists => anyhow::bail!("no skill named '{}' to update; use action 'create'", skill.name),
            "create" | "update" => {}
            other => anyhow::bail!("unknown action '{other}', expected 'create' or 'update'"),
        }
        self.store.save(&skill)?;
        Ok(json!({ "status": "ok", "name": skill.name }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::Vault;
    use std::sync::Arc;

    fn temp_store() -> SkillStore {
        SkillStore::new(Arc::new(Vault::new(std::env::temp_dir().join(format!(
            "warden-skill-tools-test-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        )))))
    }

    fn create_args(name: &str) -> Value {
        json!({ "action": "create", "name": name, "description": "Does a thing", "body": "Do it." })
    }

    #[tokio::test]
    async fn create_then_use_skill() {
        let store = temp_store();
        let manage = ManageSkillTool::new(store.clone());
        let use_skill = UseSkillTool::new(store);

        manage.call(create_args("do-thing")).await.unwrap();
        let result = use_skill.call(json!({ "name": "do-thing" })).await.unwrap();

        assert_eq!(result, json!({ "name": "do-thing", "instructions": "Do it." }));
    }

    #[tokio::test]
    async fn use_skill_errors_on_unknown_name_and_missing_arg() {
        let use_skill = UseSkillTool::new(temp_store());
        assert!(use_skill.call(json!({ "name": "nope" })).await.unwrap_err().to_string().contains("no skill named"));
        assert!(use_skill.call(json!({})).await.unwrap_err().to_string().contains("name"));
    }

    #[tokio::test]
    async fn create_refuses_to_overwrite_and_update_refuses_to_create() {
        let manage = ManageSkillTool::new(temp_store());

        let update = json!({ "action": "update", "name": "x", "description": "d", "body": "b" });
        assert!(manage.call(update.clone()).await.is_err());

        manage.call(create_args("x")).await.unwrap();
        assert!(manage.call(create_args("x")).await.is_err());

        manage.call(update).await.unwrap();
    }

    #[tokio::test]
    async fn update_without_agents_keeps_the_restriction_and_empty_list_clears_it() {
        let store = temp_store();
        let manage = ManageSkillTool::new(store.clone());

        manage.call(json!({ "action": "create", "name": "x", "description": "d", "body": "b", "agents": ["writer"] })).await.unwrap();
        assert_eq!(store.get("x").unwrap().agents, vec!["writer"]);

        manage.call(json!({ "action": "update", "name": "x", "description": "d2", "body": "b2" })).await.unwrap();
        assert_eq!(store.get("x").unwrap().agents, vec!["writer"]);

        manage.call(json!({ "action": "update", "name": "x", "description": "d2", "body": "b2", "agents": [] })).await.unwrap();
        assert!(store.get("x").unwrap().agents.is_empty());

        assert!(manage.call(json!({ "action": "update", "name": "x", "description": "d", "body": "b", "agents": "writer" })).await.is_err());
    }

    #[tokio::test]
    async fn use_skill_only_loads_skills_visible_to_its_agent() {
        let store = temp_store();
        ManageSkillTool::new(store.clone())
            .call(json!({ "action": "create", "name": "x", "description": "d", "body": "b", "agents": ["writer"] }))
            .await
            .unwrap();

        assert!(UseSkillTool::new(store.clone()).call(json!({ "name": "x" })).await.is_err());
        assert!(UseSkillTool::new(store.clone()).for_agent(Some("reviewer".into())).call(json!({ "name": "x" })).await.is_err());
        assert!(UseSkillTool::new(store).for_agent(Some("writer".into())).call(json!({ "name": "x" })).await.is_ok());
    }

    #[tokio::test]
    async fn manage_skill_rejects_bad_names_and_unknown_actions() {
        let store = temp_store();
        let manage = ManageSkillTool::new(store.clone());

        assert!(manage.call(create_args("../escape")).await.is_err());
        assert!(manage.call(json!({ "action": "delete", "name": "x", "description": "d", "body": "b" })).await.is_err());
        assert!(store.list().is_empty());
    }
}
