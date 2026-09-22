//! `.syncignore` (P75) — an optional, gitignore-style pattern list at the vault root letting the
//! user keep some vault content local-only, never leaving via push and never arriving via pull.
//! Dot-prefixed on purpose: `Vault::list_all_files` (`crates/warden-core/src/memory/mod.rs`)
//! already skips any dotfile/dot-directory, so `.syncignore` is invisible to `diff::diff_vault`
//! without any special-casing — it can never end up excluding itself.
//!
//! Deliberately simple compared to real `.gitignore`: no negation (`!pattern`), no `**` needed
//! since a bare pattern (no `/`) already matches at any depth. Nothing today asks for more than
//! that.

use std::path::Path;

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use warden_core::memory::Vault;

pub const SYNC_IGNORE_FILE: &str = ".syncignore";

pub struct SyncIgnore {
    set: GlobSet,
    pattern_count: usize,
}

impl SyncIgnore {
    pub fn empty() -> Self {
        Self { set: GlobSetBuilder::new().build().expect("empty GlobSet always builds"), pattern_count: 0 }
    }

    /// Loads `.syncignore` from `vault`'s root. A missing file — the common case, most vaults
    /// never have one — is not an error, same posture as `manifest::load_secrets`/`load_manifest`
    /// for their own missing-file case.
    pub fn load(vault: &Vault) -> anyhow::Result<Self> {
        Self::load_from(&vault.root().join(SYNC_IGNORE_FILE))
    }

    fn load_from(path: &Path) -> anyhow::Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(contents) => Self::parse(&contents),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::empty()),
            Err(err) => Err(err.into()),
        }
    }

    pub fn parse(contents: &str) -> anyhow::Result<Self> {
        let mut builder = GlobSetBuilder::new();
        let mut pattern_count = 0;
        for line in contents.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            // `literal_separator(true)` so a bare `*` stops at `/`, same as `.gitignore` — only
            // the explicit `**` (added by `normalize_pattern` for a depth-agnostic pattern, or
            // written directly by the user) crosses directory boundaries.
            builder.add(GlobBuilder::new(&normalize_pattern(line)).literal_separator(true).build()?);
            pattern_count += 1;
        }
        Ok(Self { set: builder.build()?, pattern_count })
    }

    pub fn pattern_count(&self) -> usize {
        self.pattern_count
    }

    /// `relative` is expected in the same form every path this crate diffs/bundles already uses —
    /// `PathBuf::to_string_lossy()` on the relative path `Vault::list_all_files` returns.
    pub fn matches(&self, relative: &str) -> bool {
        self.pattern_count > 0 && self.set.is_match(relative)
    }
}

/// A pattern with no leading `/` and no `/` matches at any depth (like a bare `.gitignore` entry)
/// — rewritten as `**/<pattern>`. A pattern ending in `/` means "this whole folder" — rewritten to
/// also match everything under it. A leading `/` anchors to the vault root: stripped, and
/// deliberately *not* given the `**/` treatment even though what's left may now be slash-free.
fn normalize_pattern(pattern: &str) -> String {
    let anchored = pattern.starts_with('/');
    let pattern = pattern.strip_prefix('/').unwrap_or(pattern);
    let pattern = match pattern.strip_suffix('/') {
        Some(dir) => format!("{dir}/**"),
        None => pattern.to_string(),
    };
    if anchored || pattern.contains('/') {
        pattern
    } else {
        format!("**/{pattern}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_contents_match_nothing() {
        let ignore = SyncIgnore::parse("").unwrap();
        assert_eq!(ignore.pattern_count(), 0);
        assert!(!ignore.matches("notes/todo.md"));
    }

    #[test]
    fn comments_and_blank_lines_are_skipped() {
        let ignore = SyncIgnore::parse("# comment\n\n  \nsecret.md\n").unwrap();
        assert_eq!(ignore.pattern_count(), 1);
        assert!(ignore.matches("secret.md"));
    }

    #[test]
    fn pattern_without_slash_matches_at_any_depth() {
        let ignore = SyncIgnore::parse("secret.md").unwrap();
        assert!(ignore.matches("secret.md"));
        assert!(ignore.matches("notes/secret.md"));
        assert!(ignore.matches("a/b/c/secret.md"));
        assert!(!ignore.matches("notes/other.md"));
    }

    #[test]
    fn trailing_slash_matches_the_whole_folder() {
        let ignore = SyncIgnore::parse("private/").unwrap();
        assert!(ignore.matches("private/notes.md"));
        assert!(ignore.matches("private/nested/notes.md"));
        assert!(!ignore.matches("public/notes.md"));
        assert!(!ignore.matches("private_but_not_the_folder.md"));
    }

    #[test]
    fn leading_slash_anchors_to_the_root() {
        let ignore = SyncIgnore::parse("/root-only.md").unwrap();
        assert!(ignore.matches("root-only.md"));
        assert!(!ignore.matches("nested/root-only.md"));
    }

    #[test]
    fn glob_wildcards_work() {
        let ignore = SyncIgnore::parse("scratch/*.tmp").unwrap();
        assert!(ignore.matches("scratch/a.tmp"));
        assert!(!ignore.matches("scratch/nested/a.tmp"));
    }

    #[test]
    fn missing_file_is_empty_not_an_error() {
        let path = std::env::temp_dir().join(format!(
            "warden-sync-syncignore-test-missing-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        let ignore = SyncIgnore::load_from(&path).unwrap();
        assert_eq!(ignore.pattern_count(), 0);
    }
}
