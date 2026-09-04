//! The rich terminal loop, used only when stdin is a real TTY (see `main.rs`'s `IsTerminal`
//! check) — a bordered input box and a "thinking" status box, both drawn with `ratatui`'s
//! *inline* viewport (not full-screen/alt-screen: the rest of the terminal's scrollback stays
//! completely normal, matching how the Claude Code CLI behaves), plus real token-by-token
//! streaming of the assistant's response. The plain non-interactive loop in `main.rs` is
//! untouched and still handles piped stdin (scripted use, the existing process-level tests).
//!
//! There's no line-editing library here (`rustyline` is gone) — a bordered, redrawn-every-frame
//! input box needs to own the terminal region itself, which a separate line-editing library
//! doesn't compose with. `LineEditor` below is a small hand-rolled one; its cursor/history logic
//! is unit-tested directly (no terminal involved). The line-editing history file this module
//! reads/writes is a plain newline-per-entry text file — a new, simpler format than rustyline's
//! own, since only the on-disk *format* changed, not what it's for (still just remembered input
//! lines, not the conversation itself, which was never persisted here).

use std::io::{self, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use crossterm::event::{Event, EventStream, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use futures_util::StreamExt;
use owo_colors::OwoColorize;
use ratatui::backend::CrosstermBackend;
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Paragraph};
use ratatui::{Terminal, TerminalOptions, Viewport};
use tokio::sync::mpsc;
use unicode_width::UnicodeWidthStr;
use warden_core::model::{Message, StreamEvent};
use warden_core::orchestrator::{MessageOutcome, Orchestrator};

/// Height (in terminal rows) of the inline viewport used for both the input box and the
/// "thinking" box — a single line of content plus its top/bottom border.
const VIEWPORT_HEIGHT: u16 = 3;

const SPINNER_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const MAX_HISTORY_ENTRIES: usize = 500;

type CliTerminal = Terminal<CrosstermBackend<io::Stdout>>;

/// Enables raw mode for the lifetime of this guard and unconditionally restores the terminal on
/// drop — including on an early `?`-return or a panic partway through a turn, so a crash never
/// leaves the user's shell stuck reading raw keystrokes.
struct RawModeGuard;

impl RawModeGuard {
    fn enable() -> anyhow::Result<Self> {
        enable_raw_mode()?;
        Ok(Self)
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
    }
}

/// A small hand-rolled line editor: a UTF-8-aware cursor over a `Vec<char>`, plus a persisted
/// history list with the usual up/down recall (including restoring an in-progress draft when
/// paging back down past the newest history entry). Every method here is plain, terminal-free
/// logic — see the unit tests below.
struct LineEditor {
    buffer: Vec<char>,
    cursor: usize,
    history: Vec<String>,
    history_index: Option<usize>,
    draft: Option<Vec<char>>,
}

impl LineEditor {
    fn new(history: Vec<String>) -> Self {
        Self { buffer: Vec::new(), cursor: 0, history, history_index: None, draft: None }
    }

    fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    fn as_str(&self) -> String {
        self.buffer.iter().collect()
    }

    /// The portion of the buffer before the cursor — used to compute the cursor's on-screen
    /// column, since a wide/combining character can take more than one terminal cell.
    fn prefix(&self) -> String {
        self.buffer[..self.cursor].iter().collect()
    }

    fn insert(&mut self, c: char) {
        self.buffer.insert(self.cursor, c);
        self.cursor += 1;
    }

    fn backspace(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.buffer.remove(self.cursor);
        }
    }

    fn delete_forward(&mut self) {
        if self.cursor < self.buffer.len() {
            self.buffer.remove(self.cursor);
        }
    }

    fn move_left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    fn move_right(&mut self) {
        if self.cursor < self.buffer.len() {
            self.cursor += 1;
        }
    }

    fn move_home(&mut self) {
        self.cursor = 0;
    }

    fn move_end(&mut self) {
        self.cursor = self.buffer.len();
    }

    fn clear_line(&mut self) {
        self.buffer.clear();
        self.cursor = 0;
    }

    /// Deletes the word immediately before the cursor (Ctrl+W) — skips trailing spaces first,
    /// then deletes back to the next space or the start of the line.
    fn delete_word_backward(&mut self) {
        let mut i = self.cursor;
        while i > 0 && self.buffer[i - 1] == ' ' {
            i -= 1;
        }
        while i > 0 && self.buffer[i - 1] != ' ' {
            i -= 1;
        }
        self.buffer.drain(i..self.cursor);
        self.cursor = i;
    }

    fn history_up(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let next_index = match self.history_index {
            None => {
                self.draft = Some(self.buffer.clone());
                self.history.len() - 1
            }
            Some(0) => return,
            Some(i) => i - 1,
        };
        self.history_index = Some(next_index);
        self.buffer = self.history[next_index].chars().collect();
        self.cursor = self.buffer.len();
    }

    fn history_down(&mut self) {
        match self.history_index {
            None => {}
            Some(i) if i + 1 < self.history.len() => {
                self.history_index = Some(i + 1);
                self.buffer = self.history[i + 1].chars().collect();
                self.cursor = self.buffer.len();
            }
            Some(_) => {
                self.history_index = None;
                self.buffer = self.draft.take().unwrap_or_default();
                self.cursor = self.buffer.len();
            }
        }
    }

    /// Takes the current buffer as the submitted line, resetting the editor for the next one.
    fn submit(&mut self) -> String {
        let text = self.as_str();
        self.buffer.clear();
        self.cursor = 0;
        self.history_index = None;
        self.draft = None;
        text
    }
}

