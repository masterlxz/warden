//! Automatic vault sync (P61/P71): the agent always works on the local disk, and this keeps that
//! disk in step with the sync backend configured in `config.toml`. Shared by the desktop (its
//! 5-minute loop) and the standalone hub (the same loop, plus "sync now"/init/pair from the web
//! and from `warden-server sync`), so both run exactly the same rounds.
//!
//! One round rereads `config.toml`: with `[git_sync]` it pulls, then pushes, through
//! `GitSyncEngine`; without it, it only pulls through the Arweave `SyncEngine`, because an Arweave
//! push waits for a TruthID phone approval and never runs unattended. A device that has no vault
//! key yet (`sync_secrets.json`) has nothing to do. Rounds never overlap: the loop, a "sync now",
//! an init and a pairing all take the same lock.
//!
//! A device already in the group can also show a pairing code (`start_hosting`, P88) so another
//! one joins through it. That waits up to `PAIRING_TIMEOUT` in the background and only reads the
//! vault key, so it stays out of the rounds' lock.

use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

// Re-exported for `warden-server sync host`, which says how long the code lasts and which ports to open.
pub use warden_sync::pairing::protocol::{PAIRING_PORTS, PAIRING_TIMEOUT};
use warden_sync::{GitSyncEngine, SyncEngine};

use crate::load_config_from_path;

