//! `warden_core::storage::StorageProvider` face for the decentralized (TruthID/Arweave) vault —
//! P61. `read`/`write`/`list`/`delete` (and their plain `export_all`/`import_all` defaults) all
//! delegate to the local `Vault`, exactly like `LocalFSProvider` — deliberately, not an oversight:
//! per-file CRUD has no Arweave equivalent (`pin()` has no selective read/write). The
//! `_interactive` variants (P61 follow-up) are where this provider actually earns the
//! "decentralized" name: `export_all_interactive` pulls the latest published snapshot for real
//! before delegating to the local export (non-interactive — pulling never needs phone approval,
//! only publishing does), and `import_all_interactive` writes locally and then publishes it for
//! real via `SyncEngine::begin_push`/`finish_push`, surfacing the QR payload through `on_qr` for
//! whoever's driving the interactive approval (the TruthID phone app has to scan it). Both skip
//! their Arweave step silently (falling back to the exact local-only behavior this provider always
//! had) when this device has never run `SyncEngine::init_fresh`/paired — selecting
//! `decentralized_vault` without ever visiting the Sync screen still works, it just doesn't back
//! anything up yet, same as before this file changed.

use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::sync::Arc;

use async_trait::async_trait;
use warden_core::memory::Vault;
use warden_core::storage::{LocalFSProvider, StorageProvider};

use crate::SyncEngine;

pub struct DecentralizedVaultProvider {
    inner: LocalFSProvider,
    sync: SyncEngine,
}

impl DecentralizedVaultProvider {
    pub fn new(vault: Arc<Vault>, sync: SyncEngine) -> Self {
        Self { inner: LocalFSProvider::new(vault), sync }
    }

    /// Same as `import_all_interactive`, but sweeps only `hosts` for the TruthID phone instead of
    /// the real LAN — the only way to drive the push side of this provider against a fake phone
    /// bound to `127.0.0.1` in a test, same reasoning as `SyncEngine::finish_push_with_hosts`
    /// itself (real `candidate_hosts()` deliberately excludes loopback).
    pub async fn import_all_interactive_with_hosts(
        &self,
        data: HashMap<String, Vec<u8>>,
        on_qr: Option<&(dyn Fn(String) + Send + Sync)>,
        hosts: Vec<Ipv4Addr>,
    ) -> anyhow::Result<()> {
        self.inner.import_all(data).await?;
        self.push_after_import(on_qr, Some(hosts)).await
    }

