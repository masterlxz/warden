//! Decentralized vault + `config.toml` sync via Arweave, paid through the TruthID app's `pin()`
//! mechanism (P37) — manual "Send"/"Pull", incremental (hash-per-file diff), everything encrypted
//! client-side before it ever reaches TruthID/Arweave. See `project/ARCHITECTURE.md` ("Sync
//! descentralizado (Fase 4)") for the full design rationale.
//!
//! `SyncEngine` is the one surface callers (desktop, CLI) need — everything else in this crate is
//! internal plumbing it composes: `manifest` (on-disk secrets/tracking state), `diff` (what
//! changed), `bundle` (the encrypted envelope for one push), `arweave` (GraphQL discovery + tx
//! fetch), `push`/`pull` (the two directions), `pairing` (how the vault key spreads to a new
//! device without ever touching TruthID or Arweave).

pub mod arweave;
pub mod bundle;
pub mod diff;
pub mod manifest;
pub mod pairing;
pub mod paths;
pub mod pull;
pub mod push;
pub mod storage_provider;

use std::net::Ipv4Addr;
use std::path::PathBuf;

use warden_core::memory::Vault;

pub use arweave::ArweaveClient;
pub use manifest::{SyncManifest, SyncSecrets};
pub use pull::PullOutcome;
pub use push::PushOutcome;
pub use storage_provider::DecentralizedVaultProvider;

#[derive(Debug)]
pub struct SyncStatus {
    pub paired: bool,
    pub device_id: Option<String>,
    pub owner_address: Option<String>,
    pub last_tx_id: Option<String>,
    pub last_synced_at_ms: Option<i64>,
    pub pending_vault_changes: usize,
    pub pending_config_changed: bool,
}

pub struct SyncEngine {
    vault: Vault,
    config_path: PathBuf,
    secrets_path: PathBuf,
    manifest_path: PathBuf,
    arweave: ArweaveClient,
}

impl SyncEngine {
    pub fn new(vault_root: PathBuf, config_path: PathBuf, secrets_path: PathBuf, manifest_path: PathBuf) -> Self {
        Self { vault: Vault::new(vault_root), config_path, secrets_path, manifest_path, arweave: ArweaveClient::new_default() }
    }

    /// Points this engine at a different Arweave gateway — used by tests to target a local fake
    /// gateway instead of the real network.
    pub fn with_arweave_client(mut self, arweave: ArweaveClient) -> Self {
        self.arweave = arweave;
        self
    }

    pub fn is_initialized(&self) -> bool {
        self.secrets_path.exists()
    }

    /// Generates a fresh vault key on this device — the very first device in a sync group calls
    /// this; every other device instead calls `pairing_join`/`pairing_join_with_hosts` to receive
    /// the same key from an already-initialized one.
    pub fn init_fresh(&self) -> anyhow::Result<()> {
        if self.is_initialized() {
            anyhow::bail!("sync já foi inicializado neste dispositivo ({})", self.secrets_path.display());
        }
        manifest::save_secrets(&self.secrets_path, &manifest::generate_secrets())?;
        manifest::save_manifest(&self.manifest_path, &manifest::SyncManifest { version: 1, ..Default::default() })?;
        Ok(())
    }

    pub fn status(&self) -> anyhow::Result<SyncStatus> {
        let secrets = manifest::load_secrets(&self.secrets_path)?;
        let manifest = manifest::load_manifest(&self.manifest_path)?;

        let (pending_vault_changes, pending_config_changed) = if secrets.is_some() {
            let diff = diff::diff_vault(&self.vault, &manifest)?;
            let config_bytes = std::fs::read(&self.config_path).ok();
            (diff.added_or_modified.len() + diff.deleted.len(), diff::config_changed(config_bytes.as_deref(), &manifest))
        } else {
            (0, false)
        };

        Ok(SyncStatus {
            paired: manifest.is_paired(),
            device_id: secrets.map(|s| s.device_id),
            owner_address: manifest.owner_address,
            last_tx_id: manifest.last_tx_id,
            last_synced_at_ms: manifest.last_synced_at_ms,
            pending_vault_changes,
            pending_config_changed,
        })
    }

