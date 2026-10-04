//! Turning a channel's orchestrator into "the orchestrator of project X" for one turn (P103). The hub and the
//! desktop both do exactly this, so it lives in one place, like `agent_scope`.
//!
//! A turn in a project runs on a vault rooted at the project's folder (`ProjectStore::scope`): the file tools, the
//! skill catalog and the vault context only reach what is in the project. Three kinds of tool can't be held to a folder
//! — a shell command can `cat ../x` or `cd /`, `ssh_exec`/`node_shell` run commands somewhere else, and `search_history`
//! reads every conversation — so a project turn doesn't get them, rather than promising an isolation they would break.
//! Tools of an MCP server or the web aren't vault access and are left as they are.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use warden_core::memory::Vault;
use warden_core::orchestrator::Orchestrator;
use warden_core::project::ProjectStore;

/// Tools a turn in a project doesn't get, because nothing keeps them inside the project's folder.
pub const WITHHELD_IN_A_PROJECT: [&str; 4] = ["shell", "ssh_exec", "node_shell", "search_history"];

/// One `Vault` per project folder, kept for the process's life so a folder's semantic model loads once, not once per
/// turn (the same reasoning as the hub's shared spaces). Keyed by where the folder is on disk and, for an encrypted
/// vault, by a fingerprint of its key, so a vault opened with another key is never served the old one.
static SCOPES: OnceLock<ScopeCache> = OnceLock::new();

type ScopeCache = Mutex<HashMap<(PathBuf, Option<String>), Arc<Vault>>>;

fn scope_vault(store: &ProjectStore, project_id: &str) -> anyhow::Result<Arc<Vault>> {
    let scoped = store.scope(project_id)?;
    let fingerprint = scoped.cipher().map(|cipher| cipher.seal_name("scope").unwrap_or_default());
    let key = (scoped.root().clone(), fingerprint);
    let mut cache = SCOPES.get_or_init(Mutex::default).lock().unwrap_or_else(|e| e.into_inner());
    Ok(cache.entry(key).or_insert_with(|| Arc::new(scoped)).clone())
}

