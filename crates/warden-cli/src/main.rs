use clap::Parser;
use warden_core::memory::Vault;

#[derive(Parser, Debug)]
#[command(name = "warden", version, about = "Warden — personal, model-agnostic AI agent")]
struct Cli {
    /// Path to the markdown vault (memory).
    #[arg(long, default_value = "vault")]
    vault_path: String,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let vault = Vault::new(cli.vault_path);

    // Orchestrator, model provider and channel loop land in the next steps
    // of Phase 1 (see project/PHASE.md). This scaffold just proves the
    // workspace wires together end to end.
    println!("Warden CLI — foundation scaffold.");
    println!("Vault path: {}", vault.root().display());

    Ok(())
}
