use std::path::PathBuf;

/// Markdown vault on local disk (Obsidian-compatible). IPFS mirroring lands in Phase 4.
pub struct Vault {
    root: PathBuf,
}

impl Vault {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &PathBuf {
        &self.root
    }

    pub fn read(&self, relative_path: &str) -> anyhow::Result<String> {
        Ok(std::fs::read_to_string(self.root.join(relative_path))?)
    }

    pub fn write(&self, relative_path: &str, content: &str) -> anyhow::Result<()> {
        let path = self.root.join(relative_path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Ok(std::fs::write(path, content)?)
    }
}