/// Scopes `orchestrator` to project `project_id` of the vault it reads now (the person's own: a member's
/// orchestrator already has theirs), or `Ok(None)` when there is no such project — it was removed, and what pointed
/// at it goes on as an ordinary turn. Any other failure (a locked vault, an unreadable folder) is an error: running
/// the turn with the whole vault would break what the person chose.
pub fn scope_to_project(orchestrator: &Orchestrator, project_id: &str) -> anyhow::Result<Option<Orchestrator>> {
    let store = ProjectStore::new(orchestrator.vault().clone());
    warden_core::project::validate_id(project_id)?;
    if !store.exists(project_id) {
        return Ok(None);
    }
    let project = store.get(project_id)?;
    let vault = scope_vault(&store, project_id)?;
    let mut files: Vec<String> = vault.list_all_files()?.into_iter().map(|p| p.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/")).collect();
    files.sort();
    let allowed: Vec<String> = orchestrator.tools().iter().map(|tool| tool.spec().name).filter(|name| !WITHHELD_IN_A_PROJECT.contains(&name.as_str())).collect();
    // A code project (P103 b) gets its own shell back — in its working folder, asking before every command — but only
    // where there is a shell to give: this machine has it on, and the person it speaks for is allowed it (a member's
    // orchestrator has already lost it).
    let shell = project.workdir.as_ref().filter(|_| orchestrator.tools().iter().any(|tool| tool.spec().name == "shell"));
    let briefing = project.briefing(&files, shell.is_some());
    let mut scoped = orchestrator.with_allowed_tools(Some(&allowed)).with_project(vault, briefing);
    if let Some(folder) = shell {
        let tool = warden_core::tool::shell::ShellTool::in_folder(project.name.clone(), std::path::PathBuf::from(folder));
        let tool: Arc<dyn warden_core::tool::Tool> = match orchestrator.approver().and_then(|approver| warden_core::tool::Tool::with_approver(&tool, approver)) {
            Some(asking) => asking,
            None => Arc::new(tool),
        };
        scoped = scoped.with_tool(tool);
    }
    Ok(Some(scoped))
}

/// What a turn in a working folder (P102) doesn't get: `ssh_exec` and `node_shell` run somewhere else, so no folder
/// of this machine holds them. `search_history` stays (the conversation is the person's own), and the vault is not
/// rebound — the notes and skills are still theirs.
const WITHHELD_IN_A_FOLDER: [&str; 2] = ["ssh_exec", "node_shell"];

/// Scopes `orchestrator` to the working folder `folder` of this machine for one turn (P102), for a conversation that
/// is in no project: `read_file` and `write_file` act in the folder (and cannot leave it), and the `shell`, when this
/// channel has one, starts there and asks before every command. Fails when the folder is not a folder (any more): a
/// turn that was promised a folder must not run on the vault instead.
pub fn scope_to_workdir(orchestrator: &Orchestrator, folder: &str) -> anyhow::Result<Orchestrator> {
    warden_core::project::validate_workdir(folder)?;
    anyhow::ensure!(std::path::Path::new(folder).is_dir(), "the working folder '{folder}' does not exist");
    let has = |name: &str| orchestrator.tools().iter().any(|tool| tool.spec().name == name);
    let (read, write, shell) = (has("read_file"), has("write_file"), has("shell"));
    let allowed: Vec<String> = orchestrator
        .tools()
        .iter()
        .map(|tool| tool.spec().name)
        .filter(|name| !WITHHELD_IN_A_FOLDER.contains(&name.as_str()) && !["read_file", "write_file", "shell"].contains(&name.as_str()))
        .collect();
    let name = std::path::Path::new(folder).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| folder.to_string());
    let mut scoped = orchestrator.with_allowed_tools(Some(&allowed));
    if read {
        scoped = scoped.with_tool(Arc::new(warden_core::tool::folder_tools::FolderReadTool::new(folder)));
    }
    if write {
        scoped = scoped.with_tool(Arc::new(warden_core::tool::folder_tools::FolderWriteTool::new(folder)));
    }
    if shell {
        let tool = warden_core::tool::shell::ShellTool::in_folder(name, std::path::PathBuf::from(folder));
        let tool: Arc<dyn warden_core::tool::Tool> = match orchestrator.approver().and_then(|approver| warden_core::tool::Tool::with_approver(&tool, approver)) {
            Some(asking) => asking,
            None => Arc::new(tool),
        };
        scoped = scoped.with_tool(tool);
    }
    let mut briefing = format!("This conversation works in the folder '{folder}' on this machine. ");
    briefing.push_str(match (read || write, shell) {
        (true, true) => "Your read_file and write_file tools act on that folder (paths are relative to it) and cannot leave it; your shell starts in it and the person approves every command. ",
        (true, false) => "Your read_file and write_file tools act on that folder (paths are relative to it) and cannot leave it. ",
        (false, true) => "Your shell starts in it and the person approves every command. ",
        (false, false) => "",
    });
    briefing.push_str("The person's own notes and skills are still in your context as always, but the file tools no longer reach them.");
    Ok(scoped.with_briefing(briefing))
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use serde_json::{json, Value};
    use warden_core::model::{ChatStream, Message, ModelProvider};
    use warden_core::project::Project;
    use warden_core::tool::file_tools::{ReadFileTool, WriteFileTool};
    use warden_core::tool::{Tool, ToolSpec};

    use super::*;

    struct NoModel;

    #[async_trait]
    impl ModelProvider for NoModel {
        async fn chat_stream(&self, _messages: Vec<Message>, _tools: Vec<ToolSpec>) -> anyhow::Result<ChatStream> {
            anyhow::bail!("no model in this test")
        }
    }

    /// A tool that only has a name, standing in for `shell` and the rest.
    struct Named(&'static str);

    #[async_trait]
    impl Tool for Named {
        fn spec(&self) -> ToolSpec {
            ToolSpec { name: self.0.to_string(), description: String::new(), parameters: json!({ "type": "object" }) }
        }
        async fn call(&self, _args: Value) -> anyhow::Result<Value> {
            Ok(json!({}))
        }
    }

    fn orchestrator(name: &str) -> (Orchestrator, Arc<Vault>) {
        let dir = std::env::temp_dir().join(format!("warden-project-scope-{name}-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let vault = Arc::new(Vault::new(dir));
        let mut orchestrator = Orchestrator::new(Arc::new(NoModel), vault.clone());
        orchestrator.register_tool(Arc::new(ReadFileTool::new(vault.clone())));
        orchestrator.register_tool(Arc::new(WriteFileTool::new(vault.clone())));
        for name in WITHHELD_IN_A_PROJECT.into_iter().chain(["web_search"]) {
            orchestrator.register_tool(Arc::new(Named(name)));
        }
        (orchestrator, vault)
    }

    fn names(orchestrator: &Orchestrator) -> Vec<String> {
        orchestrator.tools().iter().map(|t| t.spec().name).collect()
    }

    #[test]
    fn a_project_turn_loses_the_tools_a_folder_cannot_hold_and_keeps_the_rest() {
        let (base, vault) = orchestrator("tools");
        ProjectStore::new(vault.clone()).save(&Project { id: "tax".into(), name: "Tax".into(), description: String::new(), instructions: "Be brief.".into(), workdir: None, code: false }).unwrap();
        vault.write("projects/tax/jan.md", "x").unwrap();

        let scoped = scope_to_project(&base, "tax").unwrap().expect("the project exists");
        let kept = names(&scoped);
        for gone in WITHHELD_IN_A_PROJECT {
            assert!(!kept.iter().any(|n| n == gone), "{gone} stays out of a project: {kept:?}");
        }
        for stays in ["read_file", "write_file", "web_search"] {
            assert!(kept.iter().any(|n| n == stays), "{stays}: {kept:?}");
        }
        assert!(names(&base).iter().any(|n| n == "shell"), "the original keeps its tools");
        assert!(!Arc::ptr_eq(scoped.vault(), &vault), "and the scoped one reads the project's folder");
    }

    #[test]
    fn a_missing_project_is_no_scope_and_a_bad_id_is_an_error() {
        let (base, vault) = orchestrator("missing");
        assert!(scope_to_project(&base, "gone").unwrap().is_none());
        vault.write("projects/loose/notes.md", "x").unwrap();
        assert!(scope_to_project(&base, "loose").unwrap().is_none(), "a folder without PROJECT.md isn't a project");
        assert!(scope_to_project(&base, "../x").is_err());
    }

    fn a_folder() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("warden-workdir-scope-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn tool<'a>(orchestrator: &'a Orchestrator, name: &str) -> &'a Arc<dyn Tool> {
        orchestrator.tools().iter().find(|t| t.spec().name == name).unwrap_or_else(|| panic!("no {name}"))
    }

    #[tokio::test]
    async fn a_folder_turn_holds_the_file_tools_in_the_folder_and_leaves_the_vault_alone() {
        let (base, vault) = orchestrator("folder");
        let folder = a_folder();
        let scoped = scope_to_workdir(&base, folder.to_str().unwrap()).unwrap();

        let kept = names(&scoped);
        for gone in WITHHELD_IN_A_FOLDER {
            assert!(!kept.iter().any(|n| n == gone), "{gone}: {kept:?}");
        }
        for stays in ["read_file", "write_file", "shell", "search_history", "web_search"] {
            assert_eq!(kept.iter().filter(|n| *n == stays).count(), 1, "{stays} once: {kept:?}");
        }
        assert!(Arc::ptr_eq(scoped.vault(), &vault), "the person's notes and skills are not rebound");

        tool(&scoped, "write_file").call(json!({ "path": "out/a.md", "content": "hi" })).await.unwrap();
        assert_eq!(std::fs::read_to_string(folder.join("out/a.md")).unwrap(), "hi");
        assert!(!vault.is_file("out/a.md"), "it went to the folder, not the vault");
        assert_eq!(tool(&scoped, "read_file").call(json!({ "path": "out/a.md" })).await.unwrap()["content"], "hi");
        assert!(tool(&scoped, "read_file").call(json!({ "path": "../x" })).await.is_err());
        assert!(names(&base).iter().any(|n| n == "node_shell"), "the original keeps its tools");
    }

    #[tokio::test]
    async fn a_folder_turn_gets_no_shell_where_the_channel_has_none() {
        let vault = Arc::new(Vault::new(std::env::temp_dir().join(format!("warden-workdir-noshell-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()))));
        let mut base = Orchestrator::new(Arc::new(NoModel), vault.clone());
        base.register_tool(Arc::new(ReadFileTool::new(vault)));
        let scoped = scope_to_workdir(&base, a_folder().to_str().unwrap()).unwrap();
        assert_eq!(names(&scoped), ["read_file"], "a member without the shell gets file tools in the folder and nothing more");
    }

    #[test]
    fn a_folder_that_is_not_there_or_not_absolute_is_an_error() {
        let (base, _) = orchestrator("badfolder");
        assert!(scope_to_workdir(&base, "/no/such/folder/for/warden").is_err());
        assert!(scope_to_workdir(&base, "relative/dir").is_err());
        assert!(scope_to_workdir(&base, "/tmp/../etc").is_err());
    }

    #[test]
    fn the_scoped_vault_is_kept_so_a_projects_search_model_loads_once() {
        let (base, vault) = orchestrator("cache");
        ProjectStore::new(vault).save(&Project { id: "tax".into(), name: "Tax".into(), description: String::new(), instructions: String::new(), workdir: None, code: false }).unwrap();
        let first = scope_to_project(&base, "tax").unwrap().unwrap();
        let second = scope_to_project(&base, "tax").unwrap().unwrap();
        assert!(Arc::ptr_eq(first.vault(), second.vault()));
    }
}
