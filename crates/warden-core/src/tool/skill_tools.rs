use async_trait::async_trait;
use serde_json::{json, Value};

use crate::skill::{validate_file_name, Skill, SkillStore, MAX_FILES_PER_SKILL, MAX_FILE_BYTES};
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
                          returned instructions. If the result lists `files` (attachments such as scripts or \
                          templates), read one with read_skill_file, or run a script from its `path` with the \
                          shell tool when that is available."
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
        let mut result = json!({ "name": skill.name, "instructions": skill.body });
        // Only when there are any, so a skill without attachments answers exactly as before.
        let files = self.store.list_files(&skill.name)?;
        if !files.is_empty() {
            let files = files
                .iter()
                .map(|f| Ok(json!({ "name": f, "path": SkillStore::file_relative_path(&skill.name, f)? })))
                .collect::<anyhow::Result<Vec<Value>>>()?;
            result["files"] = Value::Array(files);
        }
        Ok(result)
    }
}

/// Reads one attachment of a skill (P72 d) — the text files listed by `use_skill`. Scoped to the
/// agent like `use_skill`: an attachment of a skill restricted to other agents reads as "no such skill".
pub struct ReadSkillFileTool {
    store: SkillStore,
    agent: Option<String>,
}

impl ReadSkillFileTool {
    pub fn new(store: SkillStore) -> Self {
        Self { store, agent: None }
    }

    pub fn for_agent(mut self, agent: Option<String>) -> Self {
        self.agent = agent;
        self
    }
}

#[async_trait]
impl Tool for ReadSkillFileTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "read_skill_file".to_string(),
            description: "Read a text file attached to a skill (a script, template or reference note listed in \
                          the `files` of the use_skill result)."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "skill": { "type": "string", "description": "Skill name" },
                    "file": { "type": "string", "description": "Attachment name exactly as listed by use_skill" }
                },
                "required": ["skill", "file"]
            }),
        }
    }

    async fn call(&self, args: Value) -> anyhow::Result<Value> {
        let skill = required_str(&args, "skill")?;
        let file = required_str(&args, "file")?;
        let content = self.store.read_file_for(skill, file, self.agent.as_deref())?;
        Ok(json!({ "skill": skill, "file": file, "content": content }))
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
                    },
                    "files": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "name": { "type": "string", "description": "File name, e.g. 'run.sh'" },
                                "content": { "type": "string", "description": "Text content of the file" }
                            },
                            "required": ["name", "content"]
                        },
                        "description": "Optional: text files to attach to the skill (scripts, templates), added \
                                        or replaced by name. Omit to leave the current attachments untouched; \
                                        attachments are never removed through this tool."
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
        let files = parse_files(&args)?;
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
        if !files.is_empty() {
            // Checked before anything is written, so a bad attachment doesn't leave a half-saved skill.
            let mut names = self.store.list_files(&skill.name)?;
            for (file, _) in &files {
                if !names.contains(file) {
                    names.push(file.clone());
                }
            }
            if names.len() > MAX_FILES_PER_SKILL {
                anyhow::bail!("a skill can have at most {MAX_FILES_PER_SKILL} attachments");
            }
        }
        self.store.save(&skill)?;
        for (file, content) in &files {
            self.store.save_file(&skill.name, file, content)?;
        }
        Ok(json!({ "status": "ok", "name": skill.name }))
    }
}

