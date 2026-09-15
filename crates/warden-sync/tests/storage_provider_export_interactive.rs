//! Proves `DecentralizedVaultProvider::export_all_interactive` (P61 follow-up) actually reaches
//! Arweave — pulling a newer published bundle into the local vault before snapshotting it — when
//! this device is initialized and paired. Same fake-gateway idiom as `pull.rs`'s own tests (no
//! fake phone needed here: pulling never requires phone approval, only publishing does).

use std::sync::Arc;

use axum::extract::Path as AxumPath;
use axum::routing::{get, post};
use axum::Router;
use warden_core::memory::Vault;
use warden_core::storage::StorageProvider;
use warden_sync::manifest::{self, SyncManifest};
use warden_sync::{ArweaveClient, DecentralizedVaultProvider, SyncEngine};

fn temp_base(suffix: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "warden-sync-export-interactive-{suffix}-{}",
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ))
}

/// Same fake gateway as `pull.rs`'s own tests: `/graphql` always answers "latest tx is
/// FAKE_TX_ID", `/FAKE_TX_ID` serves whatever encrypted bytes the test configured.
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

/// Builds a `DecentralizedVaultProvider` that's initialized+paired against `gateway_base_url`,
/// with an empty local vault — used to prove the interactive export pulls before snapshotting.
fn paired_provider(suffix: &str, secrets: manifest::SyncSecrets, gateway_base_url: &str) -> (DecentralizedVaultProvider, std::path::PathBuf) {
    let base = temp_base(suffix);
    let vault_root = base.join("vault");
    let config_path = base.join("config.toml");
    let secrets_path = base.join("sync_secrets.json");
    let manifest_path = base.join("sync_manifest.json");

    manifest::save_secrets(&secrets_path, &secrets).unwrap();
    let manifest = SyncManifest { owner_address: Some("wallet-abc".to_string()), ..Default::default() };
    manifest::save_manifest(&manifest_path, &manifest).unwrap();

    let arweave = ArweaveClient::new(format!("{gateway_base_url}/graphql"), gateway_base_url.to_string());
    let sync = SyncEngine::new(vault_root.clone(), config_path, secrets_path, manifest_path).with_arweave_client(arweave);
    let vault = Arc::new(Vault::new(vault_root.clone()));
    (DecentralizedVaultProvider::new(vault, sync), vault_root)
}

#[tokio::test]
async fn export_all_interactive_pulls_the_latest_remote_bundle_before_exporting() {
    let secrets = manifest::generate_secrets();

    let remote_source = Vault::new(temp_base("remote-source"));
    remote_source.write("notes/todo.md", "buy milk").unwrap();
    let diff = warden_sync::diff::diff_vault(&remote_source, &SyncManifest::default()).unwrap();
    let bundle = warden_sync::bundle::build_bundle(&remote_source, None, &diff, 1, &secrets.device_id).unwrap();
    let encrypted = warden_sync::bundle::encrypt_bundle(&bundle, &secrets.vault_key).unwrap();

    let gateway_url = spawn_fake_gateway(encrypted).await;
    let (provider, _vault_root) = paired_provider("pull-ok", secrets, &gateway_url);

    // Local vault starts empty — the only way "notes/todo.md" ends up in the exported snapshot is
    // if `export_all_interactive` actually pulled the remote bundle first.
    let snapshot = provider.export_all_interactive(None).await.unwrap();
    assert_eq!(snapshot.get("notes/todo.md").map(|b| String::from_utf8_lossy(b).to_string()), Some("buy milk".to_string()));
}

#[tokio::test]
async fn export_all_interactive_propagates_a_real_pull_failure_instead_of_falling_back_to_stale_local_data() {
    let secrets = manifest::generate_secrets();
    // Unreachable gateway (nothing bound on this port) — a genuine network failure, not the
    // "never paired" case that's supposed to be silently skipped.
    let (provider, vault_root) = paired_provider("pull-fail", secrets, "http://127.0.0.1:1");
    std::fs::create_dir_all(&vault_root).unwrap();
    std::fs::write(vault_root.join("local-only.md"), "should not be exported on a failed pull").unwrap();

    let err = provider.export_all_interactive(None).await.unwrap_err();
    assert!(!err.to_string().is_empty());
}
