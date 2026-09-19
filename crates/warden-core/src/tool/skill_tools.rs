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
}

impl UseSkillTool {
    pub fn new(store: SkillStore) -> Self {
        Self { store }
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
        let skill = self.store.get(required_str(&args, "name")?)?;
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
                    }
                },
                "required": ["action", "name", "description", "body"]
            }),
        }
    }

    async fn call(&self, args: Value) -> anyhow::Result<Value> {
        let action = required_str(&args, "action")?;
        let skill = Skill {
            name: required_str(&args, "name")?.to_string(),
            description: required_str(&args, "description")?.to_string(),
            body: required_str(&args, "body")?.to_string(),
        };
        skill.validate()?;

        let exists = self.store.exists(&skill.name);
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
    async fn manage_skill_rejects_bad_names_and_unknown_actions() {
        let store = temp_store();
        let manage = ManageSkillTool::new(store.clone());

        assert!(manage.call(create_args("../escape")).await.is_err());
        assert!(manage.call(json!({ "action": "delete", "name": "x", "description": "d", "body": "b" })).await.is_err());
        assert!(store.list().is_empty());
    }
}
