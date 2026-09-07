use std::path::Path;

use warden_core::memory::Vault;

use crate::arweave::ArweaveClient;
use crate::bundle;
use crate::diff::sha256_hex;
use crate::manifest::{SyncDirection, SyncManifest, SyncSecrets};

#[derive(Debug)]
pub struct PullOutcome {
    pub tx_id: Option<String>,
    pub files_written: usize,
    pub files_deleted: usize,
    pub config_updated: bool,
    pub warnings: Vec<String>,
}

impl PullOutcome {
    fn up_to_date(tx_id: Option<String>, warning: &str) -> Self {
        Self { tx_id, files_written: 0, files_deleted: 0, config_updated: false, warnings: vec![warning.to_string()] }
    }
}

/// Fetches the latest snapshot published under `manifest.owner_address` (if any newer than what
/// we already applied) and writes it onto `vault`/`config_path`. Returns the updated manifest for
/// the caller to persist — this crate never assumes where the manifest file lives.
pub async fn pull(
    arweave: &ArweaveClient,
    vault: &Vault,
    config_path: &Path,
    secrets: &SyncSecrets,
    manifest: SyncManifest,
) -> anyhow::Result<(PullOutcome, SyncManifest)> {
    let Some(owner) = manifest.owner_address.clone() else {
        anyhow::bail!(
            "ainda não pareado — pareie com um dispositivo que já sincronizou, ou envie uma vez a partir deste dispositivo primeiro"
        );
    };

    let Some(tx_id) = arweave.latest_tx_by_owner(&owner).await? else {
        return Ok((PullOutcome::up_to_date(None, "nenhum snapshot foi publicado ainda"), manifest));
    };

    if manifest.last_tx_id.as_deref() == Some(tx_id.as_str()) {
        return Ok((PullOutcome::up_to_date(Some(tx_id), "já está atualizado"), manifest));
    }

    let blob = arweave.fetch_tx_data(&tx_id).await?;
    let decoded_bundle = bundle::decrypt_bundle(&blob, &secrets.vault_key)?;

    if decoded_bundle.manifest_counter <= manifest.manifest_counter {
        let warning =
            "o snapshot remoto parece mais antigo (ou igual) ao que já foi aplicado — ignorado para não sobrescrever um estado local mais novo"
                .to_string();
        return Ok((PullOutcome::up_to_date(Some(tx_id), &warning), manifest));
    }

    let mut warnings = Vec::new();
    for relative in decoded_bundle.vault_files.keys().chain(decoded_bundle.deleted_vault_files.iter()) {
        let current_hash = std::fs::read(vault.root().join(relative)).ok().map(|bytes| sha256_hex(&bytes));
        let known_hash = manifest.vault_files.get(relative).cloned();
        if current_hash != known_hash {
            warnings.push(format!("mudança local em {relative} foi sobrescrita pelo pull"));
        }
    }
    if decoded_bundle.config_toml.is_some() || decoded_bundle.config_deleted {
        let current_hash = std::fs::read(config_path).ok().map(|bytes| sha256_hex(&bytes));
        if current_hash != manifest.config_hash {
            warnings.push("mudança local em config.toml foi sobrescrita pelo pull".to_string());
        }
    }

    let report = bundle::apply_bundle(&decoded_bundle, vault, config_path)?;
    let new_manifest = bundle::fold_into_manifest(manifest, &decoded_bundle, tx_id.clone(), SyncDirection::Pull)?;

    let outcome = PullOutcome {
        tx_id: Some(tx_id),
        files_written: report.files_written,
        files_deleted: report.files_deleted,
        config_updated: report.config_updated,
        warnings,
    };
    Ok((outcome, new_manifest))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::Path as AxumPath;
    use axum::routing::{get, post};
    use axum::Router;

    fn temp_vault(suffix: &str) -> Vault {
        let dir = std::env::temp_dir().join(format!(
            "warden-sync-pull-test-{suffix}-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        Vault::new(dir)
    }

    /// A tiny fake Arweave gateway: `/graphql` always answers "latest tx is FAKE_TX_ID" for any
    /// owner query, `/FAKE_TX_ID` serves whatever encrypted bytes the test configured.
    async fn spawn_fake_gateway(blob: Vec<u8>) -> String {
        let router = Router::new()
            .route(
                "/graphql",
                post(|| async {
                    axum::Json(serde_json::json!({
                        "data": { "transactions": { "edges": [ { "node": { "id": "FAKE_TX_ID" } } ] } }
                    }))
                }),
            )
            .route(
                "/{tx_id}",
                get(move |AxumPath(_tx_id): AxumPath<String>| {
                    let blob = blob.clone();
                    async move { blob }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn pull_without_pairing_errors_clearly() {
        let vault = temp_vault("unpaired");
        let secrets = crate::manifest::generate_secrets();
        let manifest = SyncManifest::default();
        let arweave = ArweaveClient::new("http://127.0.0.1:1/graphql", "http://127.0.0.1:1");
        let config_path = std::env::temp_dir().join("warden-sync-pull-test-no-config.toml");

        let err = pull(&arweave, &vault, &config_path, &secrets, manifest).await.unwrap_err();
        assert!(err.to_string().contains("pareado"));
    }

    #[tokio::test]
    async fn pull_applies_a_newer_bundle_and_updates_the_manifest() {
        let secrets = crate::manifest::generate_secrets();
        let source = temp_vault("pull-source");
        source.write("notes/todo.md", "buy milk").unwrap();
        let diff = crate::diff::diff_vault(&source, &SyncManifest::default()).unwrap();
        let sync_bundle = bundle::build_bundle(&source, None, &diff, 1, &secrets.device_id).unwrap();
        let encrypted = bundle::encrypt_bundle(&sync_bundle, &secrets.vault_key).unwrap();

        let base_url = spawn_fake_gateway(encrypted).await;
        let arweave = ArweaveClient::new(format!("{base_url}/graphql"), base_url);

        let manifest = SyncManifest { owner_address: Some("wallet-abc".to_string()), ..Default::default() };

        let dest = temp_vault("pull-dest");
        let config_path = std::env::temp_dir().join("warden-sync-pull-test-config.toml");

        let (outcome, new_manifest) = pull(&arweave, &dest, &config_path, &secrets, manifest).await.unwrap();
        assert_eq!(outcome.files_written, 1);
        assert_eq!(outcome.tx_id.as_deref(), Some("FAKE_TX_ID"));
        assert_eq!(dest.read("notes/todo.md").unwrap(), "buy milk");
        assert_eq!(new_manifest.last_tx_id.as_deref(), Some("FAKE_TX_ID"));
        assert_eq!(new_manifest.last_sync_direction, Some(SyncDirection::Pull));
    }

    #[tokio::test]
    async fn pull_is_a_no_op_when_already_up_to_date() {
        let secrets = crate::manifest::generate_secrets();
        let source = temp_vault("noop-source");
        let diff = crate::diff::diff_vault(&source, &SyncManifest::default()).unwrap();
        let sync_bundle = bundle::build_bundle(&source, None, &diff, 1, &secrets.device_id).unwrap();
        let encrypted = bundle::encrypt_bundle(&sync_bundle, &secrets.vault_key).unwrap();

        let base_url = spawn_fake_gateway(encrypted).await;
        let arweave = ArweaveClient::new(format!("{base_url}/graphql"), base_url);

        let manifest = SyncManifest {
            owner_address: Some("wallet-abc".to_string()),
            last_tx_id: Some("FAKE_TX_ID".to_string()),
            ..Default::default()
        };

        let dest = temp_vault("noop-dest");
        let config_path = std::env::temp_dir().join("warden-sync-pull-test-noop-config.toml");

        let (outcome, _) = pull(&arweave, &dest, &config_path, &secrets, manifest).await.unwrap();
        assert_eq!(outcome.files_written, 0);
        assert!(outcome.warnings.iter().any(|w| w.contains("atualizado")));
    }
}
