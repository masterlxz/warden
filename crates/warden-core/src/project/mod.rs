//! Projects (P103) — a folder of the vault that groups conversations, with instructions and files of its own, in
//! the spirit of Claude's and ChatGPT's Projects. A project is `projects/<id>/PROJECT.md` with a small frontmatter
//! block; every other file in that folder (any type, subfolders too) is the project's own:
//!
//! ```text
//! ---
//! name: Tax return 2026
//! description: Everything about this year's return
//! ---
//! <the instructions, free-form markdown: what the model is told in every conversation of the project>
//! ```
//!
//! The `id` is the folder name (a plain slug, so it can't climb out of `projects/`); the `name` is what a person
//! sees, and can change without breaking the conversations that point at the id. Living in the vault means sync and
//! Obsidian editing come free, and a member's projects live in the member's own (possibly encrypted) vault.
//!
//! In a conversation of a project the turn runs on `ProjectStore::scope`, a vault rooted at the project's folder:
//! the file tools and the shell only reach the project's files, and the instructions are told to the model.
//! Removing a project (`delete`) only removes `PROJECT.md`: the files stay in the vault as ordinary notes.

use std::sync::Arc;

use anyhow::{anyhow, bail, Context};

use crate::memory::Vault;

/// Vault-root folder holding the projects, one subfolder each (defined with the vault, which hides it from the model's memory).
pub use crate::memory::PROJECTS_DIR;
/// The file in a project's folder that makes it a project.
pub const PROJECT_FILE: &str = "PROJECT.md";

pub const MAX_ID_LEN: usize = 64;
pub const MAX_NAME_LEN: usize = 80;
pub const MAX_DESCRIPTION_LEN: usize = 300;
/// The instructions go to the model in every call of every conversation of the project, like the vault context,
/// so they are kept short.
pub const MAX_INSTRUCTIONS_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub description: String,
    pub instructions: String,
    /// A code project's working folder (P103 b): a path on the machine the hub runs on — the repository the project is about.
    /// Only a project with one gets a `shell` in its conversations, which starts there and asks the person before every
    /// command. Not the project's folder in the vault (that is where its notes live).
    pub workdir: Option<String>,
}

pub const MAX_WORKDIR_LEN: usize = 512;

/// A working folder is a path someone typed: absolute (so it names one place whatever the hub's own folder is), with no
/// `..` and nothing a one-line frontmatter value couldn't hold. Whether it exists is for the machine that has it to say.
pub fn validate_workdir(workdir: &str) -> anyhow::Result<()> {
    if workdir.trim().is_empty() || workdir.len() > MAX_WORKDIR_LEN {
        bail!("the working folder must be 1-{MAX_WORKDIR_LEN} characters");
    }
    if workdir != workdir.trim() || workdir.chars().any(char::is_control) {
        bail!("the working folder has spaces around it or a control character");
    }
    let path = std::path::Path::new(workdir);
    if !path.is_absolute() {
        bail!("the working folder must be an absolute path (it starts with / or a drive letter)");
    }
    if path.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        bail!("the working folder can't contain '..'");
    }
    Ok(())
}

/// A project id doubles as a folder name: 1-64 ASCII letters, digits, `-` or `_`, which rules out separators, `..`
/// and dotfiles by construction. The same rule as a conversation id.
pub fn validate_id(id: &str) -> anyhow::Result<()> {
    if id.is_empty() || id.len() > MAX_ID_LEN || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
        bail!("project id must be 1-{MAX_ID_LEN} characters: letters, digits, '-' and '_'");
    }
    Ok(())
}

/// A frontmatter value lives on one line.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

impl Project {
    pub fn validate(&self) -> anyhow::Result<()> {
        validate_id(&self.id)?;
        if self.name.trim().is_empty() || self.name.chars().count() > MAX_NAME_LEN {
            bail!("project name must be 1-{MAX_NAME_LEN} characters");
        }
        if self.description.chars().count() > MAX_DESCRIPTION_LEN {
            bail!("project description must be at most {MAX_DESCRIPTION_LEN} characters");
        }
        if self.instructions.len() > MAX_INSTRUCTIONS_BYTES {
            bail!("project instructions are {} bytes, the limit is {MAX_INSTRUCTIONS_BYTES}", self.instructions.len());
        }
        if let Some(workdir) = &self.workdir {
            validate_workdir(workdir)?;
        }
        Ok(())
    }

