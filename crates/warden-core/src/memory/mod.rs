mod cipher;
mod notes;
#[cfg(feature = "semantic-search")]
mod semantic;

pub use cipher::{base32_decode, base32_encode, VaultCipher, MAX_NAME_BYTES};
pub use notes::{content_version, NoteConflict, NoteFile, MAX_NOTE_BYTES};

#[cfg(feature = "semantic-search")]
use fastembed::{EmbeddingModel, TextEmbedding, TextInitOptions};
#[cfg(feature = "semantic-search")]
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

/// Markdown vault on local disk (Obsidian-compatible). IPFS mirroring lands in Phase 4.
pub struct Vault {
    root: PathBuf,
    /// Lazily loaded on first semantic search (downloads the ONNX model on first-ever use) and
    /// reused after that — `Vault` is held behind `Arc` for the life of the process, so this loads
    /// at most once per process, not once per turn. Behind the `semantic-search` feature (default
    /// on) — off for crates that only need `Vault` for file I/O (e.g. `warden-sync`), since `ort`
    /// (fastembed's ONNX runtime) has no prebuilt binary for some cross-compile targets those
    /// crates build for (Android's `armv7-linux-androideabi`, via `warden-mobile-bridge`).
    #[cfg(feature = "semantic-search")]
    embedder: Mutex<Option<TextEmbedding>>,
    /// Guards the read-refresh-write cycle of the on-disk semantic index (`.warden/semantic_index.json`)
    /// against two turns (e.g. two channels sharing one `warden-server` vault) racing each other.
    #[cfg(feature = "semantic-search")]
    index_lock: Mutex<()>,
    /// Makes `save_note`/`delete_note`'s version check and write one step (P78), so two editors
    /// saving the same note at once can't both pass the check.
    note_lock: Mutex<()>,
    /// Other vaults shown inside this one under `MOUNTS_DIR/<prefix>/` (P84: the owner's shared
    /// folders in a member's vault). `None` for an ordinary vault, where `MOUNTS_DIR` is just a
    /// folder like any other; `Some` (even empty) makes that folder the mounts' alone.
    mounts: RwLock<Option<Vec<Mount>>>,
    /// A member's vault (P84, fatia 4): what every file holds, and every folder and file name, is
    /// encrypted on disk. `None` for the owner's vault and for a shared folder, which stay plain.
    /// Paths given to and returned by this type are always the readable ones; only `path_of`
    /// (where a file is on disk) shows the encrypted spelling.
    cipher: Option<Arc<VaultCipher>>,
    /// A member's vault whose key the hub doesn't hold (it restarted since they last signed in):
    /// everything is refused, so nothing readable is ever written next to the encrypted files.
    locked: bool,
}

/// What a locked vault (or conversation folder) answers.
pub const LOCKED_MESSAGE: &str = "your data is locked: sign in with your password once on this hub, which restarted since you last did";

/// Where a vault shows its mounts (P84): `compartilhado/<space>/…`.
pub const MOUNTS_DIR: &str = "compartilhado";

/// Another vault shown inside this one at `MOUNTS_DIR/<prefix>/` — a shared space (P84). Paths
/// under it go to `vault`, relative to its own root, so its `..`/absolute-path guard still holds.
#[derive(Clone)]
pub struct Mount {
    pub prefix: String,
    pub vault: Arc<Vault>,
    /// `false`: reading only; writing and deleting are refused.
    pub writable: bool,
}

/// One matching line from `Vault::search`, with enough location info to cite it.
pub struct SearchHit {
    pub path: String,
    pub line_number: usize,
    pub line: String,
}

/// Reserved vault-root filenames for the "fixed/standard" memory (P52) — always injected via
/// `Vault::standing_memory`, never surfaced through `search`/`search_semantic` (would otherwise
/// double up with the standing-memory block and eat into the free-form notes' hit budget).
/// `warden-bootstrap::seed_default_vault_files` seeds these with a starter template on first use.
pub const FIXED_VAULT_FILES: [&str; 3] = ["_profile.md", "_behavior.md", "_feedback.md"];

/// Vault-root directory holding skills (P16), one `<name>.md` each — see `crate::skill`. Like the
/// fixed files, kept out of `list_files`/`search`/`search_semantic` (a skill's body is instructions,
/// loaded on demand through `use_skill`, not a memory to surface as a hit) but still returned by
/// `list_all_files`, so sync carries skills across devices without any change to `warden-sync`.
pub const SKILLS_DIR: &str = "skills";

