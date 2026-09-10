use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use crate::memory::Vault;

/// Where the agent's memory actually lives, decoupled from who/what pays or authorizes storing it
/// there (see `AuthProvider`) — P61. `read`/`write`/`list`/`delete` are the four primitives every
/// implementation must provide; `export_all`/`import_all` have default implementations built on
/// top of them (same "one required method, one provided default" shape as
/// `ModelProvider::chat_stream`/`chat`), so a new provider only needs to implement the four
/// primitives unless it wants a faster/native bulk path.
#[async_trait]
pub trait StorageProvider: Send + Sync {
    async fn read(&self, relative_path: &str) -> anyhow::Result<Vec<u8>>;
    async fn write(&self, relative_path: &str, content: &[u8]) -> anyhow::Result<()>;
    /// Relative paths of every stored file.
    async fn list(&self) -> anyhow::Result<Vec<String>>;
    async fn delete(&self, relative_path: &str) -> anyhow::Result<()>;

    /// Everything this provider holds, as a portable snapshot — used to migrate to a different
    /// `StorageProvider` (see `import_all`). Default: `list` then `read` each entry.
    async fn export_all(&self) -> anyhow::Result<HashMap<String, Vec<u8>>> {
        let mut out = HashMap::new();
        for path in self.list().await? {
            let content = self.read(&path).await?;
            out.insert(path, content);
        }
        Ok(out)
    }

    /// Loads a snapshot produced by `export_all` (of this or another provider). Default: `write`
    /// each entry.
    async fn import_all(&self, data: HashMap<String, Vec<u8>>) -> anyhow::Result<()> {
        for (path, content) in data {
            self.write(&path, &content).await?;
        }
        Ok(())
    }
}

/// Who's allowed to use a given `StorageProvider`, and under what subscription state — kept
/// separate from `StorageProvider` itself so a provider that needs no identity/payment at all
/// (`LocalFSProvider`) doesn't have to fake one. No implementation ties this to a real identity
/// system yet (`warden-truthid` exposes no subscription-check surface today) — `NoAuthProvider` is
/// the only implementation so far, deliberately trivial.
#[async_trait]
pub trait AuthProvider: Send + Sync {
    async fn get_user_id(&self) -> anyhow::Result<Option<String>>;
    async fn is_subscription_active(&self) -> anyhow::Result<bool>;
    async fn login(&self) -> anyhow::Result<()>;
    async fn logout(&self) -> anyhow::Result<()>;
}

/// No identity, no gate — always "logged in" and "active". Used wherever a `StorageProvider`
/// doesn't need authorization to function (today, that's every provider: even
/// `DecentralizedVaultProvider` doesn't check a real subscription yet, see its own doc comment).
pub struct NoAuthProvider;

#[async_trait]
impl AuthProvider for NoAuthProvider {
    async fn get_user_id(&self) -> anyhow::Result<Option<String>> {
        Ok(None)
    }

    async fn is_subscription_active(&self) -> anyhow::Result<bool> {
        Ok(true)
    }

    async fn login(&self) -> anyhow::Result<()> {
        Ok(())
    }

    async fn logout(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

/// The default, free `StorageProvider` — a thin `StorageProvider` face on top of `Vault`, which
/// already did all of this file I/O before this trait existed. `list`/`read`/`write` mirror
/// `Vault::list_all_files`/`read`/`write` (not `list_files`, which is markdown-only — a storage
/// provider must round-trip every file in the vault, same reasoning as `warden-sync`'s existing
/// use of `list_all_files`).
pub struct LocalFSProvider {
    vault: Arc<Vault>,
}

impl LocalFSProvider {
    pub fn new(vault: Arc<Vault>) -> Self {
        Self { vault }
    }
}

#[async_trait]
impl StorageProvider for LocalFSProvider {
    async fn read(&self, relative_path: &str) -> anyhow::Result<Vec<u8>> {
        Ok(std::fs::read(self.vault.root().join(relative_path))?)
    }

    async fn write(&self, relative_path: &str, content: &[u8]) -> anyhow::Result<()> {
        let path = self.vault.root().join(relative_path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Ok(std::fs::write(path, content)?)
    }

    async fn list(&self) -> anyhow::Result<Vec<String>> {
        Ok(self.vault.list_all_files()?.into_iter().map(|p| p.to_string_lossy().to_string()).collect())
    }

    async fn delete(&self, relative_path: &str) -> anyhow::Result<()> {
        self.vault.delete(relative_path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_provider() -> LocalFSProvider {
        let dir = std::env::temp_dir().join(format!(
            "warden-storage-test-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        LocalFSProvider::new(Arc::new(Vault::new(dir)))
    }

    #[tokio::test]
    async fn write_then_read_roundtrip() {
        let provider = temp_provider();
        provider.write("notes/todo.md", b"buy milk").await.unwrap();
        assert_eq!(provider.read("notes/todo.md").await.unwrap(), b"buy milk");
    }

    #[tokio::test]
    async fn list_finds_every_written_file() {
        let provider = temp_provider();
        provider.write("a.md", b"one").await.unwrap();
        provider.write("nested/b.txt", b"two").await.unwrap();

        let mut files = provider.list().await.unwrap();
        files.sort();
        assert_eq!(files, vec!["a.md".to_string(), "nested/b.txt".to_string()]);
    }

    #[tokio::test]
    async fn delete_removes_a_written_file() {
        let provider = temp_provider();
        provider.write("a.md", b"one").await.unwrap();
        provider.delete("a.md").await.unwrap();
        assert!(provider.read("a.md").await.is_err());
    }

    #[tokio::test]
    async fn export_all_then_import_all_round_trips_into_a_fresh_provider() {
        let source = temp_provider();
        source.write("a.md", b"one").await.unwrap();
        source.write("nested/b.md", b"two").await.unwrap();

        let snapshot = source.export_all().await.unwrap();
        assert_eq!(snapshot.len(), 2);

        let target = temp_provider();
        target.import_all(snapshot).await.unwrap();

        let mut files = target.list().await.unwrap();
        files.sort();
        assert_eq!(files, vec!["a.md".to_string(), "nested/b.md".to_string()]);
        assert_eq!(target.read("nested/b.md").await.unwrap(), b"two");
    }

    #[tokio::test]
    async fn no_auth_provider_always_reports_active_with_no_identity() {
        let auth = NoAuthProvider;
        assert_eq!(auth.get_user_id().await.unwrap(), None);
        assert!(auth.is_subscription_active().await.unwrap());
        auth.login().await.unwrap();
        auth.logout().await.unwrap();
    }
}
