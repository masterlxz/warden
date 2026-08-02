use std::io::{self, Write};
use std::sync::Arc;

use anyhow::Context;
use clap::Parser;
use warden_core::memory::Vault;
use warden_core::model::openai::OpenAiProvider;
use warden_core::orchestrator::Orchestrator;

#[derive(Parser, Debug)]
#[command(name = "warden", version, about = "Warden — personal, model-agnostic AI agent")]
struct Cli {
    /// Path to the markdown vault (memory).
    #[arg(long, default_value = "vault")]
    vault_path: String,

    /// Model name passed to the provider.
    #[arg(long, default_value = "gpt-4o-mini")]
    model: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let api_key = std::env::var("OPENAI_API_KEY")
        .context("OPENAI_API_KEY not set — export it before running warden")?;

    let provider = OpenAiProvider::new(api_key, cli.model);
    let vault = Vault::new(cli.vault_path);
    let orchestrator = Orchestrator::new(Arc::new(provider), vault);

    println!("Warden — talk to it below (Ctrl+D or 'exit' to quit).\n");

    let stdin = io::stdin();
    loop {
        print!("> ");
        io::stdout().flush()?;

        let mut input = String::new();
        if stdin.read_line(&mut input)? == 0 {
            break;
        }
        let input = input.trim();
        if input.is_empty() {
            continue;
        }
        if input == "exit" || input == "quit" {
            break;
        }

        match orchestrator.handle_message(input).await {
            Ok(response) => println!("{response}\n"),
            Err(err) => eprintln!("error: {err:#}\n"),
        }
    }

    Ok(())
}
