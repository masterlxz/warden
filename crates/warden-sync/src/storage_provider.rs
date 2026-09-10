//! `warden_core::storage::StorageProvider` face for the decentralized (TruthID/Arweave) vault —
//! P61. Read/write/list/delete (and their `export_all`/`import_all` defaults) all delegate to the
//! local `Vault`, exactly like `LocalFSProvider` — deliberately, not an oversight: `pin()` has no
//! selective read and every publish requires a physical phone approval, so there is no way to
//! satisfy a per-file CRUD trait by actually talking to Arweave. The *interface* exists from this
//! MVP as the spec asks; the real Arweave push/pull stays exclusively behind `SyncEngine`'s own
//! `begin_push`/`finish_push`/`pull` (QR-mediated, already wired into the CLI/desktop/mobile
//! `/sync` commands) — nothing about those flows changes here. See `project/PENDING.md` P61 for
//! the accepted limitation this leaves open.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use warden_core::memory::Vault;
use warden_core::storage::{LocalFSProvider, StorageProvider};

pub struct DecentralizedVaultProvider {
    inner: LocalFSProvider,
}

impl DecentralizedVaultProvider {
    pub fn new(vault: Arc<Vault>) -> Self {
        Self { inner: LocalFSProvider::new(vault) }
    }
}

#[async_trait]
impl StorageProvider for DecentralizedVaultProvider {
    async fn read(&self, relative_path: &str) -> anyhow::Result<Vec<u8>> {
        self.inner.read(relative_path).await
    }

    async fn write(&self, relative_path: &str, content: &[u8]) -> anyhow::Result<()> {
        self.inner.write(relative_path, content).await
    }

    async fn list(&self) -> anyhow::Result<Vec<String>> {
        self.inner.list().await
    }

    async fn delete(&self, relative_path: &str) -> anyhow::Result<()> {
        self.inner.delete(relative_path).await
    }

    async fn export_all(&self) -> anyhow::Result<HashMap<String, Vec<u8>>> {
        self.inner.export_all().await
    }

    async fn import_all(&self, data: HashMap<String, Vec<u8>>) -> anyhow::Result<()> {
        self.inner.import_all(data).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_provider() -> DecentralizedVaultProvider {
        let dir = std::env::temp_dir().join(format!(
            "warden-decentralized-vault-test-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        DecentralizedVaultProvider::new(Arc::new(Vault::new(dir)))
    }

    #[tokio::test]
    async fn read_write_list_delete_all_delegate_to_the_local_vault() {
        let provider = temp_provider();
        provider.write("a.md", b"one").await.unwrap();
        provider.write("nested/b.md", b"two").await.unwrap();

        let mut files = provider.list().await.unwrap();
        files.sort();
        assert_eq!(files, vec!["a.md".to_string(), "nested/b.md".to_string()]);
        assert_eq!(provider.read("a.md").await.unwrap(), b"one");

        provider.delete("a.md").await.unwrap();
        assert!(provider.read("a.md").await.is_err());
    }

    #[tokio::test]
    async fn export_all_then_import_all_round_trips() {
        let source = temp_provider();
        source.write("a.md", b"one").await.unwrap();

        let snapshot = source.export_all().await.unwrap();

        let target = temp_provider();
        target.import_all(snapshot).await.unwrap();
        assert_eq!(target.read("a.md").await.unwrap(), b"one");
    }
}