    /// Step 1 of Send: computes the diff, builds+encrypts the bundle, starts a `PendingPin`.
    /// `None` means nothing changed. The caller renders `begin.pending.qr_payload_json()` as a QR
    /// (or ASCII, on the CLI) *before* calling `finish_push` — showing the code and blocking on
    /// the phone are deliberately separate steps.
    pub fn begin_push(&self) -> anyhow::Result<Option<push::BeginPushResult>> {
        let secrets = self.load_secrets()?;
        let manifest = manifest::load_manifest(&self.manifest_path)?;
        push::begin_push(&self.vault, &self.config_path, &secrets, &manifest)
    }

    /// Step 2 of Send: waits for the TruthID phone to pin the bundle, then persists the updated
    /// manifest.
    pub async fn finish_push(&self, begin: push::BeginPushResult) -> anyhow::Result<push::PushOutcome> {
        let manifest = manifest::load_manifest(&self.manifest_path)?;
        let (outcome, new_manifest) = push::run_push(begin, &self.arweave, manifest).await?;
        manifest::save_manifest(&self.manifest_path, &new_manifest)?;
        Ok(outcome)
    }

    /// Same as `finish_push`, but sweeps only `hosts` for the TruthID phone — used by tests to
    /// target a fake phone on `127.0.0.1` instead of the real LAN (see
    /// `push::run_push_with_hosts`).
    pub async fn finish_push_with_hosts(
        &self,
        begin: push::BeginPushResult,
        hosts: Vec<Ipv4Addr>,
    ) -> anyhow::Result<push::PushOutcome> {
        let manifest = manifest::load_manifest(&self.manifest_path)?;
        let (outcome, new_manifest) = push::run_push_with_hosts(begin, &self.arweave, manifest, hosts).await?;
        manifest::save_manifest(&self.manifest_path, &new_manifest)?;
        Ok(outcome)
    }

    pub async fn pull(&self) -> anyhow::Result<pull::PullOutcome> {
        let secrets = self.load_secrets()?;
        let manifest = manifest::load_manifest(&self.manifest_path)?;
        let (outcome, new_manifest) = pull::pull(&self.arweave, &self.vault, &self.config_path, &secrets, manifest).await?;
        manifest::save_manifest(&self.manifest_path, &new_manifest)?;
        Ok(outcome)
    }

    /// Starts showing a pairing code and listening for another device to join — call on the
    /// device that already has a vault key (`is_initialized() == true`).
    pub async fn pairing_host(&self) -> anyhow::Result<pairing::PairingHost> {
        let secrets = self.load_secrets()?;
        let manifest = manifest::load_manifest(&self.manifest_path)?;
        pairing::PairingHost::start(&secrets, &manifest).await
    }

    /// Sweeps the LAN for a `PairingHost` showing `code`, then adopts the vault key it hands
    /// over. Errors if this device already has one — pairing an already-initialized device would
    /// silently discard its existing key and orphan whatever it had already pushed.
    pub async fn pairing_join(&self, code: &str) -> anyhow::Result<()> {
        self.adopt_joined_material(pairing::join(code).await?)
    }

    /// Same as `pairing_join`, but sweeps only `hosts` — used by tests.
    pub async fn pairing_join_with_hosts(&self, code: &str, hosts: Vec<Ipv4Addr>) -> anyhow::Result<()> {
        self.adopt_joined_material(pairing::join_with_hosts(code, hosts).await?)
    }

    fn load_secrets(&self) -> anyhow::Result<manifest::SyncSecrets> {
        manifest::load_secrets(&self.secrets_path)?
            .ok_or_else(|| anyhow::anyhow!("sync ainda não foi inicializado neste dispositivo — rode init_fresh ou pareie com um dispositivo existente primeiro"))
    }

    fn adopt_joined_material(&self, material: pairing::JoinedKeyMaterial) -> anyhow::Result<()> {
        if self.is_initialized() {
            anyhow::bail!("este dispositivo já tem sync configurado — parear agora sobrescreveria a chave existente");
        }
        let secrets = manifest::SyncSecrets { version: 1, device_id: manifest::generate_device_id(), vault_key: material.vault_key };
        manifest::save_secrets(&self.secrets_path, &secrets)?;

        let mut new_manifest = manifest::SyncManifest { version: 1, ..Default::default() };
        new_manifest.owner_address = material.owner_address;
        manifest::save_manifest(&self.manifest_path, &new_manifest)?;
        Ok(())
    }
}
