use std::io::{self, Write};
use std::sync::Arc;

use anyhow::Context;
use clap::{Parser, ValueEnum};
use warden_core::memory::Vault;
use warden_core::model::gemini::GeminiProvider;
use warden_core::model::openai::OpenAiProvider;
use warden_core::model::ModelProvider;
use warden_core::orchestrator::Orchestrator;

#[derive(ValueEnum, Clone, Debug)]
enum Provider {
    Gemini,
    Openai,
}

#[derive(Parser, Debug)]
#[command(name = "warden", version, about = "Warden — personal, model-agnostic AI agent")]
struct Cli {
    /// Path to the markdown vault (memory).
    #[arg(long, default_value = "vault")]
    vault_path: String,

    /// Which model provider to talk to.
    #[arg(long, value_enum, default_value_t = Provider::Gemini)]
    provider: Provider,

    /// Model name passed to the provider (defaults depend on --provider).
    #[arg(long)]
    model: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let model_provider: Arc<dyn ModelProvider> = match cli.provider {
        Provider::Gemini => {
            let api_key = std::env::var("GEMINI_API_KEY").context(
                "GEMINI_API_KEY not set — get a free key at https://aistudio.google.com/apikey",
            )?;
            let model = cli.model.unwrap_or_else(|| "gemini-2.5-flash".to_string());
            Arc::new(GeminiProvider::new(api_key, model))
        }
        Provider::Openai => {
            let api_key = std::env::var("OPENAI_API_KEY")
                .context("OPENAI_API_KEY not set — export it before running warden")?;
            let model = cli.model.unwrap_or_else(|| "gpt-4o-mini".to_string());
            Arc::new(OpenAiProvider::new(api_key, model))
        }
    };

    let vault = Vault::new(cli.vault_path);
    let orchestrator = Orchestrator::new(model_provider, vault);

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
