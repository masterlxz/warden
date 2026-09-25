//! Browsing and editing the vault from a screen (P78, and the desktop's Vault view) — the tree a
//! person sees, and reads/saves guarded by a content version so an edit never silently overwrites
//! what the model, sync or another screen wrote in the meantime.
//!
//! Stricter than `Vault::read`/`write` on purpose: the paths here are typed by a person (or sent
//! by a browser), so dotfiles (`.warden/`, `.syncignore`), `skills/` (managed by its own screen,
//! with its own validation) and anything reached through a symlink pointing out of the vault are
//! all refused, not just `..`.

use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

use super::{is_fixed_vault_file, Vault, SKILLS_DIR};

/// Largest note these screens open or save. Notes are text a person edits in a textarea; anything
/// bigger is almost certainly not one, and would also be a heavy message over the hub's socket.
pub const MAX_NOTE_BYTES: usize = 1024 * 1024;

/// A note's content plus the version to send back when saving an edit of it.
#[derive(Debug, Clone, PartialEq)]
pub struct NoteFile {
    pub content: String,
    pub version: String,
}

/// The note changed on disk since it was opened (or appeared/disappeared) — the editor should offer
/// to reload instead of overwriting. A distinct type so callers can tell it apart from other errors
/// with `downcast_ref`.
#[derive(Debug)]
pub struct NoteConflict(pub String);

impl std::fmt::Display for NoteConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for NoteConflict {}

/// A note's version: the SHA-256 of its bytes, hex. Content-based rather than a modification time,
/// so a write that puts back the same text (or a coarse filesystem clock) never causes a false
/// conflict.
pub fn content_version(content: &[u8]) -> String {
    Sha256::digest(content).iter().map(|b| format!("{b:02x}")).collect()
}

