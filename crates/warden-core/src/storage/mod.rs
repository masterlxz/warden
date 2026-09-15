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

    /// Same as `export_all`, but gives a provider whose backend has a non-interactive remote step
    /// (e.g. `DecentralizedVaultProvider` pulling the latest Arweave snapshot first) the chance to
    /// run it before snapshotting. `on_qr` exists for signature symmetry with
    /// `import_all_interactive` — no known provider needs it on the export/pull side (pulling from
    /// Arweave never requires a phone approval, only publishing does), so it's always `None` here
    /// in practice. Default: delegates to `export_all`, so every existing provider (`LocalFSProvider`,
    /// `RemoteNodeProvider`, any future one) behaves identically unless it opts in by overriding
    /// this method — P61.
    async fn export_all_interactive(&self, _on_qr: Option<&(dyn Fn(String) + Send + Sync)>) -> anyhow::Result<HashMap<String, Vec<u8>>> {
        self.export_all().await
    }

    /// Same as `import_all`, but gives a provider whose backend needs a human-in-the-loop
    /// approval step to actually publish (e.g. `DecentralizedVaultProvider` pushing to Arweave,
    /// which requires scanning a QR with the TruthID phone app) a channel to surface that step.
    /// `on_qr`, when provided, is invoked with the **raw QR payload JSON** — never a rendered
    /// image; `warden-core`/`warden-sync` don't depend on any QR-rendering crate, so turning that
    /// JSON into an SVG/PNG is entirely the caller's job (see `desktop/src-tauri/src/qr.rs`).
    /// Default: delegates to `import_all` and ignores `on_qr`, so every existing provider behaves
    /// identically unless it opts in by overriding this method — P61.
    async fn import_all_interactive(
        &self,
        data: HashMap<String, Vec<u8>>,
        _on_qr: Option<&(dyn Fn(String) + Send + Sync)>,
    ) -> anyhow::Result<()> {
        self.import_all(data).await
    }
}

/// Result of a successful `migrate` — a count rather than bare `()`, so a caller (and its own
/// error messages/logs) has something concrete to report; room to grow (e.g. the list of migrated
/// paths) without another change to `migrate`'s return type.
#[derive(Debug, PartialEq, Eq)]
pub struct MigrationReport {
    pub files_migrated: usize,
}

/// Moves everything one `StorageProvider` holds into another (P61) — the real migration flow the
/// spec asked for behind `storage_provider`'s config switch. `export_all` from `from`, `import_all`
/// into `to`, then **re-exports from `to` and compares byte-for-byte against the original
/// snapshot** before declaring success: `import_all`'s `Ok(())` only means every `write` call
/// returned without erroring, not that the destination actually holds what was asked — a
/// destination that silently drops or mangles bytes (or a partial write left over from a crash)
/// would otherwise go unnoticed. Doesn't delete anything from `from` afterward, and doesn't
/// reconcile files that already existed in `to` but aren't in `from` — both out of scope for this
/// MVP (see `PENDING.md` P61); a caller that needs either builds it on top of this.
pub async fn migrate(from: &dyn StorageProvider, to: &dyn StorageProvider) -> anyhow::Result<MigrationReport> {
    let snapshot = from.export_all().await?;
    let files_migrated = snapshot.len();

    to.import_all(snapshot.clone()).await?;

    let landed = to.export_all().await?;
    if landed != snapshot {
        anyhow::bail!(
            "migration integrity check failed: destination holds {} file(s) after import, source snapshot had {files_migrated}",
            landed.len()
        );
    }

    Ok(MigrationReport { files_migrated })
}

