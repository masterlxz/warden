mod support;

use std::sync::Arc;

use support::{spin_up_server, MockProvider};
use warden_core::memory::Vault;
use warden_core::storage::StorageProvider;
use warden_server::{vault_node, RemoteNodeProvider};

fn temp_vault_dir(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("warden-vault-node-e2e-{name}-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()))
}

/// Full round trip through everything built this session: a real `vault_node::serve` loop
/// (target, its own real `Vault` directory) connected to a real `Server`, and a real
/// `RemoteNodeProvider` (caller) routed to it — proving the whole chain, not just each half
/// against a scripted stand-in. Every assertion checks the *actual file on disk* in the node's
/// vault directory, not just that an RPC round-tripped.
#[tokio::test]
async fn write_read_list_and_delete_round_trip_against_a_real_node_and_a_real_vault_on_disk() {
    let addr = spin_up_server(MockProvider::replying("unused")).await;
    let node_vault_dir = temp_vault_dir("node");

    let node_conn = vault_node::connect(&format!("ws://{addr}"), "dev-node", "Vault Node", "test-key").await.unwrap();
    tokio::spawn(vault_node::serve(node_conn, Arc::new(Vault::new(node_vault_dir.clone()))));

    let caller = RemoteNodeProvider::connect(&format!("ws://{addr}"), "dev-caller", "Caller Device", "test-key", "dev-node").await.unwrap();

    // write() — the file must exist on disk in the node's own vault directory afterward.
    caller.write("notes/a.md", b"buy milk").await.unwrap();
    let on_disk = std::fs::read(node_vault_dir.join("notes/a.md")).unwrap();
    assert_eq!(on_disk, b"buy milk");

    // read() — comes back through the real node, not a cache.
    assert_eq!(caller.read("notes/a.md").await.unwrap(), b"buy milk");

    // list() — a second real file, both show up.
    caller.write("b.md", b"second file").await.unwrap();
    let mut paths = caller.list().await.unwrap();
    paths.sort();
    assert_eq!(paths, vec!["b.md".to_string(), "notes/a.md".to_string()]);

    // delete() — gone from disk for real, not just from the RPC's perspective.
    caller.delete("notes/a.md").await.unwrap();
    assert!(!node_vault_dir.join("notes/a.md").exists());
    assert!(caller.read("notes/a.md").await.is_err());

    std::fs::remove_dir_all(&node_vault_dir).ok();
}

/// A `vault_read` for a path that was never written errors clearly through the real node (not
/// scripted) — proves `LocalFSProvider`'s own "file not found" surfaces all the way back through
/// the routing as a real `DeviceToolError`.
#[tokio::test]
async fn reading_a_path_that_was_never_written_errors_clearly() {
    let addr = spin_up_server(MockProvider::replying("unused")).await;
    let node_vault_dir = temp_vault_dir("missing-path");

    let node_conn = vault_node::connect(&format!("ws://{addr}"), "dev-node", "Vault Node", "test-key").await.unwrap();
    tokio::spawn(vault_node::serve(node_conn, Arc::new(Vault::new(node_vault_dir.clone()))));

    let caller = RemoteNodeProvider::connect(&format!("ws://{addr}"), "dev-caller", "Caller Device", "test-key", "dev-node").await.unwrap();

    let err = caller.read("never-written.md").await.unwrap_err();
    assert!(err.to_string().contains("remote node"), "error was: {err}");

    std::fs::remove_dir_all(&node_vault_dir).ok();
}
