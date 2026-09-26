//! One sync at a time on this machine, across processes (P88). The `SyncRunner`'s mutex only
//! covers its own process, while the desktop's manual git buttons, `warden-server sync now` next
//! to a running `serve`, and the CLI's `/sync` each build their own engine over the same vault,
//! manifest and git clone. So the engines themselves take an OS file lock next to the manifest
//! (the state both backends share) around everything that writes it.
//!
//! Never nest two of these in one process: on Linux a second lock through another descriptor of
//! the same file waits for the first, even in the same process.

use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How long another sync may keep the lock before giving up — a git round over a slow network or
/// an Arweave push waiting on the phone can take minutes.
pub const LOCK_WAIT: Duration = Duration::from_secs(10 * 60);

const RETRY_EVERY: Duration = Duration::from_millis(250);

/// Held while it lives; closing the file releases the lock.
#[derive(Debug)]
pub struct SyncLock {
    _file: File,
}

/// The lock file guarding `manifest_path` (`sync_manifest.json` → `sync_manifest.lock`).
pub fn lock_path_for(manifest_path: &Path) -> PathBuf {
    manifest_path.with_extension("lock")
}

impl SyncLock {
    pub async fn acquire(path: &Path) -> anyhow::Result<Self> {
        Self::acquire_with_timeout(path, LOCK_WAIT).await
    }

    pub async fn acquire_with_timeout(path: &Path, wait: Duration) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new().create(true).truncate(false).write(true).open(path)?;
        let deadline = Instant::now() + wait;
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(Self { _file: file }),
                Err(TryLockError::WouldBlock) => {
                    if Instant::now() >= deadline {
                        anyhow::bail!("another Warden process is still syncing this vault ({})", path.display());
                    }
                    tokio::time::sleep(RETRY_EVERY).await;
                }
                Err(TryLockError::Error(err)) => return Err(err.into()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_lock(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "warden-sync-lock-{name}-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("sync_manifest.lock")
    }

    #[tokio::test]
    async fn a_second_holder_waits_for_the_first() {
        let path = temp_lock("wait");
        let first = SyncLock::acquire(&path).await.unwrap();
        let release = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(400)).await;
            drop(first);
        });
        let started = Instant::now();
        let _second = SyncLock::acquire(&path).await.unwrap();
        assert!(started.elapsed() >= Duration::from_millis(350), "{:?}", started.elapsed());
        release.await.unwrap();
    }

    #[tokio::test]
    async fn gives_up_after_the_wait() {
        let path = temp_lock("timeout");
        let _held = SyncLock::acquire(&path).await.unwrap();
        let err = SyncLock::acquire_with_timeout(&path, Duration::from_millis(300)).await.unwrap_err();
        assert!(err.to_string().contains("still syncing"), "{err}");
    }

    #[tokio::test]
    async fn dropping_releases_it() {
        let path = temp_lock("drop");
        drop(SyncLock::acquire(&path).await.unwrap());
        SyncLock::acquire_with_timeout(&path, Duration::ZERO).await.unwrap();
    }

    const CHILD_LOCK: &str = "WARDEN_SYNC_LOCK_CHILD";

    /// Run by `holds_across_processes` in a child process: takes the lock, says so, holds it a
    /// moment. Does nothing in a normal test run.
    #[tokio::test]
    async fn child_holds_the_lock() {
        let Ok(path) = std::env::var(CHILD_LOCK) else { return };
        let path = PathBuf::from(path);
        let _held = SyncLock::acquire(&path).await.unwrap();
        std::fs::write(path.with_extension("held"), "").unwrap();
        tokio::time::sleep(Duration::from_millis(800)).await;
    }

    #[tokio::test]
    async fn holds_across_processes() {
        let path = temp_lock("process");
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "lock::tests::child_holds_the_lock", "--nocapture"])
            .env(CHILD_LOCK, &path)
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let marker = path.with_extension("held");
        let deadline = Instant::now() + Duration::from_secs(20);
        while !marker.exists() {
            assert!(Instant::now() < deadline, "the child never took the lock");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(SyncLock::acquire_with_timeout(&path, Duration::ZERO).await.is_err(), "the child's lock is seen here");
        let started = Instant::now();
        let _mine = SyncLock::acquire(&path).await.unwrap();
        assert!(started.elapsed() >= Duration::from_millis(300), "{:?}", started.elapsed());
        assert!(child.wait().unwrap().success());
    }
}