/// Same as `migrate`, but drives the `_interactive` variants of `export_all`/`import_all` — P61's
/// closing of the "QR-interactive push/pull" gap. `on_qr` is threaded straight into
/// `to.import_all_interactive` (the only side that can ever need it — see that method's doc
/// comment); `from.export_all_interactive` never receives it, since no known provider needs
/// interactive approval to *read*. The post-import integrity check re-exports from `to` with the
/// **plain** `export_all`, not `export_all_interactive` — by that point `to` already holds
/// whatever `import_all_interactive` wrote locally, so a plain local re-export is enough to prove
/// it landed intact; re-running the interactive path here would just repeat a remote round-trip
/// (and, for a provider where interactive export *did* need approval, ask for it a second time)
/// for no verification benefit. For any provider that doesn't override the `_interactive` methods,
/// this behaves identically to `migrate`.
pub async fn migrate_interactive(
    from: &dyn StorageProvider,
    to: &dyn StorageProvider,
    on_qr: Option<&(dyn Fn(String) + Send + Sync)>,
) -> anyhow::Result<MigrationReport> {
    let snapshot = from.export_all_interactive(None).await?;
    let files_migrated = snapshot.len();

    to.import_all_interactive(snapshot.clone(), on_qr).await?;

    let landed = to.export_all().await?;
    if landed != snapshot {
        anyhow::bail!(
            "migration integrity check failed: destination holds {} file(s) after import, source snapshot had {files_migrated}",
            landed.len()
        );
    }

    Ok(MigrationReport { files_migrated })
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
    async fn migrate_copies_everything_into_a_different_provider_and_reports_the_count() {
        let source = temp_provider();
        source.write("a.md", b"one").await.unwrap();
        source.write("nested/b.md", b"two").await.unwrap();

        let target = temp_provider();
        let report = migrate(&source, &target).await.unwrap();

        assert_eq!(report, MigrationReport { files_migrated: 2 });
        assert_eq!(target.read("a.md").await.unwrap(), b"one");
        assert_eq!(target.read("nested/b.md").await.unwrap(), b"two");
    }

    /// A destination that reports `import_all` as `Ok(())` while silently dropping the content it
    /// was handed — proves `migrate`'s post-import re-export comparison actually catches this
    /// instead of trusting `import_all`'s success alone.
    struct LossyProvider(LocalFSProvider);

    #[async_trait]
    impl StorageProvider for LossyProvider {
        async fn read(&self, relative_path: &str) -> anyhow::Result<Vec<u8>> {
            self.0.read(relative_path).await
        }

        async fn write(&self, relative_path: &str, _content: &[u8]) -> anyhow::Result<()> {
            self.0.write(relative_path, b"").await
        }

        async fn list(&self) -> anyhow::Result<Vec<String>> {
            self.0.list().await
        }

        async fn delete(&self, relative_path: &str) -> anyhow::Result<()> {
            self.0.delete(relative_path).await
        }
    }

    #[tokio::test]
    async fn migrate_fails_when_the_destination_silently_corrupts_content() {
        let source = temp_provider();
        source.write("a.md", b"hello").await.unwrap();

        let target = LossyProvider(temp_provider());
        let err = migrate(&source, &target).await.unwrap_err();
        assert!(err.to_string().contains("integrity check failed"));
    }

    #[tokio::test]
    async fn migrate_interactive_behaves_like_migrate_for_providers_without_an_interactive_override() {
        let source = temp_provider();
        source.write("a.md", b"one").await.unwrap();
        source.write("nested/b.md", b"two").await.unwrap();

        let target = temp_provider();
        let report = migrate_interactive(&source, &target, None).await.unwrap();

        assert_eq!(report, MigrationReport { files_migrated: 2 });
        assert_eq!(target.read("a.md").await.unwrap(), b"one");
        assert_eq!(target.read("nested/b.md").await.unwrap(), b"two");
    }

    #[tokio::test]
    async fn migrate_interactive_also_catches_silent_corruption() {
        let source = temp_provider();
        source.write("a.md", b"hello").await.unwrap();

        let target = LossyProvider(temp_provider());
        let err = migrate_interactive(&source, &target, None).await.unwrap_err();
        assert!(err.to_string().contains("integrity check failed"));
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