    async fn push_after_import(&self, on_qr: Option<&(dyn Fn(String) + Send + Sync)>, hosts: Option<Vec<Ipv4Addr>>) -> anyhow::Result<()> {
        if !self.sync.is_initialized() {
            return Ok(());
        }
        let Some(begin) = self.sync.begin_push()? else {
            return Ok(());
        };
        if let Some(on_qr) = on_qr {
            on_qr(begin.pending.qr_payload_json()?);
        }
        match hosts {
            Some(hosts) => self.sync.finish_push_with_hosts(begin, hosts).await?,
            None => self.sync.finish_push(begin).await?,
        };
        Ok(())
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

    /// Pulls the latest published snapshot for real before snapshotting the local vault — but
    /// only when this device is both initialized (`SyncEngine::init_fresh` ran at some point) and
    /// already paired (`owner_address` known, i.e. something has been published before, by this
    /// device or one it paired with). A real pull error (network, decrypt, ...) propagates as a
    /// hard failure rather than silently falling back to a possibly-stale local export — the
    /// caller asked for the interactive/"real" path, so a failure to actually reach Arweave should
    /// not be swallowed. Without initialization/pairing, behaves exactly like the plain
    /// `export_all` — no error, no network call, same as this provider always did.
    async fn export_all_interactive(&self, _on_qr: Option<&(dyn Fn(String) + Send + Sync)>) -> anyhow::Result<HashMap<String, Vec<u8>>> {
        if self.sync.is_initialized() {
            let status = self.sync.status()?;
            if status.owner_address.is_some() {
                self.sync.pull().await?;
            }
        }
        self.inner.export_all().await
    }

    /// Writes locally (same as `import_all`), then — only when this device is initialized — also
    /// publishes for real via `SyncEngine::begin_push`/`finish_push`, invoking `on_qr` with the raw
    /// QR payload JSON right before blocking on the phone. Nothing to push (`begin_push` returns
    /// `None`) is a silent no-op; a real push error (network, phone timeout, ...) propagates as a
    /// hard failure of the whole import, matching `migrate`'s existing fail-loud posture. Without
    /// initialization, behaves exactly like the plain `import_all` — local write only, no error, no
    /// QR, same as this provider always did.
    async fn import_all_interactive(
        &self,
        data: HashMap<String, Vec<u8>>,
        on_qr: Option<&(dyn Fn(String) + Send + Sync)>,
    ) -> anyhow::Result<()> {
        self.inner.import_all(data).await?;
        self.push_after_import(on_qr, None).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn temp_dir() -> std::path::PathBuf {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        std::env::temp_dir().join(format!(
            "warden-decentralized-vault-test-{}-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ))
    }

    /// A `SyncEngine` that's never `init_fresh`'d — `is_initialized()` stays `false`, so every
    /// `_interactive` method degrades to its plain local-only behavior without touching the
    /// network. Real (initialized) `SyncEngine` behavior is covered by the integration tests in
    /// `tests/storage_provider_export_interactive.rs`/`tests/storage_provider_import_interactive.rs`.
    fn temp_provider() -> DecentralizedVaultProvider {
        let (provider, _base) = build_provider();
        provider
    }

    /// Same layout `warden-bootstrap`'s `build_storage_provider` and desktop's `AppState.sync`
    /// both use in production: the vault directory and the sync-state files (`sync_secrets.json`/
    /// `sync_manifest.json`) are *siblings*, never nested inside one another. A test that instead
    /// pointed both at the same directory would, after `init_fresh`, have those JSON files sitting
    /// inside the vault itself — `diff_vault` would then see them as untracked vault content and
    /// `begin_push` would think something real changed even for an "empty vault" test, sweeping
    /// the real LAN for a phone that was never started.
    fn build_provider() -> (DecentralizedVaultProvider, std::path::PathBuf) {
        let base = temp_dir();
        let vault_root = base.join("vault");
        let vault = Arc::new(Vault::new(vault_root));
        let sync = SyncEngine::new(base.join("vault"), base.join("config.toml"), base.join("sync_secrets.json"), base.join("sync_manifest.json"));
        (DecentralizedVaultProvider::new(vault, sync), base)
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

    #[tokio::test]
    async fn export_all_interactive_skips_pull_when_never_initialized() {
        let provider = temp_provider();
        provider.write("a.md", b"one").await.unwrap();

        // No `init_fresh` ran — `is_initialized()` is false, so this must not attempt any network
        // call (which would hang/fail in a unit test with no fake gateway configured) and must
        // just behave like the plain `export_all`.
        let snapshot = provider.export_all_interactive(None).await.unwrap();
        assert_eq!(snapshot.get("a.md"), Some(&b"one".to_vec()));
    }

    #[tokio::test]
    async fn export_all_interactive_skips_pull_when_initialized_but_never_paired() {
        let (provider, base) = build_provider();
        manifest::save_secrets(&base.join("sync_secrets.json"), &manifest::generate_secrets()).unwrap();
        provider.write("a.md", b"one").await.unwrap();

        // Initialized, but `owner_address` was never set (never pushed/paired) — `pull()` would
        // error ("ainda não pareado"); this must skip it rather than surface that as a migration
        // failure.
        let snapshot = provider.export_all_interactive(None).await.unwrap();
        assert_eq!(snapshot.get("a.md"), Some(&b"one".to_vec()));
    }

    #[tokio::test]
    async fn import_all_interactive_skips_push_when_never_initialized() {
        let provider = temp_provider();
        let mut data = HashMap::new();
        data.insert("a.md".to_string(), b"one".to_vec());

        let calls = std::sync::Mutex::new(0);
        let on_qr = |_: String| {
            *calls.lock().unwrap() += 1;
        };
        provider.import_all_interactive(data, Some(&on_qr)).await.unwrap();

        assert_eq!(provider.read("a.md").await.unwrap(), b"one");
        assert_eq!(*calls.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn import_all_interactive_skips_push_when_nothing_changed() {
        let (provider, base) = build_provider();
        manifest::save_secrets(&base.join("sync_secrets.json"), &manifest::generate_secrets()).unwrap();

        // Importing an empty snapshot into a device with nothing local either — `begin_push`
        // returns `None` (no diff), so the push step (and `on_qr`) must be skipped, not attempted.
        let calls = std::sync::Mutex::new(0);
        let on_qr = |_: String| {
            *calls.lock().unwrap() += 1;
        };
        provider.import_all_interactive(HashMap::new(), Some(&on_qr)).await.unwrap();

        assert_eq!(*calls.lock().unwrap(), 0);
    }
}
