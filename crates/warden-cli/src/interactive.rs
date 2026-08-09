//! The rich terminal loop, used only when stdin is a real TTY (see `main.rs`'s `IsTerminal`
//! check) — readline-style editing with persisted history, a spinner while the model is
//! thinking, and markdown-rendered responses. The plain non-interactive loop in `main.rs` is
//! untouched and still handles piped stdin (scripted use, the existing process-level tests).

use std::path::Path;
use std::time::Duration;

use indicatif::ProgressBar;
use owo_colors::OwoColorize;
use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;
use termimad::MadSkin;
use warden_core::model::Message;
use warden_core::orchestrator::Orchestrator;

pub async fn run(orchestrator: &Orchestrator, history_path: Option<&Path>) -> anyhow::Result<()> {
    let mut editor = DefaultEditor::new()?;
    if let Some(path) = history_path {
        let _ = editor.load_history(path);
    }

    let skin = MadSkin::default();
    let mut history: Vec<Message> = Vec::new();

    println!("Warden — talk to it below (\u{2191} for history, Ctrl+D or 'exit' to quit).\n");

    loop {
        match editor.readline("> ") {
            Ok(line) => {
                let input = line.trim();
                if input.is_empty() {
                    continue;
                }
                let _ = editor.add_history_entry(input);
                if input == "exit" || input == "quit" {
                    break;
                }

                let spinner = ProgressBar::new_spinner();
                spinner.enable_steady_tick(Duration::from_millis(100));
                spinner.set_message("Thinking...");

                let result = orchestrator.handle_message(&history, input).await;
                spinner.finish_and_clear();

                match result {
                    Ok(outcome) => {
                        skin.print_text(&outcome.content);
                        println!();
                        if let Some(usage) = &outcome.usage {
                            println!(
                                "{}",
                                format!(
                                    "  ({} prompt + {} completion = {} tokens)",
                                    usage.prompt_tokens, usage.completion_tokens, usage.total_tokens
                                )
                                .dimmed()
                            );
                            println!();
                        }
                        history.push(Message::user(input));
                        history.push(Message::assistant(outcome.content));
                    }
                    Err(err) => eprintln!("error: {err:#}\n"),
                }
            }
            // A bare Ctrl+C cancels the current line and reprompts, same as bash's readline —
            // it doesn't quit the whole session (only Ctrl+D / 'exit' do).
            Err(ReadlineError::Interrupted) => {
                println!();
                continue;
            }
            Err(ReadlineError::Eof) => break,
            Err(err) => {
                eprintln!("error reading input: {err}");
                break;
            }
        }
    }

    if let Some(path) = history_path {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = editor.save_history(path);
    }

    Ok(())
}