    pub fn render(&self) -> String {
        let workdir = self.workdir.as_ref().map(|dir| format!("workdir: {dir}\n")).unwrap_or_default();
        format!("---\nname: {}\ndescription: {}\n{workdir}---\n{}\n", one_line(&self.name), one_line(&self.description), self.instructions.trim())
    }

    /// Parses `PROJECT.md`. `id` comes from the folder name (the source of truth); the frontmatter is optional, so a
    /// file without one is a project named after its id with the whole file as instructions.
    pub fn parse(id: &str, raw: &str) -> Project {
        let raw = raw.replace("\r\n", "\n");
        let (mut name, mut description) = (String::new(), String::new());
        let mut workdir = None;
        let mut body = raw.as_str();
        if let Some(rest) = raw.strip_prefix("---\n") {
            if let Some(end) = rest.find("\n---") {
                let (front, after) = rest.split_at(end);
                for line in front.lines() {
                    if let Some((key, value)) = line.split_once(':') {
                        match key.trim() {
                            "name" => name = value.trim().to_string(),
                            "description" => description = value.trim().to_string(),
                            // A hand-edited file with a folder that wouldn't pass `validate` has no working folder: no shell.
                            "workdir" if validate_workdir(value.trim()).is_ok() => workdir = Some(value.trim().to_string()),
                            _ => {}
                        }
                    }
                }
                body = after["\n---".len()..].trim_start_matches('\n');
            }
        }
        if name.is_empty() {
            name = id.to_string();
        }
        Project { id: id.to_string(), name, description, instructions: body.trim().to_string(), workdir }
    }

    /// What the model is told at the start of every turn of a conversation in this project: the instructions, and
    /// what the file tools reach now. `files` are the project's files as the scoped vault names them. `shell`: this
    /// turn has the project's shell (a working folder, and a shell on this machine), which the briefing then describes —
    /// including that it isn't confined to the folder and that every command is put to the person first.
    pub fn briefing(&self, files: &[String], shell: bool) -> String {
        let mut text = format!("You are working in the project \"{}\".", self.name);
        if !self.description.trim().is_empty() {
            text.push_str(&format!(" {}", one_line(&self.description)));
        }
        text.push_str(
            "\nYour files and the shell reach only this project's folder; paths are relative to it. Keep what you create for this project there.",
        );
        if let (true, Some(workdir)) = (shell, &self.workdir) {
            text.push_str(&format!(
                "\n\nThis is a code project. Its working folder is {workdir}, and the `shell` tool starts there. The shell is not confined to \
                 that folder, and every command is shown to the person for approval before it runs, so keep commands few, plain and \
                 explained. Your file tools reach only the project's notes above, not that folder: read and change the repository's files \
                 through the shell."
            ));
        }
        if !self.instructions.trim().is_empty() {
            text.push_str(&format!("\n\nProject instructions:\n{}", self.instructions.trim()));
        }
        let files: Vec<&String> = files.iter().filter(|f| f.as_str() != PROJECT_FILE).collect();
        if !files.is_empty() {
            const SHOWN: usize = 50;
            text.push_str("\n\nFiles in the project:");
            for file in files.iter().take(SHOWN) {
                text.push_str(&format!("\n- {file}"));
            }
            if files.len() > SHOWN {
                text.push_str(&format!("\n- … and {} more", files.len() - SHOWN));
            }
        }
        text
    }
}

/// Reads and writes projects under `projects/` in a vault.
#[derive(Clone)]
pub struct ProjectStore {
    vault: Arc<Vault>,
}

impl ProjectStore {
    pub fn new(vault: Arc<Vault>) -> Self {
        Self { vault }
    }

    fn file_of(id: &str) -> anyhow::Result<String> {
        validate_id(id)?;
        Ok(format!("{PROJECTS_DIR}/{id}/{PROJECT_FILE}"))
    }