impl Vault {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self::build(root.into(), None)
    }

    /// A vault whose files and names are encrypted on disk with `cipher` (P84, fatia 4).
    pub fn new_encrypted(root: impl Into<PathBuf>, cipher: Arc<VaultCipher>) -> Self {
        Self::build(root.into(), Some(cipher))
    }

    /// The vault of a member whose data is encrypted but whose key isn't here: refuses everything
    /// with `LOCKED_MESSAGE`.
    pub fn new_locked(root: impl Into<PathBuf>) -> Self {
        Self { locked: true, ..Self::build(root.into(), None) }
    }

    fn build(root: PathBuf, cipher: Option<Arc<VaultCipher>>) -> Self {
        let _ = std::fs::create_dir_all(&root);
        Self {
            cipher,
            locked: false,
            root,
            #[cfg(feature = "semantic-search")]
            embedder: Mutex::new(None),
            #[cfg(feature = "semantic-search")]
            index_lock: Mutex::new(()),
            note_lock: Mutex::new(()),
            mounts: RwLock::new(None),
        }
    }

    /// Replaces the vaults shown under `MOUNTS_DIR` — from now on that folder is only theirs: a
    /// path under it that isn't one of them is refused.
    pub fn set_mounts(&self, mounts: Vec<Mount>) {
        *self.mounts.write().unwrap_or_else(|e| e.into_inner()) = Some(mounts);
    }

    fn current_mounts(&self) -> Option<Vec<Mount>> {
        self.mounts.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// The mount `relative_path` is in and the path inside it, `None` for a path of this vault's
    /// own, or an error for one under `MOUNTS_DIR` that no mount answers.
    pub(crate) fn route(&self, relative_path: &str) -> anyhow::Result<Option<(Mount, String)>> {
        let Some(mounts) = self.current_mounts() else { return Ok(None) };
        let normalized = relative_path.trim_start_matches("./");
        let Some(rest) = normalized.strip_prefix(MOUNTS_DIR) else { return Ok(None) };
        let Some(rest) = rest.strip_prefix('/') else {
            if rest.is_empty() {
                anyhow::bail!("'{MOUNTS_DIR}' holds the shared spaces — pick a file inside one");
            }
            return Ok(None); // e.g. `compartilhados.md`: a sibling, not the folder
        };
        let (space, inner) = rest.split_once('/').unwrap_or((rest, ""));
        let mount = mounts.into_iter().find(|m| m.prefix == space).ok_or_else(|| anyhow::anyhow!("there's no shared space '{space}' for you"))?;
        anyhow::ensure!(!inner.is_empty(), "'{relative_path}' is a shared space, not a file in it");
        Ok(Some((mount, inner.to_string())))
    }

    fn writable_route(&self, relative_path: &str) -> anyhow::Result<Option<(Mount, String)>> {
        let routed = self.route(relative_path)?;
        if let Some((mount, _)) = &routed {
            anyhow::ensure!(mount.writable, "the shared space '{}' is read-only for you", mount.prefix);
        }
        Ok(routed)
    }

    pub fn root(&self) -> &PathBuf {
        &self.root
    }

    /// Where `relative_path` lives on disk, refusing anything that would land outside the vault: an
    /// absolute path, or one with a `..` component. The paths reaching `read`/`write`/`delete` come
    /// from the model's tools, from another node (P61) and from a sync bundle, so none of them can
    /// be trusted to stay inside on their own.
    pub fn path_of(&self, relative_path: &str) -> anyhow::Result<PathBuf> {
        let relative = Path::new(relative_path);
        if relative_path.is_empty() || !relative.components().all(|c| matches!(c, Component::Normal(_) | Component::CurDir)) {
            anyhow::bail!("'{relative_path}' is not a path inside the vault");
        }
        self.physical(relative)
    }

    /// Where a path already checked (only normal components) is on disk: the same path for a plain
    /// vault, every folder and file name encrypted for a member's.
    pub(crate) fn physical(&self, relative: &Path) -> anyhow::Result<PathBuf> {
        anyhow::ensure!(!self.locked, LOCKED_MESSAGE);
        let Some(cipher) = &self.cipher else { return Ok(self.root.join(relative)) };
        let mut path = self.root.clone();
        for component in relative.components() {
            if let Component::Normal(name) = component {
                path.push(cipher.seal_name(&name.to_string_lossy())?);
            }
        }
        Ok(path)
    }

    pub fn is_encrypted(&self) -> bool {
        self.cipher.is_some()
    }

    /// The cipher of an encrypted vault: what a tool that writes files for this person (outside the
    /// vault) uses to keep them encrypted too.
    pub fn cipher(&self) -> Option<Arc<VaultCipher>> {
        self.cipher.clone()
    }

    /// What `bytes` look like on disk.
    pub(crate) fn encode(&self, bytes: &[u8]) -> Vec<u8> {
        match &self.cipher {
            Some(cipher) => cipher.seal(bytes),
            None => bytes.to_vec(),
        }
    }

    /// What a file on disk holds, decrypted for a member's vault.
    pub(crate) fn decode(&self, bytes: Vec<u8>) -> anyhow::Result<Vec<u8>> {
        match &self.cipher {
            Some(cipher) => cipher.open(&bytes),
            None => Ok(bytes),
        }
    }

    /// The readable name of an entry found on disk, `None` for one this vault didn't write (an
    /// encrypted vault ignores anything that isn't an encrypted name).
    fn readable_name(&self, on_disk: &str) -> Option<String> {
        match &self.cipher {
            Some(cipher) => cipher.open_name(on_disk),
            None => Some(on_disk.to_string()),
        }
    }

    fn read_file(&self, path: &Path) -> anyhow::Result<Vec<u8>> {
        self.decode(std::fs::read(path)?)
    }

    fn write_file(&self, path: &Path, content: &[u8]) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Ok(std::fs::write(path, self.encode(content))?)
    }

    /// A markdown file of this vault, read as text.
    fn read_own_text(&self, relative: &Path) -> anyhow::Result<String> {
        Ok(String::from_utf8(self.read_file(&self.physical(relative)?)?)?)
    }

    pub fn read(&self, relative_path: &str) -> anyhow::Result<String> {
        if let Some((mount, inner)) = self.route(relative_path)? {
            return mount.vault.read(&inner);
        }
        Ok(String::from_utf8(self.read_file(&self.path_of(relative_path)?)?)?)
    }

    pub fn write(&self, relative_path: &str, content: &str) -> anyhow::Result<()> {
        if let Some((mount, inner)) = self.writable_route(relative_path)? {
            return mount.vault.write(&inner, content);
        }
        self.write_file(&self.path_of(relative_path)?, content.as_bytes())
    }

    /// Removes a file from the vault — how `warden-sync`'s bundle-apply (P37) deletes a note another
    /// device removed.
    pub fn delete(&self, relative_path: &str) -> anyhow::Result<()> {
        if let Some((mount, inner)) = self.writable_route(relative_path)? {
            return mount.vault.delete(&inner);
        }
        Ok(std::fs::remove_file(self.path_of(relative_path)?)?)
    }

    /// Whether `relative_path` is a file of this vault (its own, not a mounted one).
    pub fn is_file(&self, relative_path: &str) -> bool {
        self.path_of(relative_path).is_ok_and(|p| p.is_file())
    }

    /// The names of the files directly inside a folder of this vault (not the folders, and not the
    /// mounted ones), in no particular order. Empty when the folder doesn't exist.
    pub fn files_in(&self, relative_dir: &str) -> anyhow::Result<Vec<String>> {
        let Ok(entries) = std::fs::read_dir(self.path_of(relative_dir)?) else { return Ok(Vec::new()) };
        Ok(entries
            .flatten()
            .filter(|entry| entry.path().is_file())
            .filter_map(|entry| self.readable_name(&entry.file_name().to_string_lossy()))
            .collect())
    }

    /// Removes a folder of this vault with everything in it. A folder that isn't there counts as done.
    pub fn remove_dir_all(&self, relative_dir: &str) -> anyhow::Result<()> {
        match std::fs::remove_dir_all(self.path_of(relative_dir)?) {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err.into()),
            _ => Ok(()),
        }
    }

    /// Removes a folder of this vault only if nothing is left in it.
    pub fn remove_dir_if_empty(&self, relative_dir: &str) {
        if let Ok(path) = self.path_of(relative_dir) {
            let _ = std::fs::remove_dir(path);
        }
    }

    /// All markdown files in the vault, relative to its root — the mounted ones too, under
    /// `MOUNTS_DIR/<prefix>/`.
    pub fn list_files(&self) -> anyhow::Result<Vec<PathBuf>> {
        let mut files = self.own_markdown_files()?;
        for mount in self.current_mounts().unwrap_or_default() {
            let prefix = Path::new(MOUNTS_DIR).join(&mount.prefix);
            files.extend(mount.vault.list_files()?.into_iter().map(|f| prefix.join(f)));
        }
        Ok(files)
    }

    /// This vault's own markdown files — with mounts, a real `MOUNTS_DIR` folder here is left out
    /// (it's the mounts' place).
    fn own_markdown_files(&self) -> anyhow::Result<Vec<PathBuf>> {
        let mut files = Vec::new();
        self.collect_files(&self.root, Path::new(""), true, &mut files)?;
        if self.current_mounts().is_some() {
            files.retain(|f| !f.starts_with(MOUNTS_DIR));
        }
        Ok(files)
    }

    /// Every regular file in the vault, any extension — unlike `list_files`, which only returns
    /// `.md` (the memory `search` reads). Used by sync (Fase 4/P37), which mirrors the whole
    /// vault, not just the markdown subset. Skips dotfiles/dot-directories (OS/editor cruft like
    /// `.DS_Store`, `.git`) — same "good enough for v1" posture as `search`'s naive grep.
    pub fn list_all_files(&self) -> anyhow::Result<Vec<PathBuf>> {
        let mut files = Vec::new();
        self.collect_files(&self.root, Path::new(""), false, &mut files)?;
        Ok(files)
    }

    /// Walks `dir` (on disk; `readable_dir` is the same folder as this vault names it) and pushes
    /// the files under it by their readable path. `markdown_only` is `list_files`' walk: `.md`
    /// files, without the fixed ones or `skills/`. Otherwise it's `list_all_files`': everything
    /// but dotfiles. Symlinks are never followed: one pointing out of the vault would put outside
    /// files in search results and sync, and one pointing at an ancestor would recurse until the
    /// OS path limit.
    fn collect_files(&self, dir: &Path, readable_dir: &Path, markdown_only: bool, out: &mut Vec<PathBuf>) -> anyhow::Result<()> {
        anyhow::ensure!(!self.locked, LOCKED_MESSAGE);
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let Some(name) = self.readable_name(&entry.file_name().to_string_lossy()) else { continue };
            let readable = readable_dir.join(&name);
            if entry.file_type()?.is_symlink() || (!markdown_only && name.starts_with('.')) {
                continue;
            }
            let path = entry.path();
            if path.is_dir() {
                if markdown_only && readable == Path::new(SKILLS_DIR) {
                    continue;
                }
                self.collect_files(&path, &readable, markdown_only, out)?;
            } else if !markdown_only || (readable.extension().and_then(|e| e.to_str()) == Some("md") && !is_fixed_vault_file(Path::new(""), &readable)) {
                out.push(readable);
            }
        }
        Ok(())
    }

    /// Naive grep: case-insensitive substring match on any query word (3+ chars)
    /// across every markdown file. Good enough for v1 memory retrieval — semantic
    /// search is Phase 4 territory (see ARCHITECTURE.md, PENDING.md P5/P6).
    pub fn search(&self, query: &str, max_hits: usize) -> anyhow::Result<Vec<SearchHit>> {
        let words: Vec<String> = query
            .split_whitespace()
            .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase())
            .filter(|w| w.len() >= 3)
            .collect();

        if words.is_empty() {
            return Ok(Vec::new());
        }

        let mut hits = Vec::new();
        for relative in self.own_markdown_files()? {
            if hits.len() >= max_hits {
                break;
            }
            let content = self.read_own_text(&relative)?;
            for (i, line) in content.lines().enumerate() {
                if hits.len() >= max_hits {
                    break;
                }
                let lower = line.to_lowercase();
                if words.iter().any(|w| lower.contains(w.as_str())) {
                    hits.push(SearchHit {
                        path: relative.to_string_lossy().to_string(),
                        line_number: i + 1,
                        line: line.to_string(),
                    });
                }
            }
        }
        for mount in self.current_mounts().unwrap_or_default() {
            if hits.len() >= max_hits {
                break;
            }
            let prefix = format!("{MOUNTS_DIR}/{}/", mount.prefix);
            hits.extend(mount.vault.search(query, max_hits - hits.len())?.into_iter().map(|h| SearchHit { path: format!("{prefix}{}", h.path), ..h }));
        }
        Ok(hits)
    }

    /// The "fixed/standard" memory block (P52) — unlike `search`/`search_semantic`, not keyed off
    /// any query: always reads `FIXED_VAULT_FILES` in order and returns one combined block, with a
    /// heading per file, skipping any that's missing or blank (an unseeded vault, or one where the
    /// user cleared a section on purpose). Returns an empty string when all three are empty/absent,
    /// so callers can skip injecting an empty system message.
    pub fn standing_memory(&self) -> String {
        const HEADINGS: [&str; 3] = ["User profile", "AI behavior", "Feedback / lessons learned"];
        let mut sections = Vec::new();
        for (name, heading) in FIXED_VAULT_FILES.iter().zip(HEADINGS) {
            if let Ok(content) = self.read(name) {
                let content = content.trim();
                if !content.is_empty() {
                    sections.push(format!("## {heading}\n\n{content}"));
                }
            }
        }
        sections.join("\n\n")
    }

    #[cfg(feature = "semantic-search")]
    /// Semantic counterpart to `search` — ranks markdown chunks by embedding similarity to `query`
    /// instead of substring match. Self-healing: re-hashes every chunk on each call and only
    /// re-embeds what's new or changed since the last call (including edits made outside the
    /// Warden app entirely, since the vault is a plain Obsidian-compatible directory), rather than
    /// hooking every write path. Returns the same `SearchHit` shape as `search`, so callers don't
    /// need to change — `line` becomes a chunk preview rather than the literal matched line.
    pub fn search_semantic(&self, query: &str, max_hits: usize) -> anyhow::Result<Vec<SearchHit>> {
        let own = self.search_semantic_own(query, max_hits)?;
        let mounts = self.current_mounts().unwrap_or_default();
        if mounts.is_empty() {
            return Ok(own);
        }
        // Each vault ranks its own chunks; their scores aren't comparable across indexes, so the
        // best of each take turns (own vault first) until `max_hits`.
        let mut lists = vec![own];
        for mount in mounts {
            let prefix = format!("{MOUNTS_DIR}/{}/", mount.prefix);
            lists.push(mount.vault.search_semantic(query, max_hits)?.into_iter().map(|h| SearchHit { path: format!("{prefix}{}", h.path), ..h }).collect());
        }
        let mut merged = Vec::new();
        let mut iters: Vec<_> = lists.into_iter().map(Vec::into_iter).collect();
        while merged.len() < max_hits {
            let before = merged.len();
            for it in &mut iters {
                if merged.len() < max_hits {
                    if let Some(hit) = it.next() {
                        merged.push(hit);
                    }
                }
            }
            if merged.len() == before {
                break;
            }
        }
        Ok(merged)
    }

    #[cfg(feature = "semantic-search")]
    fn search_semantic_own(&self, query: &str, max_hits: usize) -> anyhow::Result<Vec<SearchHit>> {
        if max_hits == 0 || query.trim().is_empty() {
            return Ok(Vec::new());
        }

        let _guard = self.index_lock.lock().unwrap();
        let index_path = self.physical(&Path::new(semantic::INDEX_DIR).join(semantic::INDEX_FILE))?;
        let index = semantic::SemanticIndex::from_bytes(self.read_file(&index_path).ok().as_deref());

        let mut wanted: Vec<(String, usize, String, String)> = Vec::new();
        for relative in self.own_markdown_files()? {
            let path = relative.to_string_lossy().to_string();
            let content = self.read_own_text(&relative)?;
            for (start_line, text) in semantic::chunk_file(&content, semantic::CHUNK_WINDOW_LINES) {
                let hash = semantic::hash_chunk(&text);
                wanted.push((path.clone(), start_line, hash, text));
            }
        }

        let existing: HashMap<(&str, usize), &semantic::IndexedChunk> =
            index.chunks.iter().map(|c| ((c.path.as_str(), c.start_line), c)).collect();

        let mut rebuilt = Vec::with_capacity(wanted.len());
        let mut pending: Vec<usize> = Vec::new();
        for (i, (path, start_line, hash, text)) in wanted.iter().enumerate() {
            match existing.get(&(path.as_str(), *start_line)) {
                Some(chunk) if chunk.hash == *hash => rebuilt.push((*chunk).clone()),
                _ => {
                    rebuilt.push(semantic::IndexedChunk {
                        path: path.clone(),
                        start_line: *start_line,
                        hash: hash.clone(),
                        preview: semantic::preview(text),
                        embedding: Vec::new(),
                    });
                    pending.push(i);
                }
            }
        }

        if !pending.is_empty() {
            let texts: Vec<&str> = pending.iter().map(|&i| wanted[i].3.as_str()).collect();
            let embeddings = self.with_embedder(|model| Ok(model.embed(texts, None)?))?;
            for (slot, embedding) in pending.into_iter().zip(embeddings) {
                rebuilt[slot].embedding = embedding;
            }
        }

        let mut index = index;
        index.chunks = rebuilt;
        if let Ok(bytes) = index.to_bytes() {
            let _ = self.write_file(&index_path, &bytes);
        }

        let query_embedding = self.with_embedder(|model| {
            Ok(model.embed(vec![query], None)?.into_iter().next().unwrap_or_default())
        })?;

        let mut scored: Vec<(f32, &semantic::IndexedChunk)> = index
            .chunks
            .iter()
            .filter(|c| !c.embedding.is_empty())
            .map(|c| (semantic::cosine_similarity(&query_embedding, &c.embedding), c))
            .collect();
        scored.sort_by(|a, b| b.0.total_cmp(&a.0));

        Ok(scored
            .into_iter()
            .take(max_hits)
            .map(|(_, c)| SearchHit { path: c.path.clone(), line_number: c.start_line, line: c.preview.clone() })
            .collect())
    }

    /// Lazily loads the local embedding model (downloads it on first-ever use — the one part of
    /// "local" search that needs network, exactly once per machine) and runs `f` against it.
    #[cfg(feature = "semantic-search")]
    fn with_embedder<T>(&self, f: impl FnOnce(&mut TextEmbedding) -> anyhow::Result<T>) -> anyhow::Result<T> {
        let mut guard = self.embedder.lock().unwrap();
        if guard.is_none() {
            let model = TextEmbedding::try_new(
                TextInitOptions::new(EmbeddingModel::AllMiniLML6V2)
                    .with_cache_dir(model_cache_dir())
                    .with_show_download_progress(false),
            )?;
            *guard = Some(model);
        }
        f(guard.as_mut().expect("just initialized above"))
    }
}