impl Vault {
    /// Every file a person can browse, relative to the vault root with `/` separators, sorted:
    /// everything `list_all_files` returns except the fixed files at the root (screens show those
    /// in their own section) and the root `skills/` folder.
    pub fn browse_files(&self) -> anyhow::Result<Vec<String>> {
        let mut files: Vec<String> = self
            .list_all_files()?
            .into_iter()
            .filter(|path| !is_fixed_vault_file(Path::new(""), path) && !starts_with_skills_dir(path))
            .map(|path| path.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/"))
            .collect();
        files.sort();
        Ok(files)
    }

    /// Opens a note for viewing or editing.
    pub fn read_note(&self, relative_path: &str) -> anyhow::Result<NoteFile> {
        let path = self.note_path(relative_path)?;
        let bytes = read_capped(&path, relative_path)?.ok_or_else(|| anyhow::anyhow!("'{relative_path}' doesn't exist"))?;
        let version = content_version(&bytes);
        let content = String::from_utf8(bytes).map_err(|_| anyhow::anyhow!("'{relative_path}' is not a text file"))?;
        Ok(NoteFile { content, version })
    }

    /// Saves a note, returning its new version. `expected_version` is the version it was opened at:
    /// `None` creates a new note and fails if the path is taken; `Some` fails with `NoteConflict`
    /// if the note changed or was deleted since.
    pub fn save_note(&self, relative_path: &str, content: &str, expected_version: Option<&str>) -> anyhow::Result<String> {
        let path = self.note_path(relative_path)?;
        if content.len() > MAX_NOTE_BYTES {
            anyhow::bail!("a note can have at most {} KB", MAX_NOTE_BYTES / 1024);
        }

        let _guard = self.note_lock.lock().unwrap_or_else(|e| e.into_inner());
        let current = read_capped(&path, relative_path)?.map(|bytes| content_version(&bytes));
        match (expected_version, current.as_deref()) {
            (None, None) => {}
            (None, Some(_)) => return Err(NoteConflict(format!("'{relative_path}' already exists")).into()),
            (Some(_), None) => return Err(NoteConflict(format!("'{relative_path}' was deleted after it was opened")).into()),
            (Some(expected), Some(current)) if expected != current => {
                return Err(NoteConflict(format!("'{relative_path}' changed after it was opened")).into())
            }
            (Some(_), Some(_)) => {}
        }

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        write_atomically(&path, content.as_bytes())?;
        Ok(content_version(content.as_bytes()))
    }

    /// Deletes a note, unless it changed since it was opened at `expected_version`. Already gone
    /// counts as done.
    pub fn delete_note(&self, relative_path: &str, expected_version: &str) -> anyhow::Result<()> {
        let path = self.note_path(relative_path)?;
        let _guard = self.note_lock.lock().unwrap_or_else(|e| e.into_inner());
        let Some(bytes) = read_capped(&path, relative_path)? else {
            return Ok(());
        };
        if content_version(&bytes) != expected_version {
            return Err(NoteConflict(format!("'{relative_path}' changed after it was opened")).into());
        }
        Ok(std::fs::remove_file(path)?)
    }

    /// Checks a path a person asked for — see the module docs for what's refused — and returns
    /// where it lives on disk.
    fn note_path(&self, relative_path: &str) -> anyhow::Result<PathBuf> {
        let invalid = || anyhow::anyhow!("'{relative_path}' is not a valid note path");
        let relative = Path::new(relative_path);
        if relative_path.is_empty() || relative_path.contains('\\') || relative_path.len() > 512 {
            return Err(invalid());
        }
        for component in relative.components() {
            match component {
                Component::Normal(name) if !name.to_string_lossy().starts_with('.') => {}
                _ => return Err(invalid()),
            }
        }
        if starts_with_skills_dir(relative) {
            anyhow::bail!("skills are edited on the Skills screen");
        }

        let path = self.root.join(relative);
        // A symlink inside the vault may point anywhere. Resolve the deepest part of the path that
        // exists (the note itself, or the folder it will be created in) and require it to still
        // be under the resolved vault root.
        let root = self.root.canonicalize()?;
        let existing = path.ancestors().find(|p| p.symlink_metadata().is_ok()).unwrap_or(&self.root);
        if !existing.canonicalize()?.starts_with(&root) {
            return Err(invalid());
        }
        Ok(path)
    }
}

fn starts_with_skills_dir(relative: &Path) -> bool {
    relative.components().next().is_some_and(|c| c.as_os_str() == SKILLS_DIR) && relative.components().count() > 1
}

/// The file's bytes, `None` if it doesn't exist, or an error if it's over `MAX_NOTE_BYTES`.
fn read_capped(path: &Path, relative_path: &str) -> anyhow::Result<Option<Vec<u8>>> {
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_dir() => anyhow::bail!("'{relative_path}' is a folder"),
        Ok(meta) if meta.len() > MAX_NOTE_BYTES as u64 => {
            anyhow::bail!("'{relative_path}' is too large to open here ({} KB)", meta.len() / 1024)
        }
        Ok(_) => Ok(Some(std::fs::read(path)?)),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err.into()),
    }
}