fn load_history(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .map(|contents| contents.lines().filter(|line| !line.is_empty()).map(str::to_string).collect())
        .unwrap_or_default()
}

fn save_history(path: &Path, history: &[String]) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let start = history.len().saturating_sub(MAX_HISTORY_ENTRIES);
    let _ = std::fs::write(path, history[start..].join("\n"));
}

fn render_input_box(frame: &mut ratatui::Frame, editor: &LineEditor) {
    let area = frame.area();
    let block = Block::bordered().border_style(Style::default().fg(Color::Rgb(230, 126, 34))).title(" Warden ");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(Paragraph::new(editor.as_str()), inner);

    let cursor_col = inner.x + UnicodeWidthStr::width(editor.prefix().as_str()) as u16;
    frame.set_cursor_position((cursor_col, inner.y));
}

fn render_thinking_box(frame: &mut ratatui::Frame, elapsed: Duration, spinner_frame: usize) {
    let area = frame.area();
    let block = Block::bordered().border_style(Style::default().fg(Color::Rgb(241, 196, 15))).title(" Warden ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let glyph = SPINNER_FRAMES[spinner_frame % SPINNER_FRAMES.len()];
    let line = format!("{glyph} pensando... ({}s — Ctrl+C para interromper)", elapsed.as_secs());
    frame.render_widget(Paragraph::new(line).style(Style::default().fg(Color::DarkGray)), inner);
}

fn new_inline_terminal() -> anyhow::Result<CliTerminal> {
    let backend = CrosstermBackend::new(io::stdout());
    Ok(Terminal::with_options(backend, TerminalOptions { viewport: Viewport::Inline(VIEWPORT_HEIGHT) })?)
}

fn is_ctrl_c(event: &Event) -> bool {
    matches!(event, Event::Key(key) if key.kind == KeyEventKind::Press && key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
}

enum LineOutcome {
    Submitted(String),
    Exit,
}

/// Drives the bordered input box until the user submits a line, quits, or the terminal's event
/// stream ends (e.g. stdin closed unexpectedly). Owns its own short-lived `Terminal` — by the
/// time this returns, the box has been cleared, so whatever's printed next starts from a clean,
/// normal line of scrollback.
async fn read_line(line_history: &mut Vec<String>, term_events: &mut EventStream) -> anyhow::Result<LineOutcome> {
    let mut editor = LineEditor::new(line_history.clone());
    let mut terminal = new_inline_terminal()?;

    loop {
        terminal.draw(|frame| render_input_box(frame, &editor))?;

        match term_events.next().await {
            Some(Ok(Event::Key(key))) if key.kind == KeyEventKind::Press => match (key.code, key.modifiers) {
                (KeyCode::Enter, _) => {
                    let text = editor.submit();
                    terminal.clear()?;
                    if !text.trim().is_empty() {
                        line_history.push(text.clone());
                    }
                    return Ok(LineOutcome::Submitted(text));
                }
                // Matches GNU readline: Ctrl+D only quits on an empty line, so it can't be
                // mistaken for "discard what I just typed" — same reason rustyline's `Eof` never
                // fired on a non-empty buffer before this module dropped it.
                (KeyCode::Char('d'), m) if m.contains(KeyModifiers::CONTROL) && editor.is_empty() => {
                    terminal.clear()?;
                    return Ok(LineOutcome::Exit);
                }
                (KeyCode::Char('c'), m) if m.contains(KeyModifiers::CONTROL) => editor.clear_line(),
                (KeyCode::Char('a'), m) if m.contains(KeyModifiers::CONTROL) => editor.move_home(),
                (KeyCode::Char('e'), m) if m.contains(KeyModifiers::CONTROL) => editor.move_end(),
                (KeyCode::Char('u'), m) if m.contains(KeyModifiers::CONTROL) => editor.clear_line(),
                (KeyCode::Char('w'), m) if m.contains(KeyModifiers::CONTROL) => editor.delete_word_backward(),
                (KeyCode::Char(c), m) if !m.contains(KeyModifiers::CONTROL) && !m.contains(KeyModifiers::ALT) => editor.insert(c),
                (KeyCode::Backspace, _) => editor.backspace(),
                (KeyCode::Delete, _) => editor.delete_forward(),
                (KeyCode::Left, _) => editor.move_left(),
                (KeyCode::Right, _) => editor.move_right(),
                (KeyCode::Home, _) => editor.move_home(),
                (KeyCode::End, _) => editor.move_end(),
                (KeyCode::Up, _) => editor.history_up(),
                (KeyCode::Down, _) => editor.history_down(),
                _ => {}
            },
            Some(Ok(_)) => {}
            Some(Err(err)) => return Err(err.into()),
            None => {
                terminal.clear()?;
                return Ok(LineOutcome::Exit);
            }
        }
    }
}

/// Runs one full turn: spawns the streaming call, shows the "thinking" box until the first
/// content arrives (or the call finishes/fails with none), then prints the response as plain
/// text as it streams in. Returns `Ok(None)` if the user interrupted with Ctrl+C — nothing gets
/// appended to conversation history in that case, matching a turn that never happened.
async fn run_turn(orchestrator: &Orchestrator, history: &[Message], input: &str, term_events: &mut EventStream) -> anyhow::Result<Option<MessageOutcome>> {
    let (tx, mut rx) = mpsc::unbounded_channel::<StreamEvent>();
    let orchestrator = orchestrator.clone();
    let history_owned = history.to_vec();
    let input_owned = input.to_string();
    let mut handle = tokio::spawn(async move {
        orchestrator
            .handle_message_streaming(&history_owned, &input_owned, move |event| {
                let _ = tx.send(event.clone());
            })
            .await
    });

    let mut terminal = new_inline_terminal()?;
    let start = Instant::now();
    let mut ticker = tokio::time::interval(Duration::from_millis(90));
    let mut spinner_frame = 0usize;
    let mut header_printed = false;
    let mut rx_open = true;

    loop {
        tokio::select! {
            biased;

            maybe_event = rx.recv(), if rx_open => {
                match maybe_event {
                    Some(StreamEvent::ContentDelta(delta)) => {
                        if !header_printed {
                            terminal.clear()?;
                            println!("{}", "● Warden".truecolor(46, 204, 113).bold());
                            header_printed = true;
                        }
                        print!("{delta}");
                        io::stdout().flush()?;
                    }
                    Some(_) => {}
                    None => rx_open = false,
                }
            }

            _ = ticker.tick(), if !header_printed => {
                spinner_frame = spinner_frame.wrapping_add(1);
                terminal.draw(|frame| render_thinking_box(frame, start.elapsed(), spinner_frame))?;
            }

            event = term_events.next() => {
                if let Some(Ok(event)) = &event {
                    if is_ctrl_c(event) {
                        handle.abort();
                        if !header_printed {
                            terminal.clear()?;
                        } else {
                            println!();
                        }
                        println!("{}", "(interrompido)".dimmed());
                        println!();
                        return Ok(None);
                    }
                }
            }

            result = &mut handle => {
                if !header_printed {
                    terminal.clear()?;
                } else {
                    println!();
                }
                let outcome = result??;
                if let Some(usage) = &outcome.usage {
                    println!(
                        "{}",
                        format!("  ({} prompt + {} completion = {} tokens)", usage.prompt_tokens, usage.completion_tokens, usage.total_tokens).dimmed()
                    );
                }
                println!();
                return Ok(Some(outcome));
            }
        }
    }
}

fn print_banner() {
    println!("{}", "Warden".bold());
    println!("{}", "seu assistente pessoal — escreva algo abaixo (↑ para o histórico, Ctrl+D pra sair)".dimmed());
    println!();
}

pub async fn run(orchestrator: &Orchestrator, history_path: Option<&Path>) -> anyhow::Result<()> {
    let mut line_history = history_path.map(load_history).unwrap_or_default();
    let mut history: Vec<Message> = Vec::new();

    print_banner();

    let _raw_mode = RawModeGuard::enable()?;
    let mut term_events = EventStream::new();

    loop {
        let input = match read_line(&mut line_history, &mut term_events).await? {
            LineOutcome::Submitted(text) => text,
            LineOutcome::Exit => break,
        };

        let trimmed = input.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed == "exit" || trimmed == "quit" {
            break;
        }

        println!("{} {}", ">".truecolor(230, 126, 34).bold(), trimmed);

        match run_turn(orchestrator, &history, trimmed, &mut term_events).await {
            Ok(Some(outcome)) => {
                history.push(Message::user(trimmed));
                history.push(Message::assistant(outcome.content));
            }
            Ok(None) => {}
            Err(err) => {
                println!("{}", format!("erro: {err:#}").red());
                println!();
            }
        }
    }

    drop(_raw_mode);

    if let Some(path) = history_path {
        save_history(path, &line_history);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_backspace_move_the_cursor_correctly() {
        let mut editor = LineEditor::new(Vec::new());
        editor.insert('o');
        editor.insert('i');
        assert_eq!(editor.as_str(), "oi");
        assert_eq!(editor.cursor, 2);

        editor.backspace();
        assert_eq!(editor.as_str(), "o");
        assert_eq!(editor.cursor, 1);
    }

    #[test]
    fn cursor_movement_is_utf8_char_aware_not_byte_aware() {
        let mut editor = LineEditor::new(Vec::new());
        for c in "héllo".chars() {
            editor.insert(c);
        }
        assert_eq!(editor.cursor, 5);

        editor.move_left();
        editor.move_left();
        editor.move_left();
        assert_eq!(editor.cursor, 2);
        editor.insert('X');
        assert_eq!(editor.as_str(), "héXllo");
    }

    #[test]
    fn insert_in_the_middle_respects_the_cursor_position() {
        let mut editor = LineEditor::new(Vec::new());
        for c in "Wrden".chars() {
            editor.insert(c);
        }
        editor.move_home();
        editor.move_right();
        editor.insert('a');
        assert_eq!(editor.as_str(), "Warden");
    }

    #[test]
    fn delete_forward_removes_the_character_after_the_cursor() {
        let mut editor = LineEditor::new(Vec::new());
        for c in "Wardxen".chars() {
            editor.insert(c);
        }
        editor.move_left();
        editor.move_left();
        editor.move_left();
        editor.delete_forward();
        assert_eq!(editor.as_str(), "Warden");
    }

    #[test]
    fn home_and_end_move_to_the_boundaries() {
        let mut editor = LineEditor::new(Vec::new());
        for c in "hi".chars() {
            editor.insert(c);
        }
        editor.move_home();
        assert_eq!(editor.cursor, 0);
        editor.move_end();
        assert_eq!(editor.cursor, 2);
    }

    #[test]
    fn delete_word_backward_removes_the_last_word_and_trailing_space() {
        let mut editor = LineEditor::new(Vec::new());
        for c in "hello there  ".chars() {
            editor.insert(c);
        }
        editor.delete_word_backward();
        assert_eq!(editor.as_str(), "hello ");
    }

    #[test]
    fn clear_line_empties_the_buffer_and_resets_the_cursor() {
        let mut editor = LineEditor::new(Vec::new());
        for c in "oops".chars() {
            editor.insert(c);
        }
        editor.clear_line();
        assert!(editor.is_empty());
        assert_eq!(editor.cursor, 0);
    }

    #[test]
    fn history_up_recalls_the_most_recent_entry_first() {
        let mut editor = LineEditor::new(vec!["first".to_string(), "second".to_string()]);
        editor.history_up();
        assert_eq!(editor.as_str(), "second");
        editor.history_up();
        assert_eq!(editor.as_str(), "first");
        // Already at the oldest entry — another Up does nothing.
        editor.history_up();
        assert_eq!(editor.as_str(), "first");
    }

    #[test]
    fn history_down_past_the_newest_entry_restores_the_in_progress_draft() {
        let mut editor = LineEditor::new(vec!["first".to_string(), "second".to_string()]);
        for c in "unsent draft".chars() {
            editor.insert(c);
        }

        editor.history_up();
        assert_eq!(editor.as_str(), "second");
        editor.history_down();
        assert_eq!(editor.as_str(), "unsent draft");
    }

    #[test]
    fn submit_returns_the_text_and_resets_the_editor() {
        let mut editor = LineEditor::new(Vec::new());
        for c in "hi".chars() {
            editor.insert(c);
        }
        let text = editor.submit();
        assert_eq!(text, "hi");
        assert!(editor.is_empty());
        assert_eq!(editor.cursor, 0);
    }
}