/// Where the (shared, not per-vault) ONNX model file lives once downloaded — not vault content,
/// so it must not live inside a vault directory or get duplicated per vault.
#[cfg(feature = "semantic-search")]
fn model_cache_dir() -> PathBuf {
    dirs::cache_dir().unwrap_or_else(std::env::temp_dir).join("warden").join("models")
}

/// True for `_profile.md`/`_behavior.md`/`_feedback.md` at the vault root specifically — a
/// same-named file nested under a subdirectory (e.g. a user's own `notes/_profile.md`) is a
/// regular note, not the reserved one, so only the root-level match is excluded from search.
fn is_fixed_vault_file(root: &Path, path: &Path) -> bool {
    path.parent() == Some(root)
        && path.file_name().and_then(|n| n.to_str()).is_some_and(|name| FIXED_VAULT_FILES.contains(&name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_vault() -> Vault {
        let dir = std::env::temp_dir().join(format!(
            "warden-vault-test-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        Vault::new(dir)
    }

    /// P84: a member's vault with the owner's `casa` folder read-only and `trip` writable.
    fn mounted() -> (Vault, Arc<Vault>, Arc<Vault>) {
        let owner = temp_vault();
        owner.write("casa/lista.md", "arroz e feijão").unwrap();
        owner.write("private/diary.md", "segredo").unwrap();
        let casa = Arc::new(Vault::new(owner.root().join("casa")));
        let trip = Arc::new(Vault::new(owner.root().join("trip")));
        let member = temp_vault();
        member.write("mine.md", "my own note about feijão").unwrap();
        member.set_mounts(vec![Mount { prefix: "casa".into(), vault: casa.clone(), writable: false }, Mount { prefix: "trip".into(), vault: trip.clone(), writable: true }]);
        (member, casa, trip)
    }

    #[test]
    fn mounted_spaces_read_list_and_search_under_their_prefix() {
        let (member, _, _) = mounted();
        assert_eq!(member.read("compartilhado/casa/lista.md").unwrap(), "arroz e feijão");
        let files: Vec<String> = member.list_files().unwrap().iter().map(|p| p.to_string_lossy().to_string()).collect();
        assert!(files.contains(&"mine.md".to_string()) && files.contains(&"compartilhado/casa/lista.md".to_string()), "{files:?}");
        assert!(!files.iter().any(|f| f.contains("diary")), "nothing outside the shared folder");
        let hits: Vec<String> = member.search("feijão", 10).unwrap().into_iter().map(|h| h.path).collect();
        assert_eq!(hits, ["mine.md", "compartilhado/casa/lista.md"]);
        assert!(member.browse_files().unwrap().contains(&"compartilhado/casa/lista.md".to_string()));
        assert!(member.list_all_files().unwrap().iter().all(|f| !f.starts_with(MOUNTS_DIR)), "sync only carries the vault's own files");
    }

    #[test]
    fn a_read_only_space_refuses_writes_and_a_writable_one_writes_through() {
        let (member, casa, trip) = mounted();
        assert!(member.write("compartilhado/casa/new.md", "x").is_err());
        assert!(member.delete("compartilhado/casa/lista.md").is_err());
        assert!(member.save_note("compartilhado/casa/new.md", "x", None).is_err());
        assert!(!casa.root().join("new.md").exists());
        member.write("compartilhado/trip/plan.md", "Lisboa").unwrap();
        assert_eq!(trip.read("plan.md").unwrap(), "Lisboa", "it lands in the owner's folder");
        let version = member.save_note("compartilhado/trip/notes.md", "one", None).unwrap();
        assert_eq!(member.read_note("compartilhado/trip/notes.md").unwrap().version, version);
    }

    #[test]
    fn the_shared_folder_is_the_mounts_alone_and_never_a_way_out() {
        let (member, _, _) = mounted();
        assert!(member.write("compartilhado/other/x.md", "x").is_err(), "no such space");
        assert!(member.read("compartilhado").is_err());
        assert!(member.read("compartilhado/casa/../../private/diary.md").is_err());
        assert!(member.read("compartilhado/casa/../casa/lista.md").is_err());
        member.write("compartilhados.md", "a sibling, not the folder").unwrap();

        // Changing the mounts changes the access at once.
        member.set_mounts(Vec::new());
        assert!(member.read("compartilhado/casa/lista.md").is_err());
        assert!(!member.list_files().unwrap().iter().any(|f| f.starts_with(MOUNTS_DIR)));
    }

    #[test]
    fn an_ordinary_vault_keeps_its_own_compartilhado_folder() {
        let vault = temp_vault();
        vault.write("compartilhado/receitas.md", "bolo").unwrap();
        assert_eq!(vault.read("compartilhado/receitas.md").unwrap(), "bolo");
        assert_eq!(vault.list_files().unwrap().len(), 1);
    }

    #[test]
    fn write_then_read_roundtrip() {
        let vault = temp_vault();
        vault.write("notes/todo.md", "buy milk").unwrap();
        assert_eq!(vault.read("notes/todo.md").unwrap(), "buy milk");
    }

    #[test]
    fn delete_removes_a_written_file() {
        let vault = temp_vault();
        vault.write("notes/todo.md", "buy milk").unwrap();
        vault.delete("notes/todo.md").unwrap();
        assert!(vault.read("notes/todo.md").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn listings_and_search_do_not_follow_symlinks() {
        let vault = temp_vault();
        let outside = vault.root().parent().unwrap().join(format!("{}-linked", vault.root().file_name().unwrap().to_string_lossy()));
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.md"), "outside dentist").unwrap();
        std::os::unix::fs::symlink(&outside, vault.root().join("out")).unwrap();
        std::os::unix::fs::symlink(outside.join("secret.md"), vault.root().join("file-link.md")).unwrap();
        // A link to the vault's own parent would loop forever if followed.
        std::os::unix::fs::symlink(vault.root().parent().unwrap(), vault.root().join("loop")).unwrap();
        vault.write("a.md", "inside dentist").unwrap();

        assert_eq!(vault.list_all_files().unwrap(), vec![PathBuf::from("a.md")]);
        assert_eq!(vault.list_files().unwrap(), vec![PathBuf::from("a.md")]);
        let hits = vault.search("dentist", 10).unwrap();
        assert_eq!(hits.iter().map(|h| h.path.as_str()).collect::<Vec<_>>(), vec!["a.md"]);
        std::fs::remove_dir_all(outside).unwrap();
    }

    #[test]
    fn read_write_and_delete_refuse_paths_outside_the_vault() {
        let vault = temp_vault();
        let outside = vault.root().parent().unwrap().join(format!("{}-outside.md", vault.root().file_name().unwrap().to_string_lossy()));
        std::fs::write(&outside, "secret").unwrap();
        let escaping = format!("../{}", outside.file_name().unwrap().to_string_lossy());

        for bad in [escaping.as_str(), outside.to_str().unwrap(), "notes/../../x.md", ""] {
            assert!(vault.read(bad).is_err(), "{bad}");
            assert!(vault.write(bad, "x").is_err(), "{bad}");
            assert!(vault.delete(bad).is_err(), "{bad}");
        }
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "secret");
        vault.write("./notes/ok.md", "fine").unwrap();
        assert_eq!(vault.read("notes/ok.md").unwrap(), "fine");
        std::fs::remove_file(outside).unwrap();
    }

    #[test]
    fn list_files_finds_nested_markdown_only() {
        let vault = temp_vault();
        vault.write("a.md", "one").unwrap();
        vault.write("nested/b.md", "two").unwrap();
        vault.write("nested/notes.txt", "ignored").unwrap();

        let mut files: Vec<String> =
            vault.list_files().unwrap().into_iter().map(|p| p.to_string_lossy().to_string()).collect();
        files.sort();

        assert_eq!(files, vec!["a.md".to_string(), "nested/b.md".to_string()]);
    }

    #[test]
    fn list_all_files_finds_every_file_including_non_markdown() {
        let vault = temp_vault();
        vault.write("a.md", "one").unwrap();
        vault.write("nested/notes.txt", "ignored by list_files").unwrap();
        vault.write("nested/.hidden/secret.md", "excluded, dot-directory").unwrap();
        vault.write(".dotfile", "excluded, dotfile").unwrap();

        let mut files: Vec<String> =
            vault.list_all_files().unwrap().into_iter().map(|p| p.to_string_lossy().to_string()).collect();
        files.sort();

        assert_eq!(files, vec!["a.md".to_string(), "nested/notes.txt".to_string()]);
    }

    #[test]
    fn search_matches_case_insensitively_and_caps_results() {
        let vault = temp_vault();
        vault.write("dentist.md", "Appointment with the Dentist on Friday").unwrap();
        vault.write("unrelated.md", "Grocery list: eggs, bread").unwrap();

        let hits = vault.search("dentist appointment", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "dentist.md");
        assert_eq!(hits[0].line_number, 1);

        let capped = vault.search("dentist appointment", 0).unwrap();
        assert!(capped.is_empty());
    }

    #[test]
    fn fixed_vault_files_excluded_from_list_and_search_but_not_a_nested_same_name_file() {
        let vault = temp_vault();
        vault.write("_profile.md", "name: dentist appointment person").unwrap();
        vault.write("notes/_profile.md", "a real note that happens to share the name").unwrap();
        vault.write("a.md", "unrelated dentist appointment note").unwrap();

        let mut files: Vec<String> =
            vault.list_files().unwrap().into_iter().map(|p| p.to_string_lossy().to_string()).collect();
        files.sort();
        assert_eq!(files, vec!["a.md".to_string(), "notes/_profile.md".to_string()]);

        let hits = vault.search("dentist appointment", 10).unwrap();
        assert!(hits.iter().all(|h| h.path != "_profile.md"));
        assert!(hits.iter().any(|h| h.path == "a.md"));
    }

    #[test]
    fn skills_dir_excluded_from_list_and_search_but_kept_in_list_all_files() {
        let vault = temp_vault();
        vault.write("skills/review.md", "dentist appointment instructions").unwrap();
        vault.write("notes/skills/nested.md", "a real note in a folder that happens to be named skills").unwrap();
        vault.write("a.md", "unrelated dentist appointment note").unwrap();

        let mut files: Vec<String> =
            vault.list_files().unwrap().into_iter().map(|p| p.to_string_lossy().to_string()).collect();
        files.sort();
        assert_eq!(files, vec!["a.md".to_string(), "notes/skills/nested.md".to_string()]);

        let hits = vault.search("dentist appointment", 10).unwrap();
        assert!(hits.iter().all(|h| !h.path.starts_with("skills/")));

        let all: Vec<String> =
            vault.list_all_files().unwrap().into_iter().map(|p| p.to_string_lossy().to_string()).collect();
        assert!(all.contains(&"skills/review.md".to_string()));
    }

    #[test]
    fn standing_memory_skips_missing_and_blank_files() {
        let vault = temp_vault();
        assert_eq!(vault.standing_memory(), "");

        vault.write("_profile.md", "  \n").unwrap(); // blank after trim
        assert_eq!(vault.standing_memory(), "");

        vault.write("_behavior.md", "Be concise.").unwrap();
        assert_eq!(vault.standing_memory(), "## AI behavior\n\nBe concise.");
    }

    #[test]
    fn standing_memory_joins_present_sections_in_fixed_order() {
        let vault = temp_vault();
        vault.write("_feedback.md", "Prefers terse answers.").unwrap();
        vault.write("_profile.md", "Name: Ada.").unwrap();

        assert_eq!(
            vault.standing_memory(),
            "## User profile\n\nName: Ada.\n\n## Feedback / lessons learned\n\nPrefers terse answers."
        );
    }

    /// P84 fatia 4: a member's vault, encrypted on disk.
    fn encrypted_vault() -> (Vault, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "warden-vault-enc-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        (Vault::new_encrypted(&dir, Arc::new(VaultCipher::new(&[7; 32]))), dir)
    }

    /// Every file under `dir` with its name and bytes, to check nothing readable is on disk.
    fn everything_on_disk(dir: &Path) -> String {
        let mut seen = String::new();
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            seen.push_str(&path.file_name().unwrap().to_string_lossy());
            if path.is_dir() {
                seen.push_str(&everything_on_disk(&path));
            } else {
                seen.push_str(&String::from_utf8_lossy(&std::fs::read(&path).unwrap()));
            }
        }
        seen
    }

    #[test]
    fn an_encrypted_vault_shows_the_same_paths_and_text_but_stores_neither() {
        let (vault, dir) = encrypted_vault();
        vault.write("reuniões/março.md", "o segredo é feijão").unwrap();
        vault.write("_profile.md", "Nome: Ada").unwrap();
        vault.write("skills/x.md", "instruções").unwrap();

        assert_eq!(vault.read("reuniões/março.md").unwrap(), "o segredo é feijão");
        assert_eq!(vault.standing_memory(), "## User profile\n\nNome: Ada");
        let files: Vec<String> = vault.list_files().unwrap().iter().map(|p| p.to_string_lossy().to_string()).collect();
        assert_eq!(files, ["reuniões/março.md"], "the fixed file and skills/ stay out, as in a plain vault");
        let all: Vec<String> = vault.list_all_files().unwrap().iter().map(|p| p.to_string_lossy().to_string()).collect();
        assert!(all.contains(&"skills/x.md".to_string()) && all.contains(&"reuniões/março.md".to_string()), "{all:?}");
        let hits = vault.search("feijão", 10).unwrap();
        assert_eq!((hits[0].path.as_str(), hits[0].line.as_str()), ("reuniões/março.md", "o segredo é feijão"));

        let disk = everything_on_disk(&dir);
        for secret in ["feijão", "março", "reuniões", "Ada", "_profile", "skills", "instruções"] {
            assert!(!disk.contains(secret), "'{secret}' is on disk");
        }
        assert!(std::fs::read(vault.path_of("reuniões/março.md").unwrap()).unwrap().starts_with(b"WRD1"));
    }

    #[test]
    fn an_encrypted_vault_edits_notes_with_versions_and_deletes_them() {
        let (vault, dir) = encrypted_vault();
        let v1 = vault.save_note("diário/hoje.md", "primeira", None).unwrap();
        assert_eq!(vault.read_note("diário/hoje.md").unwrap(), NoteFile { content: "primeira".into(), version: v1.clone() });
        assert!(vault.save_note("diário/hoje.md", "de novo", None).unwrap_err().downcast_ref::<NoteConflict>().is_some());
        let v2 = vault.save_note("diário/hoje.md", "segunda", Some(&v1)).unwrap();
        assert!(vault.save_note("diário/hoje.md", "velha", Some(&v1)).unwrap_err().downcast_ref::<NoteConflict>().is_some());
        assert_eq!(vault.browse_files().unwrap(), ["diário/hoje.md"]);
        assert!(!everything_on_disk(&dir).contains("segunda"));
        vault.delete_note("diário/hoje.md", &v2).unwrap();
        assert!(vault.browse_files().unwrap().is_empty());
        assert!(vault.read_note("diário/hoje.md").is_err());
    }

    #[test]
    fn an_encrypted_vault_refuses_a_file_it_did_not_write_and_a_wrong_key() {
        let (vault, dir) = encrypted_vault();
        vault.write("a.md", "texto").unwrap();
        std::fs::write(dir.join("solto.md"), "plain leftover").unwrap();
        assert_eq!(vault.list_files().unwrap().len(), 1, "a plain file is not part of an encrypted vault");
        let other_key = Vault::new_encrypted(&dir, Arc::new(VaultCipher::new(&[8; 32])));
        assert!(other_key.list_files().unwrap().is_empty(), "another key can't even see the names");
        assert!(other_key.read("a.md").is_err());
        let sealed = std::fs::read(vault.path_of("a.md").unwrap()).unwrap();
        assert!(other_key.decode(sealed).is_err());
    }

    #[test]
    fn an_encrypted_vault_keeps_the_mounts_readable_from_the_owner_vault() {
        let owner = temp_vault();
        owner.write("casa/lista.md", "arroz e feijão").unwrap();
        let casa = Arc::new(Vault::new(owner.root().join("casa")));
        let (member, dir) = encrypted_vault();
        member.write("minha.md", "nota minha").unwrap();
        member.set_mounts(vec![Mount { prefix: "casa".into(), vault: casa, writable: false }]);
        assert_eq!(member.read("compartilhado/casa/lista.md").unwrap(), "arroz e feijão");
        let files: Vec<String> = member.list_files().unwrap().iter().map(|p| p.to_string_lossy().to_string()).collect();
        assert!(files.contains(&"minha.md".to_string()) && files.contains(&"compartilhado/casa/lista.md".to_string()), "{files:?}");
        assert!(member.write("compartilhado/casa/x.md", "não").is_err());
        assert!(!everything_on_disk(&dir).contains("nota minha"));
    }

    #[test]
    fn an_encrypted_vault_refuses_a_name_too_long_to_store() {
        let (vault, _) = encrypted_vault();
        assert!(vault.write(&format!("{}.md", "n".repeat(MAX_NAME_BYTES)), "x").is_err());
    }

    #[cfg(feature = "semantic-search")]
    #[test]
    fn the_semantic_index_of_an_encrypted_vault_is_encrypted_too() {
        let (vault, dir) = encrypted_vault();
        vault.write("a.md", "nota sobre feijão").unwrap();
        let index = vault.physical(&Path::new(semantic::INDEX_DIR).join(semantic::INDEX_FILE)).unwrap();
        vault.write_file(&index, &semantic::SemanticIndex::new().to_bytes().unwrap()).unwrap();
        let loaded = semantic::SemanticIndex::from_bytes(vault.read_file(&index).ok().as_deref());
        assert_eq!(loaded.model_id, semantic::MODEL_ID);
        assert!(std::fs::read(&index).unwrap().starts_with(b"WRD1"));
        assert!(!everything_on_disk(&dir).contains("model_id"));
    }
}
