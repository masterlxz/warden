//! P102: `read_file` and `write_file` for a conversation that works in a folder of the machine. Same names and
//! arguments as the vault's tools (the model already knows them), but the paths are relative to the chosen folder and
//! nothing gets out of it: no `..`, no absolute path, and no symlink that leads somewhere else. They never touch the
//! vault, so the person's notes and skills are not rebound to a folder that belongs to something else.

use std::path::{Component, Path, PathBuf};

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::tool::{Tool, ToolSpec};

/// What a read returns at most: a file bigger than this would only fill the context.
const MAX_READ_BYTES: u64 = 1024 * 1024;

/// Where `relative` is under `root`, or why it is refused. The deepest part of the path that exists (the file, or the
/// folder it will be created in) must still be under the resolved root once symlinks are followed.
fn inside(root: &Path, relative: &str) -> anyhow::Result<PathBuf> {
    let invalid = || anyhow::anyhow!("'{relative}' is not a path inside the working folder");
    if relative.is_empty() || relative.contains('\\') || relative.len() > 512 {
        return Err(invalid());
    }
    if !Path::new(relative).components().all(|c| matches!(c, Component::Normal(_) | Component::CurDir)) {
        return Err(invalid());
    }
    let resolved_root = root.canonicalize().map_err(|_| anyhow::anyhow!("the working folder is not there any more"))?;
    let path = root.join(relative);
    let existing = path.ancestors().find(|p| p.symlink_metadata().is_ok()).unwrap_or(root);
    if !existing.canonicalize()?.starts_with(&resolved_root) {
        return Err(invalid());
    }
    Ok(path)
}

/// Reads a text file of the working folder.
pub struct FolderReadTool {
    root: PathBuf,
}

impl FolderReadTool {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

#[async_trait]
impl Tool for FolderReadTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "read_file".to_string(),
            description: "Read a text file from the working folder by its path relative to that folder.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path relative to the working folder, e.g. 'notes/todo.md'" }
                },
                "required": ["path"]
            }),
        }
    }

    async fn call(&self, args: Value) -> anyhow::Result<Value> {
        let relative = args.get("path").and_then(Value::as_str).ok_or_else(|| anyhow::anyhow!("missing required 'path' argument"))?;
        let path = inside(&self.root, relative)?;
        let size = std::fs::metadata(&path)?.len();
        anyhow::ensure!(size <= MAX_READ_BYTES, "'{relative}' is {size} bytes; the most this reads is {MAX_READ_BYTES}");
        let bytes = std::fs::read(&path)?;
        let content = String::from_utf8(bytes).map_err(|_| anyhow::anyhow!("'{relative}' is not a text file"))?;
        Ok(json!({ "content": content }))
    }
}

/// Writes (creates or overwrites) a text file in the working folder.
pub struct FolderWriteTool {
    root: PathBuf,
}

impl FolderWriteTool {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

#[async_trait]
impl Tool for FolderWriteTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "write_file".to_string(),
            description: "Write (create or overwrite) a text file in the working folder at the given path relative to that folder.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path relative to the working folder, e.g. 'notes/todo.md'" },
                    "content": { "type": "string", "description": "Full file content to write" }
                },
                "required": ["path", "content"]
            }),
        }
    }

    async fn call(&self, args: Value) -> anyhow::Result<Value> {
        let relative = args.get("path").and_then(Value::as_str).ok_or_else(|| anyhow::anyhow!("missing required 'path' argument"))?;
        let content = args.get("content").and_then(Value::as_str).ok_or_else(|| anyhow::anyhow!("missing required 'content' argument"))?;
        let path = inside(&self.root, relative)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // The folders just made are new and plain, but check again: a path that did not exist a moment ago is not
        // a reason to trust what `create_dir_all` ended up following.
        let path = inside(&self.root, relative)?;
        std::fs::write(&path, content)?;
        Ok(json!({ "status": "ok" }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder() -> PathBuf {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("warden-folder-tools-{}-{n}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn a_file_is_written_and_read_back_inside_the_folder_and_nothing_else_is_made() {
        let root = folder();
        FolderWriteTool::new(&root).call(json!({ "path": "docs/a.md", "content": "hello" })).await.unwrap();
        assert_eq!(std::fs::read_to_string(root.join("docs/a.md")).unwrap(), "hello");
        let read = FolderReadTool::new(&root).call(json!({ "path": "docs/a.md" })).await.unwrap();
        assert_eq!(read["content"], "hello");
        let mut names: Vec<_> = std::fs::read_dir(&root).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        assert_eq!(names, ["docs"], "no index or marker file is left in the person's folder");
    }

    #[tokio::test]
    async fn a_path_that_leaves_the_folder_is_refused() {
        let root = folder();
        let outside = folder();
        std::fs::write(outside.join("secret.txt"), "secret").unwrap();
        let read = FolderReadTool::new(&root);
        let write = FolderWriteTool::new(&root);
        for path in ["../secret.txt", "/etc/hostname", "a/../../x", "", "a\\b"] {
            assert!(read.call(json!({ "path": path })).await.is_err(), "{path}");
            assert!(write.call(json!({ "path": path, "content": "x" })).await.is_err(), "{path}");
        }
        assert!(!outside.join("x").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_symlink_that_points_out_of_the_folder_is_not_followed() {
        let root = folder();
        let outside = folder();
        std::fs::write(outside.join("secret.txt"), "secret").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
        std::os::unix::fs::symlink(outside.join("secret.txt"), root.join("file-link")).unwrap();
        let read = FolderReadTool::new(&root);
        let write = FolderWriteTool::new(&root);
        assert!(read.call(json!({ "path": "link/secret.txt" })).await.is_err());
        assert!(read.call(json!({ "path": "file-link" })).await.is_err());
        assert!(write.call(json!({ "path": "link/new.txt", "content": "x" })).await.is_err());
        assert!(write.call(json!({ "path": "file-link", "content": "changed" })).await.is_err());
        assert_eq!(std::fs::read_to_string(outside.join("secret.txt")).unwrap(), "secret");
        assert!(!outside.join("new.txt").exists());
    }

    #[tokio::test]
    async fn a_file_too_big_or_not_text_is_not_read() {
        let root = folder();
        std::fs::write(root.join("big.txt"), vec![b'a'; MAX_READ_BYTES as usize + 1]).unwrap();
        std::fs::write(root.join("bin.dat"), [0xff, 0xfe, 0x00]).unwrap();
        let read = FolderReadTool::new(&root);
        assert!(read.call(json!({ "path": "big.txt" })).await.unwrap_err().to_string().contains("the most this reads"));
        assert!(read.call(json!({ "path": "bin.dat" })).await.unwrap_err().to_string().contains("not a text file"));
    }
}