    /// Every project, sorted by name (then id). A folder without a readable `PROJECT.md` isn't a project, and one bad
    /// file must not hide the rest.
    pub fn list(&self) -> Vec<Project> {
        let Ok(dirs) = self.vault.dirs_in(PROJECTS_DIR) else { return Vec::new() };
        let mut projects: Vec<Project> = dirs
            .into_iter()
            .filter(|id| validate_id(id).is_ok())
            .filter_map(|id| {
                let raw = self.vault.read(&Self::file_of(&id).ok()?).ok()?;
                Some(Project::parse(&id, &raw))
            })
            .collect();
        projects.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then_with(|| a.id.cmp(&b.id)));
        projects
    }

    pub fn get(&self, id: &str) -> anyhow::Result<Project> {
        let raw = self.vault.read(&Self::file_of(id)?).map_err(|_| anyhow!("no project '{id}'"))?;
        Ok(Project::parse(id, &raw))
    }

    pub fn exists(&self, id: &str) -> bool {
        Self::file_of(id).is_ok_and(|path| self.vault.is_file(&path))
    }

    /// Creates or replaces a project's `PROJECT.md`; its other files are untouched.
    pub fn save(&self, project: &Project) -> anyhow::Result<()> {
        project.validate()?;
        self.vault.write(&Self::file_of(&project.id)?, &project.render()).with_context(|| format!("failed to write project '{}'", project.id))
    }

    /// Removes the project: only `PROJECT.md`. The folder and the files in it stay in the vault as ordinary notes.
    pub fn delete(&self, id: &str) -> anyhow::Result<()> {
        self.vault.delete(&Self::file_of(id)?).map_err(|_| anyhow!("no project '{id}'"))
    }

    /// The vault a turn of a conversation in project `id` runs on: rooted at the project's folder, same encryption.
    /// An unknown project is an error, so a conversation whose project is gone can be told apart (and run unscoped).
    pub fn scope(&self, id: &str) -> anyhow::Result<Vault> {
        if !self.exists(id) {
            bail!("no project '{id}'");
        }
        self.vault.subvault(&format!("{PROJECTS_DIR}/{id}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::VaultCipher;

    fn dir(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("warden-project-{name}-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()))
    }

    fn store(name: &str) -> ProjectStore {
        ProjectStore::new(Arc::new(Vault::new(dir(name))))
    }

    fn sample(id: &str) -> Project {
        Project { id: id.into(), name: "Tax return".into(), description: "This year's return".into(), instructions: "Answer in Portuguese.\nCite the file.".into(), workdir: None }
    }

    #[test]
    fn a_working_folder_round_trips_and_a_bad_one_is_refused_or_dropped() {
        let store = store("workdir");
        let code = Project { workdir: Some("/home/me/repo".into()), ..sample("code") };
        store.save(&code).unwrap();
        assert_eq!(store.get("code").unwrap(), code);
        for bad in ["relative/dir", "/a/../b", "", " /a", "/a\nb"] {
            assert!(Project { workdir: Some(bad.into()), ..sample("x") }.validate().is_err(), "{bad:?}");
        }
        assert_eq!(Project::parse("p", "---\nname: P\nworkdir: relative\n---\nbody").workdir, None, "a hand-edited bad folder means no shell");
    }

    #[test]
    fn an_id_is_a_plain_slug() {
        for ok in ["a", "tax-2026", "Tax_Return", &"x".repeat(64)] {
            assert!(validate_id(ok).is_ok(), "{ok}");
        }
        for bad in ["", "../x", "a/b", "a.b", ".hidden", "a b", "é", &"x".repeat(65)] {
            assert!(validate_id(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_project_survives_the_round_trip_and_a_file_without_frontmatter_is_still_one() {
        let store = store("roundtrip");
        store.save(&sample("tax")).unwrap();
        assert_eq!(store.get("tax").unwrap(), sample("tax"));

        let parsed = Project::parse("loose", "just instructions\nsecond line");
        assert_eq!((parsed.name.as_str(), parsed.description.as_str(), parsed.instructions.as_str()), ("loose", "", "just instructions\nsecond line"));
        // A name with a colon keeps everything after the first one, and a CRLF file parses the same.
        let crlf = Project::parse("p", "---\r\nname: A: b\r\ndescription: d\r\n---\r\nbody\r\n");
        assert_eq!((crlf.name.as_str(), crlf.instructions.as_str()), ("A: b", "body"));
    }

    #[test]
    fn save_refuses_what_it_cannot_keep() {
        let store = store("validate");
        let too_long = Project { instructions: "x".repeat(MAX_INSTRUCTIONS_BYTES + 1), ..sample("a") };
        for (what, project) in [
            ("a bad id", sample("../a")),
            ("no name", Project { name: "  ".into(), ..sample("a") }),
            ("a long name", Project { name: "n".repeat(MAX_NAME_LEN + 1), ..sample("a") }),
            ("a long description", Project { description: "d".repeat(MAX_DESCRIPTION_LEN + 1), ..sample("a") }),
            ("long instructions", too_long),
        ] {
            assert!(store.save(&project).is_err(), "{what}");
        }
        assert!(store.list().is_empty(), "nothing was written");
    }

    #[test]
    fn the_list_is_sorted_by_name_and_skips_folders_that_are_not_projects() {
        let store = store("list");
        store.save(&Project { name: "beta".into(), ..sample("b") }).unwrap();
        store.save(&Project { name: "Alpha".into(), ..sample("a") }).unwrap();
        // A folder with notes but no PROJECT.md, and a stray file: neither is a project.
        store.vault.write("projects/loose/notes.md", "x").unwrap();
        store.vault.write("projects/readme.md", "x").unwrap();
        assert_eq!(store.list().iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        assert!(store.get("loose").is_err() && !store.exists("loose"));
    }

    #[test]
    fn removing_a_project_keeps_its_files() {
        let store = store("delete");
        store.save(&sample("tax")).unwrap();
        store.vault.write("projects/tax/receipts/jan.md", "R$ 10").unwrap();
        store.delete("tax").unwrap();
        assert!(store.list().is_empty() && store.get("tax").is_err());
        assert_eq!(store.vault.read("projects/tax/receipts/jan.md").unwrap(), "R$ 10", "the files stay as ordinary notes");
        assert!(store.delete("tax").is_err(), "it is already gone");
        assert!(store.delete("../tax").is_err());
    }

    #[test]
    fn a_scoped_vault_reaches_only_the_projects_folder() {
        let store = store("scope");
        store.save(&sample("tax")).unwrap();
        store.vault.write("projects/tax/jan.md", "in the project").unwrap();
        store.vault.write("diary.md", "outside").unwrap();
        store.vault.write("projects/other/secret.md", "another project").unwrap();

        let scoped = store.scope("tax").unwrap();
        assert_eq!(scoped.read("jan.md").unwrap(), "in the project");
        for escape in ["../../diary.md", "../other/secret.md", "/etc/passwd", "projects/other/secret.md", "diary.md"] {
            assert!(scoped.read(escape).is_err(), "{escape}");
        }
        scoped.write("made-here.md", "new").unwrap();
        assert_eq!(store.vault.read("projects/tax/made-here.md").unwrap(), "new", "what the model writes lands in the project's folder");
        assert!(scoped.write("../escaped.md", "x").is_err());
        assert!(!store.vault.is_file("projects/escaped.md"));
        assert!(store.scope("nope").is_err() && store.scope("../tax").is_err(), "an unknown project has no scope");
    }

    #[test]
    fn an_encrypted_vaults_projects_stay_encrypted_and_scope_still_reads_them() {
        let root = dir("encrypted");
        let cipher = Arc::new(VaultCipher::new(&[9; 32]));
        let store = ProjectStore::new(Arc::new(Vault::new_encrypted(&root, cipher.clone())));
        store.save(&sample("tax")).unwrap();
        store.vault.write("projects/tax/jan.md", "secret figures").unwrap();

        assert_eq!(store.list().len(), 1);
        let scoped = store.scope("tax").unwrap();
        assert!(scoped.is_encrypted());
        assert_eq!(scoped.read("jan.md").unwrap(), "secret figures");
        scoped.write("new.md", "also secret").unwrap();
        assert_eq!(store.vault.read("projects/tax/new.md").unwrap(), "also secret");
        // Nothing readable is on disk: neither the names nor the content.
        let on_disk = std::fs::read_dir(&root).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect::<Vec<_>>();
        assert!(!on_disk.iter().any(|name| name.contains("projects")), "{on_disk:?}");
        assert!(!store.vault.list_all_files().unwrap().is_empty());
    }

    #[test]
    fn the_briefing_tells_the_instructions_and_the_files_without_the_project_file() {
        let project = sample("tax");
        let text = project.briefing(&["PROJECT.md".into(), "jan.md".into(), "receipts/feb.md".into()], false);
        assert!(text.contains("\"Tax return\"") && text.contains("Answer in Portuguese.") && text.contains("- jan.md") && text.contains("- receipts/feb.md"), "{text}");
        assert!(!text.contains("PROJECT.md"), "the file that defines the project isn't one of its files: {text}");
        let many: Vec<String> = (0..60).map(|n| format!("f{n}.md")).collect();
        assert!(project.briefing(&many, false).contains("… and 10 more"));
        let bare = Project { instructions: String::new(), description: String::new(), ..sample("tax") };
        assert!(!bare.briefing(&[], false).contains("Project instructions") && !bare.briefing(&[], false).contains("Files in the project"));
        let code = Project { workdir: Some("/home/me/repo".into()), ..sample("code") };
        assert!(code.briefing(&[], true).contains("/home/me/repo"));
        assert!(!code.briefing(&[], false).contains("/home/me/repo"), "no shell this turn, so no working folder in the briefing");
    }
}