/// How often the loop runs a round — the first one right away, covering "just opened after
/// being away".
pub const AUTO_SYNC_INTERVAL: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncBackend {
    /// No vault key on this device yet.
    NotSetUp,
    Git,
    Arweave,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PulledSummary {
    /// The Arweave tx applied; `None` for git, which can apply several commits in one pull.
    pub tx_id: Option<String>,
    pub files_written: usize,
    pub files_deleted: usize,
    pub config_updated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushedSummary {
    pub commit_sha: String,
    pub files_changed: usize,
    pub config_changed: bool,
}

/// What one round did. `pulled`/`pushed` are only set when something actually moved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncReport {
    pub at_ms: i64,
    pub backend: SyncBackend,
    pub pulled: Option<PulledSummary>,
    pub pushed: Option<PushedSummary>,
    pub error: Option<String>,
}

impl SyncReport {
    /// Whether this round brought a new `config.toml`, so whoever built an orchestrator from the
    /// old one should rebuild it.
    pub fn config_updated(&self) -> bool {
        self.pulled.as_ref().is_some_and(|p| p.config_updated)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncState {
    pub backend: SyncBackend,
    /// `[git_sync]`'s URL, never its token.
    pub git_remote: Option<String>,
    pub last_synced_at_ms: Option<i64>,
    pub pending_vault_changes: usize,
    pub pending_config_changed: bool,
    /// The most recent round this process ran, if any.
    pub last_round: Option<SyncReport>,
    /// Until when this device is showing a pairing code. The code itself is never part of the
    /// state: anyone who reads it can take the vault key.
    pub hosting_until_ms: Option<i64>,
    /// How the most recent pairing this device showed a code for ended.
    pub last_pairing: Option<PairingResult>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingResult {
    pub at_ms: i64,
    /// `None`: a device joined.
    pub error: Option<String>,
}

/// The code `start_hosting` is showing, and until when.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostedPairing {
    pub code: String,
    pub expires_at_ms: i64,
}

struct ActiveHosting {
    shown: HostedPairing,
    /// Tells apart the session a finishing task belongs to from a newer one.
    session: u64,
    task: tokio::task::AbortHandle,
}

#[derive(Default)]
struct Hosting {
    active: Option<ActiveHosting>,
    last: Option<PairingResult>,
    next_session: u64,
}

pub struct SyncRunner {
    vault_path: PathBuf,
    config_path: PathBuf,
    secrets_path: PathBuf,
    manifest_path: PathBuf,
    git_repo_path: PathBuf,
    lock: tokio::sync::Mutex<()>,
    last: Mutex<Option<SyncReport>>,
    hosting: Arc<Mutex<Hosting>>,
}

impl SyncRunner {
    pub fn new(vault_path: PathBuf, config_path: PathBuf, secrets_path: PathBuf, manifest_path: PathBuf, git_repo_path: PathBuf) -> Self {
        Self { vault_path, config_path, secrets_path, manifest_path, git_repo_path, lock: tokio::sync::Mutex::new(()), last: Mutex::new(None), hosting: Arc::default() }
    }

    /// The paths every Warden install uses under the OS config dir (`warden_sync::paths`).
    pub fn with_default_paths(vault_path: PathBuf, config_path: PathBuf) -> Self {
        Self::new(
            vault_path,
            config_path,
            warden_sync::paths::default_sync_secrets_path().unwrap_or_else(|| PathBuf::from("sync_secrets.json")),
            warden_sync::paths::default_sync_manifest_path().unwrap_or_else(|| PathBuf::from("sync_manifest.json")),
            warden_sync::paths::default_git_sync_repo_path().unwrap_or_else(|| PathBuf::from("git-sync-repo")),
        )
    }

    fn arweave_engine(&self) -> SyncEngine {
        SyncEngine::new(self.vault_path.clone(), self.config_path.clone(), self.secrets_path.clone(), self.manifest_path.clone())
    }

    fn git_config(&self) -> anyhow::Result<Option<crate::GitSyncConfig>> {
        Ok(load_config_from_path(&self.config_path, false)?.git_sync)
    }

    /// Runs one round now, waiting for any round already running to finish first.
    pub async fn run_once(&self) -> SyncReport {
        let _serialized = self.lock.lock().await;
        let report = self.round().await;
        *self.last.lock().unwrap() = Some(report.clone());
        report
    }

    async fn round(&self) -> SyncReport {
        let mut report = SyncReport { at_ms: now_ms(), backend: SyncBackend::NotSetUp, pulled: None, pushed: None, error: None };
        if !self.secrets_path.exists() {
            return report;
        }
        let git = match self.git_config() {
            Ok(git) => git,
            Err(err) => {
                report.error = Some(format!("{err:#}"));
                return report;
            }
        };
        match git {
            Some(git) => {
                report.backend = SyncBackend::Git;
                let engine = GitSyncEngine::new(
                    self.vault_path.clone(),
                    self.config_path.clone(),
                    self.secrets_path.clone(),
                    self.manifest_path.clone(),
                    self.git_repo_path.clone(),
                    git.remote_url,
                    git.token,
                );
                match engine.pull().await {
                    Ok(o) => {
                        if o.files_written > 0 || o.files_deleted > 0 || o.config_updated {
                            report.pulled = Some(PulledSummary {
                                tx_id: None,
                                files_written: o.files_written,
                                files_deleted: o.files_deleted,
                                config_updated: o.config_updated,
                            });
                        }
                    }
                    Err(err) => {
                        // Pushing on top of a state that may be stale would only be rejected.
                        report.error = Some(format!("pull: {err:#}"));
                        return report;
                    }
                }
                match engine.push().await {
                    Ok(Some(o)) => {
                        report.pushed = Some(PushedSummary { commit_sha: o.commit_sha, files_changed: o.files_changed, config_changed: o.config_changed })
                    }
                    Ok(None) => {}
                    Err(err) => report.error = Some(format!("push: {err:#}")),
                }
            }
            None => {
                report.backend = SyncBackend::Arweave;
                // Not paired with a TruthID phone yet: there's no owner to pull from, which isn't
                // an error worth repeating every round.
                match warden_sync::manifest::load_manifest(&self.manifest_path) {
                    Ok(manifest) if !manifest.is_paired() => return report,
                    Ok(_) => {}
                    Err(err) => {
                        report.error = Some(format!("{err:#}"));
                        return report;
                    }
                }
                match self.arweave_engine().pull().await {
                    Ok(o) => {
                        if o.files_written > 0 || o.files_deleted > 0 || o.config_updated {
                            report.pulled = Some(PulledSummary {
                                tx_id: o.tx_id,
                                files_written: o.files_written,
                                files_deleted: o.files_deleted,
                                config_updated: o.config_updated,
                            });
                        }
                    }
                    Err(err) => report.error = Some(format!("pull: {err:#}")),
                }
            }
        }
        report
    }

    /// Runs a round every `interval`, forever, handing each report to `on_report` and waiting for
    /// what it returns (the hub reloads its orchestrator there). The caller spawns it on its own
    /// runtime (Tauri's on the desktop, tokio's on the hub).
    pub async fn run_loop<F, Fut>(self: Arc<Self>, interval: Duration, on_report: F)
    where
        F: Fn(&SyncReport) -> Fut,
        Fut: std::future::Future<Output = ()>,
    {
        let mut ticker = tokio::time::interval(interval);
        loop {
            ticker.tick().await;
            let report = self.run_once().await;
            if let Some(err) = &report.error {
                eprintln!("auto-sync: {err}");
            }
            on_report(&report).await;
        }
    }

    pub fn state(&self) -> anyhow::Result<SyncState> {
        let initialized = self.secrets_path.exists();
        let git = self.git_config()?;
        let status = self.arweave_engine().status()?;
        let backend = match (initialized, &git) {
            (false, _) => SyncBackend::NotSetUp,
            (true, Some(_)) => SyncBackend::Git,
            (true, None) => SyncBackend::Arweave,
        };
        Ok(SyncState {
            backend,
            git_remote: git.map(|g| g.remote_url),
            last_synced_at_ms: status.last_synced_at_ms,
            pending_vault_changes: status.pending_vault_changes,
            pending_config_changed: status.pending_config_changed,
            last_round: self.last.lock().unwrap().clone(),
            hosting_until_ms: self.active_hosting().map(|h| h.expires_at_ms),
            last_pairing: self.hosting.lock().unwrap().last.clone(),
        })
    }

    /// Makes this the first device of a sync group: a fresh vault key.
    pub async fn init_fresh(&self) -> anyhow::Result<()> {
        let _serialized = self.lock.lock().await;
        self.arweave_engine().init_fresh()
    }

    /// Joins a sync group through a device showing `code` (`/sync pair` / the desktop's Sync
    /// screen). With `host`, only that address is tried — the way to reach a device over
    /// Tailscale or any network the LAN sweep doesn't cover.
    pub async fn pair_join(&self, code: &str, host: Option<Ipv4Addr>) -> anyhow::Result<()> {
        let _serialized = self.lock.lock().await;
        let engine = self.arweave_engine();
        match host {
            Some(host) => engine.pairing_join_with_hosts(code, vec![host]).await,
            None => engine.pairing_join(code).await,
        }
    }
}

impl SyncRunner {
    /// Shows a pairing code other devices join through (`/sync pair <code>`, the desktop's Sync
    /// screen, `warden-server sync pair`), waiting in the background until one does or
    /// `PAIRING_TIMEOUT` runs out. Asking again while a code is up hands back the same one.
    pub async fn start_hosting(&self) -> anyhow::Result<HostedPairing> {
        if let Some(shown) = self.active_hosting() {
            return Ok(shown);
        }
        // Errors on a device without a vault key, with nothing to hand over.
        let host = self.pairing_host().await?;
        let shown = HostedPairing { code: host.code().to_string(), expires_at_ms: now_ms() + PAIRING_TIMEOUT.as_millis() as i64 };

        let mut hosting = self.hosting.lock().unwrap();
        // Another call got here first while this one was binding: keep that one, this listener
        // is dropped with `host`.
        if let Some(active) = hosting.active.as_ref().filter(|a| a.shown.expires_at_ms > now_ms()) {
            return Ok(active.shown.clone());
        }
        let session = hosting.next_session;
        hosting.next_session += 1;
        let slot = Arc::clone(&self.hosting);
        let task = tokio::spawn(async move {
            let error = host.wait_for_join().await.err().map(|err| format!("{err:#}"));
            let mut hosting = slot.lock().unwrap();
            if hosting.active.as_ref().is_some_and(|a| a.session == session) {
                hosting.active = None;
                hosting.last = Some(PairingResult { at_ms: now_ms(), error });
            }
        });
        hosting.active = Some(ActiveHosting { shown: shown.clone(), session, task: task.abort_handle() });
        Ok(shown)
    }

    /// Stops showing the code, closing its port. Nothing happens when none is up.
    pub fn cancel_hosting(&self) {
        let mut hosting = self.hosting.lock().unwrap();
        if let Some(active) = hosting.active.take() {
            active.task.abort();
            hosting.last = Some(PairingResult { at_ms: now_ms(), error: Some("cancelled".to_string()) });
        }
    }

    /// A pairing host for a caller that waits on it itself (`warden-server sync host`), outside
    /// `start_hosting`'s bookkeeping.
    pub async fn pairing_host(&self) -> anyhow::Result<warden_sync::pairing::PairingHost> {
        self.arweave_engine().pairing_host().await
    }

    fn active_hosting(&self) -> Option<HostedPairing> {
        let hosting = self.hosting.lock().unwrap();
        hosting.active.as_ref().map(|a| a.shown.clone())
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "warden-auto-sync-{name}-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn bare_remote(dir: &std::path::Path) -> PathBuf {
        let path = dir.join("remote.git");
        let status = std::process::Command::new("git").args(["init", "--bare", "-q", &path.to_string_lossy()]).status().unwrap();
        assert!(status.success());
        path
    }

    /// One device: its own vault, config, secrets, manifest and git clone under `dir/name`.
    fn device(dir: &std::path::Path, name: &str, remote: Option<&std::path::Path>) -> SyncRunner {
        let root = dir.join(name);
        std::fs::create_dir_all(root.join("vault")).unwrap();
        let config_path = root.join("config.toml");
        if let Some(remote) = remote {
            std::fs::write(&config_path, format!("[git_sync]\nremote_url = {:?}\ntoken = \"t\"\n", remote.to_string_lossy())).unwrap();
        }
        SyncRunner::new(root.join("vault"), config_path, root.join("secrets.json"), root.join("manifest.json"), root.join("git-repo"))
    }

    #[tokio::test]
    async fn a_device_without_a_key_does_nothing() {
        let dir = temp_dir("no-key");
        let remote = bare_remote(&dir);
        let runner = device(&dir, "a", Some(&remote));
        let report = runner.run_once().await;
        assert_eq!(report.backend, SyncBackend::NotSetUp);
        assert_eq!((report.pulled, report.pushed, report.error), (None, None, None));
        assert_eq!(runner.state().unwrap().backend, SyncBackend::NotSetUp);
    }

    #[tokio::test]
    async fn git_rounds_carry_notes_and_config_between_devices() {
        let dir = temp_dir("git");
        let remote = bare_remote(&dir);
        let a = device(&dir, "a", Some(&remote));
        a.init_fresh().await.unwrap();
        // Same key on the second device, the way pairing would leave it.
        let b = device(&dir, "b", Some(&remote));
        std::fs::copy(&a.secrets_path, &b.secrets_path).unwrap();

        std::fs::write(a.vault_path.join("nota.md"), "oi").unwrap();
        let report = a.run_once().await;
        assert_eq!(report.backend, SyncBackend::Git);
        assert_eq!(report.error, None);
        assert!(report.pushed.as_ref().is_some_and(|p| p.files_changed == 1), "{report:?}");
        assert_eq!(a.state().unwrap().pending_vault_changes, 0);

        // b's own config differs from a's, so b's first round pulls a's over it.
        std::fs::write(&b.config_path, format!("vault_path = \"x\"\n[git_sync]\nremote_url = {:?}\ntoken = \"t\"\n", remote.to_string_lossy())).unwrap();
        let report = b.run_once().await;
        assert_eq!(report.error, None, "{report:?}");
        assert!(report.config_updated(), "{report:?}");
        assert_eq!(std::fs::read_to_string(b.vault_path.join("nota.md")).unwrap(), "oi");
        assert_eq!(b.state().unwrap().last_round, Some(report));

        // Nothing new anywhere: a quiet round.
        let report = a.run_once().await;
        assert_eq!((report.pulled, report.pushed, report.error), (None, None, None));
    }

    #[tokio::test]
    async fn without_git_it_only_pulls_from_arweave() {
        let dir = temp_dir("arweave");
        let runner = device(&dir, "a", None);
        runner.init_fresh().await.unwrap();
        std::fs::write(runner.vault_path.join("nota.md"), "oi").unwrap();
        let state = runner.state().unwrap();
        assert_eq!((state.backend, state.git_remote, state.pending_vault_changes), (SyncBackend::Arweave, None, 1));
        // Not paired with a TruthID phone: nothing to pull from, and never a push.
        let report = runner.run_once().await;
        assert_eq!(report.backend, SyncBackend::Arweave);
        assert_eq!((report.pulled, report.pushed, report.error), (None, None, None));
    }

    #[tokio::test]
    async fn rounds_never_overlap() {
        let dir = temp_dir("lock");
        let remote = bare_remote(&dir);
        let runner = Arc::new(device(&dir, "a", Some(&remote)));
        runner.init_fresh().await.unwrap();
        std::fs::write(runner.vault_path.join("nota.md"), "oi").unwrap();
        let (one, two) = tokio::join!(runner.run_once(), runner.run_once());
        // Had they overlapped, both would push the same change and one would be rejected.
        assert_eq!((one.error, two.error), (None, None));
        assert_eq!([one.pushed.is_some(), two.pushed.is_some()].iter().filter(|p| **p).count(), 1);
    }

    #[tokio::test]
    async fn pairing_with_an_explicit_host_adopts_the_key() {
        let dir = temp_dir("pair");
        let a = device(&dir, "a", None);
        a.init_fresh().await.unwrap();
        let host = a.arweave_engine().pairing_host().await.unwrap();
        let code = host.code().to_string();
        let b = device(&dir, "b", None);
        let (joined, hosted) = tokio::join!(b.pair_join(&code, Some(Ipv4Addr::LOCALHOST)), host.wait_for_join());
        joined.unwrap();
        hosted.unwrap();
        assert_eq!(b.state().unwrap().backend, SyncBackend::Arweave);
        let key = |r: &SyncRunner| warden_sync::manifest::load_secrets(&r.secrets_path).unwrap().unwrap().vault_key;
        assert_eq!(key(&a), key(&b));
        // A second pairing would throw that key away.
        assert!(b.init_fresh().await.is_err());
    }

    #[tokio::test]
    async fn a_device_in_the_group_shows_a_code_another_joins_through() {
        let dir = temp_dir("host");
        let a = device(&dir, "a", None);
        assert!(a.start_hosting().await.is_err(), "no vault key, nothing to hand over");
        a.init_fresh().await.unwrap();

        let shown = a.start_hosting().await.unwrap();
        assert_eq!(a.start_hosting().await.unwrap(), shown, "asking again shows the same code");
        let state = a.state().unwrap();
        assert_eq!((state.hosting_until_ms, state.last_pairing), (Some(shown.expires_at_ms), None));

        let b = device(&dir, "b", None);
        b.pair_join(&shown.code, Some(Ipv4Addr::LOCALHOST)).await.unwrap();
        let key = |r: &SyncRunner| warden_sync::manifest::load_secrets(&r.secrets_path).unwrap().unwrap().vault_key;
        assert_eq!(key(&a), key(&b));

        // The host's side finishes right after the joiner's ack.
        let mut state = a.state().unwrap();
        for _ in 0..50 {
            if state.hosting_until_ms.is_none() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
            state = a.state().unwrap();
        }
        assert_eq!(state.hosting_until_ms, None);
        assert_eq!(state.last_pairing.map(|p| p.error), Some(None));
    }

    #[tokio::test]
    async fn cancelling_closes_the_code() {
        let dir = temp_dir("host-cancel");
        let a = device(&dir, "a", None);
        a.init_fresh().await.unwrap();
        let shown = a.start_hosting().await.unwrap();
        a.cancel_hosting();
        let state = a.state().unwrap();
        assert_eq!(state.hosting_until_ms, None);
        assert_eq!(state.last_pairing.and_then(|p| p.error).as_deref(), Some("cancelled"));

        // Nobody answers on the pairing ports any more, so the joiner gives up.
        let b = device(&dir, "b", None);
        let joined = tokio::time::timeout(Duration::from_secs(3), b.pair_join(&shown.code, Some(Ipv4Addr::LOCALHOST))).await;
        assert!(!matches!(joined, Ok(Ok(()))), "{joined:?}");
        assert_eq!(b.state().unwrap().backend, SyncBackend::NotSetUp);
    }
}
