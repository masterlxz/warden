use std::io::{self, Write};
use std::path::PathBuf;

use clap::{Parser, ValueEnum};
use warden_bootstrap::{bootstrap, Overrides};
use warden_core::model::Message;

#[derive(ValueEnum, Clone, Copy, Debug)]
enum Provider {
    Gemini,
    Openai,
}

impl From<Provider> for warden_bootstrap::Provider {
    fn from(p: Provider) -> Self {
        match p {
            Provider::Gemini => warden_bootstrap::Provider::Gemini,
            Provider::Openai => warden_bootstrap::Provider::Openai,
        }
    }
}

#[derive(Parser, Debug)]
#[command(name = "warden", version, about = "Warden — personal, model-agnostic AI agent")]
struct Cli {
    /// Path to the markdown vault (memory). Overrides the config file; defaults to "vault".
    #[arg(long)]
    vault_path: Option<String>,

    /// Which model provider to talk to. Overrides the config file; defaults to gemini.
    #[arg(long, value_enum)]
    provider: Option<Provider>,

    /// Model name passed to the provider. Overrides the config file; provider-specific default otherwise.
    #[arg(long)]
    model: Option<String>,

    /// Path to the config file (TOML). Defaults to the OS config dir (e.g. ~/.config/warden/config.toml on Linux).
    #[arg(long)]
    config: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let orchestrator = bootstrap(
        cli.config.as_deref(),
        Overrides { provider: cli.provider.map(Into::into), model: cli.model, vault_path: cli.vault_path },
        PathBuf::from("vault"),
    )?;

    println!("Warden — talk to it below (Ctrl+D or 'exit' to quit).\n");

    let stdin = io::stdin();
    let mut history: Vec<Message> = Vec::new();
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

        match orchestrator.handle_message(&history, input).await {
            Ok(response) => {
                println!("{response}\n");
                history.push(Message::user(input));
                history.push(Message::assistant(response));
            }
            Err(err) => eprintln!("error: {err:#}\n"),
        }
    }

    Ok(())
}