/// The optional `files` argument of `manage_skill`, validated up front (name and size).
fn parse_files(args: &Value) -> anyhow::Result<Vec<(String, String)>> {
    let items = match args.get("files") {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::Array(items)) => items,
        Some(_) => anyhow::bail!("'files' must be an array of {{name, content}} objects"),
    };
    let mut files: Vec<(String, String)> = Vec::new();
    for item in items {
        let name = required_str(item, "name")?;
        let content = required_str(item, "content")?;
        validate_file_name(name)?;
        if content.len() > MAX_FILE_BYTES {
            anyhow::bail!("attachment '{name}' is {} bytes, the limit is {MAX_FILE_BYTES}", content.len());
        }
        // The same name twice: the last one wins, like an upsert applied in order.
        files.retain(|(n, _)| n != name);
        files.push((name.to_string(), content.to_string()));
    }
    Ok(files)
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

    #[tokio::test]
    async fn manage_skill_attaches_files_and_use_skill_lists_them() {
        let store = temp_store();
        let manage = ManageSkillTool::new(store.clone());
        let use_skill = UseSkillTool::new(store.clone());

        manage
            .call(json!({
                "action": "create", "name": "x", "description": "d", "body": "b",
                "files": [{ "name": "run.sh", "content": "echo hi" }, { "name": "notes.md", "content": "# n" }]
            }))
            .await
            .unwrap();

        let result = use_skill.call(json!({ "name": "x" })).await.unwrap();
        assert_eq!(
            result["files"],
            json!([
                { "name": "notes.md", "path": "skills/x.files/notes.md" },
                { "name": "run.sh", "path": "skills/x.files/run.sh" }
            ])
        );
        assert_eq!(store.read_file("x", "run.sh").unwrap(), "echo hi");

        // Updating without `files` keeps them; with `files` upserts by name and leaves the rest.
        manage.call(json!({ "action": "update", "name": "x", "description": "d2", "body": "b2" })).await.unwrap();
        assert_eq!(store.list_files("x").unwrap(), vec!["notes.md", "run.sh"]);
        manage
            .call(json!({ "action": "update", "name": "x", "description": "d", "body": "b", "files": [{ "name": "run.sh", "content": "echo v2" }] }))
            .await
            .unwrap();
        assert_eq!(store.read_file("x", "run.sh").unwrap(), "echo v2");
        assert_eq!(store.list_files("x").unwrap().len(), 2);
    }

    #[tokio::test]
    async fn manage_skill_rejects_bad_files_before_writing_anything() {
        let store = temp_store();
        let manage = ManageSkillTool::new(store.clone());
        let with = |files: Value| json!({ "action": "create", "name": "x", "description": "d", "body": "b", "files": files });

        assert!(manage.call(with(json!([{ "name": "../escape", "content": "z" }]))).await.is_err());
        assert!(manage.call(with(json!([{ "name": "a.txt" }]))).await.is_err());
        assert!(manage.call(with(json!("run.sh"))).await.is_err());
        let big = "a".repeat(MAX_FILE_BYTES + 1);
        assert!(manage.call(with(json!([{ "name": "big.txt", "content": big }]))).await.is_err());
        let many: Vec<Value> = (0..=MAX_FILES_PER_SKILL).map(|i| json!({ "name": format!("f{i}"), "content": "c" })).collect();
        assert!(manage.call(with(Value::Array(many))).await.is_err());
        assert!(store.list().is_empty(), "no half-saved skill");
    }

    #[tokio::test]
    async fn use_skill_result_has_no_files_key_without_attachments() {
        let store = temp_store();
        ManageSkillTool::new(store.clone()).call(create_args("x")).await.unwrap();
        let result = UseSkillTool::new(store).call(json!({ "name": "x" })).await.unwrap();
        assert!(result.get("files").is_none());
    }

    #[tokio::test]
    async fn read_skill_file_reads_and_respects_agent_scope_and_traversal() {
        let store = temp_store();
        ManageSkillTool::new(store.clone())
            .call(json!({
                "action": "create", "name": "x", "description": "d", "body": "b", "agents": ["writer"],
                "files": [{ "name": "a.txt", "content": "hello" }]
            }))
            .await
            .unwrap();
        let args = json!({ "skill": "x", "file": "a.txt" });

        let ok = ReadSkillFileTool::new(store.clone()).for_agent(Some("writer".into())).call(args.clone()).await.unwrap();
        assert_eq!(ok, json!({ "skill": "x", "file": "a.txt", "content": "hello" }));

        let hidden = ReadSkillFileTool::new(store.clone()).call(args).await.unwrap_err();
        assert!(hidden.to_string().contains("no skill named"));

        let tool = ReadSkillFileTool::new(store).for_agent(Some("writer".into()));
        assert!(tool.call(json!({ "skill": "x", "file": "../x.md" })).await.is_err());
        assert!(tool.call(json!({ "skill": "x", "file": "missing.txt" })).await.unwrap_err().to_string().contains("no attachment"));
        assert!(tool.call(json!({ "skill": "x" })).await.is_err());
    }
}