/// Writes through a temporary dotfile next to the note and renames it over, so a reader (sync, the
/// model's search) never sees a half-written note. Dotfiles are skipped by every vault listing.
fn write_atomically(path: &Path, content: &[u8]) -> anyhow::Result<()> {
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let tmp = path.with_file_name(format!(".{name}.warden-tmp"));
    std::fs::write(&tmp, content)?;
    if let Err(err) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(err.into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_vault() -> Vault {
        Vault::new(std::env::temp_dir().join(format!(
            "warden-notes-test-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        )))
    }

    fn conflict(err: anyhow::Error) -> bool {
        err.downcast_ref::<NoteConflict>().is_some()
    }

    #[test]
    fn browse_hides_root_fixed_files_skills_and_dotfiles_but_keeps_nested_namesakes() {
        let vault = temp_vault();
        for path in ["_profile.md", "a.md", "notes/b.md", "notes/_profile.md", "notes/skills/c.md", "skills/review.md", "img.png", ".syncignore"] {
            vault.write(path, "x").unwrap();
        }
        assert_eq!(vault.browse_files().unwrap(), vec!["a.md", "img.png", "notes/_profile.md", "notes/b.md", "notes/skills/c.md"]);
    }

    #[test]
    fn create_edit_and_delete_follow_the_version() {
        let vault = temp_vault();
        let v1 = vault.save_note("notes/new.md", "one", None).unwrap();
        assert_eq!(vault.read_note("notes/new.md").unwrap(), NoteFile { content: "one".into(), version: v1.clone() });

        let v2 = vault.save_note("notes/new.md", "two", Some(&v1)).unwrap();
        assert_ne!(v1, v2);
        assert!(conflict(vault.save_note("notes/new.md", "stale", Some(&v1)).unwrap_err()));
        assert!(conflict(vault.delete_note("notes/new.md", &v1).unwrap_err()));
        assert_eq!(vault.read("notes/new.md").unwrap(), "two");

        vault.delete_note("notes/new.md", &v2).unwrap();
        assert!(vault.read_note("notes/new.md").is_err());
        vault.delete_note("notes/new.md", &v2).unwrap(); // already gone
    }

    #[test]
    fn creating_over_an_existing_note_or_saving_a_deleted_one_is_a_conflict() {
        let vault = temp_vault();
        vault.write("a.md", "written by the model").unwrap();
        assert!(conflict(vault.save_note("a.md", "mine", None).unwrap_err()));

        let version = vault.read_note("a.md").unwrap().version;
        vault.delete("a.md").unwrap();
        assert!(conflict(vault.save_note("a.md", "mine", Some(&version)).unwrap_err()));
        assert!(vault.read("a.md").is_err());
    }

    #[test]
    fn a_change_made_outside_is_detected_but_rewriting_the_same_text_is_not() {
        let vault = temp_vault();
        let version = vault.save_note("a.md", "same", None).unwrap();
        vault.write("a.md", "same").unwrap();
        vault.save_note("a.md", "edited", Some(&version)).unwrap();
    }

    #[test]
    fn fixed_files_are_editable() {
        let vault = temp_vault();
        vault.write("_profile.md", "# Perfil").unwrap();
        let note = vault.read_note("_profile.md").unwrap();
        vault.save_note("_profile.md", "# Perfil\n\nName: Ada", Some(&note.version)).unwrap();
        assert!(vault.read("_profile.md").unwrap().contains("Ada"));
    }

    #[test]
    fn unsafe_paths_are_refused() {
        let vault = temp_vault();
        for bad in ["", "../x.md", "/etc/passwd", "a/../../x.md", ".warden/semantic_index.json", "notes/.hidden.md", "skills/review.md", "a\\b.md", "./a.md"] {
            assert!(vault.read_note(bad).is_err(), "{bad}");
            assert!(vault.save_note(bad, "x", None).is_err(), "{bad}");
        }
        // A folder named `skills` that isn't the root one is an ordinary folder.
        vault.save_note("notes/skills/x.md", "fine", None).unwrap();
        // A root note that happens to be named `skills` isn't the skills folder either.
        vault.save_note("skills", "fine", None).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_out_of_the_vault_is_refused() {
        let vault = temp_vault();
        let outside = std::env::temp_dir().join(format!("{}-outside", vault.root().file_name().unwrap().to_string_lossy()));
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.md"), "secret").unwrap();
        std::os::unix::fs::symlink(&outside, vault.root().join("link")).unwrap();

        assert!(vault.read_note("link/secret.md").is_err());
        assert!(vault.save_note("link/new.md", "x", None).is_err());
        assert!(!outside.join("new.md").exists());
        std::fs::remove_dir_all(outside).unwrap();
    }

    #[test]
    fn binary_folders_and_oversized_content_are_refused() {
        let vault = temp_vault();
        std::fs::write(vault.root().join("img.png"), [0xff, 0xfe, 0x00]).unwrap();
        assert!(vault.read_note("img.png").unwrap_err().to_string().contains("not a text file"));

        vault.write("notes/a.md", "x").unwrap();
        assert!(vault.read_note("notes").unwrap_err().to_string().contains("folder"));

        let big = "x".repeat(MAX_NOTE_BYTES + 1);
        assert!(vault.save_note("big.md", &big, None).is_err());
        std::fs::write(vault.root().join("big.md"), &big).unwrap();
        assert!(vault.read_note("big.md").unwrap_err().to_string().contains("too large"));
    }

    #[test]
    fn a_save_leaves_no_temporary_file_behind() {
        let vault = temp_vault();
        vault.save_note("notes/a.md", "x", None).unwrap();
        assert_eq!(std::fs::read_dir(vault.root().join("notes")).unwrap().count(), 1);
    }
}
