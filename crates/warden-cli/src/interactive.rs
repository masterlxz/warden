//! The rich terminal loop, used only when stdin is a real TTY (see `main.rs`'s `IsTerminal`
//! check) — a bordered input box and a "thinking" status box, both drawn with `ratatui`'s
//! *inline* viewport (not full-screen/alt-screen — the terminal's own scroll history, not a
//! second buffer ratatui owns, is what holds the growing conversation), plus real token-by-token
//! streaming of the assistant's response. `run` clears the screen (and scrollback) before
//! printing anything, so opening Warden reads as starting a clean session rather than a banner
//! stuck below whatever was on screen already (explicit user feedback: the first version of this
//! module didn't do that, and it read as a plain terminal script, not an app). The plain
//! non-interactive loop in `main.rs` is untouched and still handles piped stdin (scripted use,
//! the existing process-level tests).
//!
//! There's no line-editing library here (`rustyline` is gone) — a bordered, redrawn-every-frame
//! input box needs to own the terminal region itself, which a separate line-editing library
//! doesn't compose with. `LineEditor` below is a small hand-rolled one; its cursor/history logic
//! is unit-tested directly (no terminal involved). The line-editing history file this module
//! reads/writes is a plain newline-per-entry text file — a new, simpler format than rustyline's
//! own, since only the on-disk *format* changed, not what it's for (still just remembered input
//! lines, not the conversation itself, which was never persisted here).
//!
//! Key events are read via `crossterm::event::poll`/`read` — bounded, synchronous polls — never
//! `crossterm::event::EventStream`. `EventStream` spawns a background thread that, once polled
//! even once, blocks indefinitely inside crossterm's internal event reader until a byte actually
//! arrives on stdin; that reader is a single process-wide lock shared with
//! `cursor::position()` (what ratatui's inline viewport uses under the hood, on construction and
//! on every `Terminal::clear()`). Since a chat turn spends most of its time with nobody typing,
//! that lock stays held for the whole "thinking" wait, so the `clear()` call that swaps the
//! thinking box for the streamed response can't get the cursor position and fails after
//! crossterm's hardcoded 2s timeout — reproduced directly against a real pty before this comment
//! was written. Bounded polls never hold that lock past their own timeout, so `clear()` always
//! finds it free.
//!
//! Every line that should become permanent conversation history (the user's own message, the
//! assistant's reply, token counts, errors) is committed via `Terminal::insert_before` —
//! `ratatui`'s own API for "print history above a pinned inline box" — never via a bare
//! `print!`/`println!`. An earlier version of this module printed those lines directly to
//! stdout while raw mode was on, bypassing `ratatui`'s bookkeeping of where the inline viewport
//! (the input/status box) actually sits on screen; the box's position is only ever updated by
//! `Terminal::draw`/`clear`/`insert_before`, so a raw print left it stale, and the next
//! `draw()`/`clear()` rendered into the wrong row — the input box would end up staircased away
//! from the conversation instead of pinned below it (confirmed against a real pty, and against
//! the user's own report of a garbled, non-chat-like layout). `insert_before` also sidesteps the
//! `\n`-vs-`\r\n` raw-mode pitfall entirely, since it positions the cursor per cell on the
//! backend rather than relying on the terminal's own newline handling.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use owo_colors::OwoColorize;
use ratatui::backend::CrosstermBackend;
use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph};
use ratatui::{Terminal, TerminalOptions, Viewport};
use tokio::sync::mpsc;
use unicode_width::UnicodeWidthStr;
use warden_bootstrap::{
    build_model_provider, default_model_for, load_config_from_path, remove_provider_references, rename_provider_cascade, save_config, AgentConfig,
    FileConfig, Provider, ProviderConfig,
};
use warden_core::model::{Message, ModelProvider, StreamEvent};
use warden_core::orchestrator::{MessageOutcome, Orchestrator};

use crate::commands::{self, Command, ParseOutcome};

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

    /// Whether the cursor sits at the end of the buffer — the only position `drive_line_editor`
    /// shows/accepts a tab-completion ghost suggestion at, since a suggestion computed from the
    /// whole buffer wouldn't make sense to insert in the middle of already-typed text.
    fn cursor_at_end(&self) -> bool {
        self.cursor == self.buffer.len()
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

/// `ghost` is the dim, non-editable completion suffix shown right after the typed text (see
/// `drive_line_editor`'s doc comment) — `None` for a wizard field (`read_field`), which never
/// suggests slash-commands since its buffer holds an id/persona/key, not a command.
fn render_input_box(frame: &mut ratatui::Frame, editor: &LineEditor, title: &str, ghost: Option<&str>) {
    let area = frame.area();
    let block = Block::bordered().border_type(BorderType::Rounded).border_style(Style::default().fg(accent_color())).title(title.to_string());
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut spans = vec![Span::raw(editor.as_str())];
    if let Some(suffix) = ghost {
        spans.push(Span::styled(suffix.to_string(), Style::default().add_modifier(Modifier::DIM)));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), inner);

    let cursor_col = inner.x + UnicodeWidthStr::width(editor.prefix().as_str()) as u16;
    frame.set_cursor_position((cursor_col, inner.y));
}

/// What the live, not-yet-committed assistant preview (see `render_card_preview`) currently
/// shows: a spinner before any content has arrived, or the tail of the response streamed in so
/// far once it has (see `run_turn` and `MAX_PREVIEW_ROWS`).
enum PreviewState<'a> {
    Thinking { elapsed: Duration, spinner_frame: usize },
    Streaming { lines: &'a [String] },
}

/// Same rounded border as `render_input_box`, titled like the final committed card
/// (`insert_card`) it turns into once the reply completes — so the transition from "still
/// streaming" to "permanent history" is just the border staying in place while the box grows and
/// then gets committed, not a jump between two unrelated visual styles.
fn render_card_preview(frame: &mut ratatui::Frame, state: PreviewState) {
    let area = frame.area();
    let title = Line::from(vec![Span::styled("● ", Style::default().fg(Color::Rgb(46, 204, 113))), Span::raw("Warden")]);
    let block = Block::bordered().border_type(BorderType::Rounded).border_style(Style::default().fg(accent_color())).title(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    match state {
        PreviewState::Thinking { elapsed, spinner_frame } => {
            let glyph = SPINNER_FRAMES[spinner_frame % SPINNER_FRAMES.len()];
            let line = Line::from(vec![
                Span::styled(format!("{glyph} "), Style::default().fg(Color::Rgb(241, 196, 15))),
                Span::styled(format!("pensando... ({}s — Ctrl+C para interromper)", elapsed.as_secs()), Style::default().add_modifier(Modifier::DIM)),
            ]);
            frame.render_widget(Paragraph::new(line), inner);
        }
        PreviewState::Streaming { lines } => {
            let text: Vec<Line> = lines.iter().map(|l| Line::raw(l.clone())).collect();
            frame.render_widget(Paragraph::new(text), inner);
        }
    }
}

fn new_inline_terminal_with_height(height: u16) -> anyhow::Result<CliTerminal> {
    let backend = CrosstermBackend::new(io::stdout());
    Ok(Terminal::with_options(backend, TerminalOptions { viewport: Viewport::Inline(height) })?)
}

fn new_inline_terminal() -> anyhow::Result<CliTerminal> {
    new_inline_terminal_with_height(VIEWPORT_HEIGHT)
}

/// Body rows the live preview shows at once before it stops growing and just scrolls to show the
/// tail (see `ensure_preview_height`) — keeps a very long reply from making the preview take over
/// the whole screen while it's still streaming. The final card (`insert_card`) always gets every
/// row regardless; this only bounds the *live* view.
const MAX_PREVIEW_ROWS: usize = 6;

/// Grows (or shrinks back down) the live card-preview viewport to fit `wanted_rows` of body text
/// plus its border, rebuilding the `Terminal` only when the height actually needs to change.
/// Safe to do repeatedly now that `EventStream` (see the module doc comment) is gone: rebuilding
/// just queries the cursor position once via the backend, and nothing else can hold that query's
/// lock indefinitely anymore, which is what made a fresh `Terminal` per call hang before.
fn ensure_preview_height(terminal: &mut CliTerminal, current_height: &mut u16, wanted_rows: usize) -> anyhow::Result<()> {
    let wanted_height = (wanted_rows.min(MAX_PREVIEW_ROWS) as u16 + 2).max(VIEWPORT_HEIGHT);
    if wanted_height != *current_height {
        terminal.clear()?;
        *terminal = new_inline_terminal_with_height(wanted_height)?;
        *current_height = wanted_height;
    }
    Ok(())
}

fn key_is_ctrl_c(key: &crossterm::event::KeyEvent) -> bool {
    key.kind == KeyEventKind::Press && key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL)
}

enum LineOutcome {
    Submitted(String),
    Exit,
}

/// The terminal's current column count — used to wrap history text ourselves before committing
/// it (see `wrap_text`), since `Terminal::insert_before` needs an exact row count up front.
fn terminal_width() -> u16 {
    crossterm::terminal::size().map(|(cols, _)| cols).unwrap_or(80)
}

/// Wraps one line (no embedded newlines) to at most `width` display columns, breaking on spaces
/// where possible and only hard-breaking a single word that alone exceeds `width`. Always returns
/// at least one (possibly empty) row.
fn wrap_segment(segment: &str, width: usize) -> Vec<String> {
    let mut rows = Vec::new();
    let mut current = String::new();
    let mut current_width = 0usize;

    for word in segment.split_inclusive(' ') {
        let word_width = UnicodeWidthStr::width(word);
        if current_width > 0 && current_width + word_width > width {
            rows.push(std::mem::take(&mut current));
            current_width = 0;
        }
        if word_width > width {
            for c in word.chars() {
                let c_width = UnicodeWidthStr::width(c.to_string().as_str()).max(1);
                if current_width + c_width > width {
                    rows.push(std::mem::take(&mut current));
                    current_width = 0;
                }
                current.push(c);
                current_width += c_width;
            }
            continue;
        }
        current.push_str(word);
        current_width += word_width;
    }
    rows.push(current);
    rows
}

/// Wraps possibly-multi-line `text` (split on real `\n`s first, each segment wrapped on its own)
/// to `width` display columns — Warden's own tiny word-wrapper, used instead of `ratatui`'s
/// (which only reports a wrapped row count through an unstable, render-time API) so a row count
/// is known ahead of the `insert_before` call below.
fn wrap_text(text: &str, width: u16) -> Vec<String> {
    let width = width.max(1) as usize;
    text.split('\n').flat_map(|segment| wrap_segment(segment, width)).collect()
}

/// Permanently commits one block of text (which may be wider than the terminal, in which case it
/// soft-wraps, and may contain embedded newlines) to the scrollback, just above the pinned
/// input/status box, via `insert_before` — see the module doc comment for why this, and never a
/// raw `print!`/`println!`, is the only safe way to grow the conversation history.
fn insert_history_line(terminal: &mut CliTerminal, text: &str, style: Style) -> anyhow::Result<()> {
    let rows = wrap_text(text, terminal_width());
    let height = rows.len() as u16;
    terminal.insert_before(height, move |buf| {
        for (i, row) in rows.iter().enumerate() {
            buf.set_string(buf.area.x, buf.area.y + i as u16, row, style);
        }
    })?;
    Ok(())
}

/// Width (in display columns) available for a message card's content — the full terminal width
/// minus the left `"│ "` and right `" │"` border padding (see `insert_card`).
fn card_content_width() -> usize {
    (terminal_width() as usize).saturating_sub(4).max(1)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LineKind {
    Header,
    Bullet,
    Code,
    Plain,
}

/// Classifies one already-`\n`-free markdown-ish line, stripping its block syntax so only the
/// content remains. `in_code_block` toggles on a fenced-code delimiter line (`` ``` ``, in which
/// case there's nothing to render — `None`) and must be threaded across an entire turn by the
/// caller, not reset per call.
fn classify_and_strip(line: &str, in_code_block: &mut bool) -> Option<(LineKind, String)> {
    let trimmed = line.trim_start();
    if trimmed.starts_with("```") {
        *in_code_block = !*in_code_block;
        return None;
    }
    if *in_code_block {
        return Some((LineKind::Code, line.to_string()));
    }
    for prefix in ["### ", "## ", "# "] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            return Some((LineKind::Header, rest.to_string()));
        }
    }
    if let Some(rest) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")) {
        return Some((LineKind::Bullet, rest.to_string()));
    }
    Some((LineKind::Plain, line.to_string()))
}

/// The index of the first run of `len` consecutive `marker` characters at or after `start`, if
/// any — used by `parse_inline` to find a markdown delimiter's closing counterpart.
fn find_marker(chars: &[char], start: usize, marker: char, len: usize) -> Option<usize> {
    (start..=chars.len().saturating_sub(len)).find(|&i| chars[i..i + len].iter().all(|&c| c == marker))
}

/// Parses `**bold**`, `*italic*`/`_italic_` and `` `inline code` `` out of one line, applying
/// `base` to everything else. Not full CommonMark — just the constructs a chat reply actually
/// uses — and doesn't try to recover from a marker left open at the end of `text`: it's rendered
/// as its own literal characters instead. A marker split across two separately-committed history
/// rows (see `run_turn`) is a rare, self-limited cosmetic edge case for the same reason: once a
/// row is committed via `insert_before` it can't be redrawn once the rest of the markup arrives.
fn parse_inline(text: &str, base: Style) -> Vec<(String, Style)> {
    let chars: Vec<char> = text.chars().collect();
    let mut spans = Vec::new();
    let mut buf = String::new();
    let mut i = 0;

    macro_rules! flush {
        () => {
            if !buf.is_empty() {
                spans.push((std::mem::take(&mut buf), base));
            }
        };
    }

    while i < chars.len() {
        if chars[i] == '`' {
            if let Some(end) = find_marker(&chars, i + 1, '`', 1) {
                flush!();
                spans.push((chars[i + 1..end].iter().collect(), base.fg(accent_color())));
                i = end + 1;
                continue;
            }
        }
        if chars[i] == '*' && chars.get(i + 1) == Some(&'*') {
            if let Some(end) = find_marker(&chars, i + 2, '*', 2) {
                flush!();
                spans.push((chars[i + 2..end].iter().collect(), base.add_modifier(Modifier::BOLD)));
                i = end + 2;
                continue;
            }
        }
        if chars[i] == '*' || chars[i] == '_' {
            let marker = chars[i];
            if let Some(end) = find_marker(&chars, i + 1, marker, 1) {
                if end > i + 1 {
                    flush!();
                    spans.push((chars[i + 1..end].iter().collect(), base.add_modifier(Modifier::ITALIC)));
                    i = end + 1;
                    continue;
                }
            }
        }
        buf.push(chars[i]);
        i += 1;
    }
    flush!();
    spans
}

/// Same purple as `BRAND` (see its doc comment) — reused here for markdown headers/bullets so
/// they read as part of the same visual identity rather than a random extra color.
fn accent_color() -> Color {
    let (r, g, b) = BRAND;
    Color::Rgb(r, g, b)
}

/// Classifies+strips `line` (a complete, `\n`-terminated logical line — never a still-streaming
/// partial one, see `run_turn`) and turns it into styled spans: a header is bold and accented, a
/// bullet gets a `"• "` marker, a fenced code line is styled as code with no inline parsing
/// (so `**`/`` ` `` inside real code isn't mistaken for markdown), everything else gets inline
/// span parsing. Returns `None` for a fence delimiter line, which renders nothing.
fn markdown_spans_for_line(line: &str, in_code_block: &mut bool) -> Option<Vec<(String, Style)>> {
    let (kind, content) = classify_and_strip(line, in_code_block)?;
    Some(match kind {
        LineKind::Header => parse_inline(&content, Style::default().add_modifier(Modifier::BOLD).fg(accent_color())),
        LineKind::Bullet => {
            let mut spans = vec![("• ".to_string(), Style::default().fg(accent_color()))];
            spans.extend(parse_inline(&content, Style::default()));
            spans
        }
        LineKind::Code => vec![(content, Style::default().fg(accent_color()))],
        LineKind::Plain => parse_inline(&content, Style::default()),
    })
}

/// Word-wraps already-styled spans to `width` display columns — the same greedy algorithm as
/// `wrap_segment`, but keeping each token's style attached, so markdown emphasis survives
/// wrapping instead of collapsing back to plain text.
fn wrap_spans(spans: &[(String, Style)], width: usize) -> Vec<Vec<(String, Style)>> {
    let width = width.max(1);
    let mut rows = Vec::new();
    let mut current: Vec<(String, Style)> = Vec::new();
    let mut current_width = 0usize;

    for (text, style) in spans {
        for word in text.split_inclusive(' ') {
            let word_width = UnicodeWidthStr::width(word);
            if current_width > 0 && current_width + word_width > width {
                rows.push(std::mem::take(&mut current));
                current_width = 0;
            }
            if word_width > width {
                let mut piece = String::new();
                let mut piece_width = 0usize;
                for c in word.chars() {
                    let c_width = UnicodeWidthStr::width(c.to_string().as_str()).max(1);
                    if current_width + piece_width + c_width > width {
                        if !piece.is_empty() {
                            current.push((std::mem::take(&mut piece), *style));
                            piece_width = 0;
                        }
                        rows.push(std::mem::take(&mut current));
                        current_width = 0;
                    }
                    piece.push(c);
                    piece_width += c_width;
                }
                if !piece.is_empty() {
                    current.push((piece, *style));
                    current_width += piece_width;
                }
                continue;
            }
            current.push((word.to_string(), *style));
            current_width += word_width;
        }
    }
    rows.push(current);
    rows
}

/// Classifies+styles a complete assistant reply (all of it, not a still-streaming partial chunk
/// — see `run_turn`) into wrapped, card-ready body rows: every logical (`\n`-separated) line runs
/// through `markdown_spans_for_line` and gets wrapped to `card_content_width()`. Calling this only
/// once the full text is known — instead of line-by-line as it streams, which an earlier version
/// of this module did — means a markdown marker can never end up split across two separately
/// committed rows; see `PENDING.md` P34 for that now-obsolete trade-off. Never empty: an empty
/// reply still gets one empty row, so its card has a body instead of being just two borders.
fn markdown_body_rows(text: &str) -> Vec<Vec<(String, Style)>> {
    let mut in_code_block = false;
    let width = card_content_width();
    let rows: Vec<Vec<(String, Style)>> =
        text.split('\n').flat_map(|line| markdown_spans_for_line(line, &mut in_code_block).map(|spans| wrap_spans(&spans, width)).unwrap_or_default()).collect();
    if rows.is_empty() { vec![Vec::new()] } else { rows }
}

/// Wraps the user's own raw input to `card_content_width()` for their message card — deliberately
/// no markdown parsing: what they typed is literal text, not something to interpret as markup.
fn plain_body_rows(text: &str) -> Vec<Vec<(String, Style)>> {
    wrap_text(text, card_content_width() as u16).into_iter().map(|row| vec![(row, Style::default())]).collect()
}

/// Wraps each already-styled line to `card_content_width()` for a `/models`/`/agents` list card —
/// same idea as `plain_body_rows`, but for a caller that already picked a style per line (e.g. a
/// dimmed marker on the session-active entry) instead of plain text.
fn list_body_rows(lines: Vec<(String, Style)>) -> Vec<Vec<(String, Style)>> {
    let width = card_content_width();
    lines.into_iter().flat_map(|(text, style)| wrap_spans(&[(text, style)], width)).collect()
}

/// One row of a card's bordered content — `"│ "`, the row's styled spans, then a right-aligned
/// `"│"` at the card's own width. A blank `row` (no spans) still draws both border characters, so
/// an empty spacer line inside a card (see `insert_card`'s footer handling) still looks framed.
fn draw_card_row(buf: &mut Buffer, x0: u16, y: u16, width: u16, row: &[(String, Style)], border_style: Style) {
    buf.set_string(x0, y, "│ ", border_style);
    let mut x = x0 + 2;
    for (text, style) in row {
        buf.set_string(x, y, text, *style);
        x += UnicodeWidthStr::width(text.as_str()) as u16;
    }
    buf.set_string(x0 + width - 1, y, "│", border_style);
}

/// The card's own width: sized to fit its widest row (title or content), never wider than
/// `max_width` (the terminal) and never narrower than a sane minimum. Body/footer rows are
/// already wrapped to `card_content_width()` (a `max_width`-based cap, always `<= max_width - 4`),
/// so shrinking to the widest one here can only ever narrow the card, never force a re-wrap — a
/// short reply like "ok" gets a snug card instead of a bar stretched across the whole terminal.
fn card_width(title_width: usize, content_rows: &[Vec<(String, Style)>], max_width: usize) -> u16 {
    let widest_content: usize = content_rows.iter().map(|row| row.iter().map(|(t, _)| UnicodeWidthStr::width(t.as_str())).sum()).max().unwrap_or(0);
    (widest_content + 4).max(title_width + 6).clamp(12, max_width.max(12)) as u16
}

/// Renders one complete message "card" — a bordered box sized to fit its own longest row (never
/// wider than the terminal, never narrower than a sane minimum), with a titled top border,
/// already-wrapped/styled body rows, an optional single footer row (e.g. token count, preceded by
/// a blank spacer), and a bottom border — as one permanent block of history via `insert_before`.
/// A card is only ever committed once its full content is known: `insert_before` commits are
/// permanent, so a border can't be "reopened" to append more body rows into it later — the
/// still-streaming assistant reply is shown live in the pinned preview instead
/// (`render_card_preview`) and only turned into a card once it's complete (see `run_turn`).
fn insert_card(terminal: &mut CliTerminal, title: Vec<(String, Style)>, border_style: Style, body: Vec<Vec<(String, Style)>>, footer: Option<Vec<(String, Style)>>) -> anyhow::Result<()> {
    let title_width: usize = title.iter().map(|(t, _)| UnicodeWidthStr::width(t.as_str())).sum();

    let mut content_rows = body;
    if let Some(footer_row) = footer {
        content_rows.push(Vec::new());
        content_rows.push(footer_row);
    }

    let width = card_width(title_width, &content_rows, terminal_width() as usize);
    let top_dashes = (width as usize).saturating_sub(4 + title_width);
    let height = content_rows.len() as u16 + 2;

    terminal.insert_before(height, move |buf| {
        let x0 = buf.area.x;
        let y0 = buf.area.y;

        buf.set_string(x0, y0, "╭─ ", border_style);
        let mut x = x0 + 3;
        for (text, style) in &title {
            buf.set_string(x, y0, text, *style);
            x += UnicodeWidthStr::width(text.as_str()) as u16;
        }
        buf.set_string(x, y0, format!(" {}╮", "─".repeat(top_dashes)), border_style);

        for (i, row) in content_rows.iter().enumerate() {
            draw_card_row(buf, x0, y0 + 1 + i as u16, width, row, border_style);
        }

        buf.set_string(x0, y0 + height - 1, format!("╰{}╯", "─".repeat((width as usize).saturating_sub(2))), border_style);
    })?;
    Ok(())
}

/// How long each bounded `event::poll` waits before giving the caller a chance to redraw/check
/// other state. Small enough that typing and the spinner both feel instant; see the module-level
/// doc comment for why this must be a bounded poll, never `EventStream`.
const POLL_INTERVAL: Duration = Duration::from_millis(30);

/// Drives one `LineEditor` to completion: redraws the bordered input box (titled `title`) every
/// frame and applies key events to it, exactly like the main chat prompt — shared so a wizard
/// field (`read_field`) gets the same editing keys (arrows, Ctrl+A/E/U/W, history recall if the
/// editor was built with any) without duplicating this loop. By the time this returns, the box
/// has been cleared, so whatever's printed next starts from a clean, normal line of scrollback.
///
/// `suggest_commands` gates the tab-completion ghost suggestion (`commands::ghost_suggestion`) —
/// on for the main chat prompt (`read_line`), off for a wizard field (`read_field`), whose buffer
/// holds an id/persona/key, never a slash-command. `Tab` accepts the currently shown ghost (if
/// any), appending its suffix plus a trailing space — a no-op when there's nothing to accept
/// (ambiguous, already complete, or the cursor isn't at the end of the buffer).
async fn drive_line_editor(terminal: &mut CliTerminal, editor: &mut LineEditor, title: &str, suggest_commands: bool) -> anyhow::Result<LineOutcome> {
    loop {
        let ghost = if suggest_commands && editor.cursor_at_end() { commands::ghost_suggestion(&editor.as_str()) } else { None };
        terminal.draw(|frame| render_input_box(frame, editor, title, ghost.as_deref()))?;

        if !event::poll(POLL_INTERVAL)? {
            continue;
        }

        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => match (key.code, key.modifiers) {
                (KeyCode::Enter, _) => {
                    let text = editor.submit();
                    terminal.clear()?;
                    return Ok(LineOutcome::Submitted(text));
                }
                // Matches GNU readline: Ctrl+D only quits on an empty line, so it can't be
                // mistaken for "discard what I just typed" — same reason rustyline's `Eof` never
                // fired on a non-empty buffer before this module dropped it. In a wizard
                // (`read_field`), this same `Exit` outcome means "cancel this field/wizard"
                // rather than "quit the REPL" — the caller decides which.
                (KeyCode::Char('d'), m) if m.contains(KeyModifiers::CONTROL) && editor.is_empty() => {
                    terminal.clear()?;
                    return Ok(LineOutcome::Exit);
                }
                (KeyCode::Tab, _) => {
                    if let Some(suffix) = &ghost {
                        for c in suffix.chars() {
                            editor.insert(c);
                        }
                        editor.insert(' ');
                    }
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
            _ => {}
        }
    }
}

async fn read_line(line_history: &mut Vec<String>, terminal: &mut CliTerminal) -> anyhow::Result<LineOutcome> {
    let mut editor = LineEditor::new(line_history.clone());
    let outcome = drive_line_editor(terminal, &mut editor, " Warden ", true).await?;
    if let LineOutcome::Submitted(text) = &outcome {
        if !text.trim().is_empty() {
            line_history.push(text.clone());
        }
    }
    Ok(outcome)
}

/// Prompts for one wizard field (`/models add`, `/agents create`, ...): a bordered input box
/// titled `title`, pre-filled with `initial` so pressing Enter with no edits accepts the current/
/// default value verbatim. No history recall (a fresh `LineEditor` with an empty history list) —
/// a one-off field isn't the kind of thing worth recalling later. `Ctrl+D` on an empty field
/// returns `LineOutcome::Exit`, which the caller treats as "cancel this field/wizard", not "quit
/// the REPL" (see `drive_line_editor`'s doc comment).
async fn read_field(terminal: &mut CliTerminal, title: &str, initial: &str) -> anyhow::Result<LineOutcome> {
    let mut editor = LineEditor::new(Vec::new());
    for c in initial.chars() {
        editor.insert(c);
    }
    drive_line_editor(terminal, &mut editor, title, false).await
}

/// Runs one full turn: spawns the streaming call, grows a live bordered preview (`render_card_
/// preview`) as content arrives, then — once the reply is complete — turns it into a permanent
/// message card (`insert_card`) with full markdown styling and a token-count footer. Returns
/// `Ok(None)` if the user interrupted with Ctrl+C — nothing gets appended to conversation history
/// in that case, matching a turn that never happened. Draws into the caller's long-lived
/// `Terminal` (though it may swap it out for a taller/shorter one via `ensure_preview_height` —
/// see that function's doc comment for why rebuilding mid-turn is safe here).
async fn run_turn(
    orchestrator: &Orchestrator,
    history: &[Message],
    input: &str,
    terminal: &mut CliTerminal,
    model_override: Option<Arc<dyn ModelProvider>>,
    system_prompt: Option<&str>,
) -> anyhow::Result<Option<MessageOutcome>> {
    let (tx, mut rx) = mpsc::unbounded_channel::<StreamEvent>();
    let orchestrator = match model_override {
        Some(model) => orchestrator.with_model(model),
        None => orchestrator.clone(),
    };
    let history_owned = history.to_vec();
    let input_owned = input.to_string();
    let system_prompt_owned = system_prompt.map(str::to_string);
    let handle = tokio::spawn(async move {
        orchestrator
            .handle_turn_streaming(&history_owned, &input_owned, Vec::new(), system_prompt_owned.as_deref(), move |event| {
                let _ = tx.send(event.clone());
            })
            .await
    });

    let start = Instant::now();
    let mut spinner_frame = 0usize;
    // Raw text received so far — re-parsed for markdown as a whole once the reply completes
    // (`markdown_body_rows`), not line-by-line as it streams in. Shown live, plainly wrapped and
    // capped to `MAX_PREVIEW_ROWS`, via `render_card_preview` in the meantime.
    let mut pending = String::new();
    let mut current_height = VIEWPORT_HEIGHT;

    let warden_title = vec![("● ".to_string(), Style::default().fg(Color::Rgb(46, 204, 113))), ("Warden".to_string(), Style::default())];
    let card_border = Style::default().fg(accent_color());
    let dim_style = Style::default().add_modifier(Modifier::DIM);

    // Not a `tokio::select!` over `rx`/a ticker/key events/`handle` — deliberately: this whole
    // turn just needs to notice new state at human-perceptible speed (a spinner frame, a Ctrl+C
    // keypress), and `event::poll`'s bounded wait below already paces the loop at that speed, so
    // it doubles as the tick. See the module doc comment for why key events specifically can't be
    // read via an async `EventStream` here.
    loop {
        while let Ok(event) = rx.try_recv() {
            if let StreamEvent::ContentDelta(delta) = event {
                pending.push_str(&delta);
            }
        }

        if handle.is_finished() {
            let result = handle.await;
            // Reset the preview's height back to the baseline *before* the fallible unwrap below
            // — `?` short-circuits immediately on an error, and the caller (`run`, which prints
            // the error) always expects the terminal to be back at `VIEWPORT_HEIGHT` for the next
            // `read_line`, whether this turn ended in success or failure.
            ensure_preview_height(terminal, &mut current_height, 0)?;
            let outcome = result??;
            let footer = outcome.usage.as_ref().map(|usage| {
                vec![(format!("{} prompt + {} completion = {} tokens", usage.prompt_tokens, usage.completion_tokens, usage.total_tokens), dim_style)]
            });
            insert_card(terminal, warden_title, card_border, markdown_body_rows(&outcome.content), footer)?;
            terminal.insert_before(1, |_buf| {})?;
            return Ok(Some(outcome));
        }

        let preview_lines = wrap_text(&pending, card_content_width() as u16);
        ensure_preview_height(terminal, &mut current_height, if pending.is_empty() { 0 } else { preview_lines.len() })?;
        if pending.is_empty() {
            spinner_frame = spinner_frame.wrapping_add(1);
            terminal.draw(|frame| render_card_preview(frame, PreviewState::Thinking { elapsed: start.elapsed(), spinner_frame }))?;
        } else {
            let tail_start = preview_lines.len().saturating_sub(MAX_PREVIEW_ROWS);
            terminal.draw(|frame| render_card_preview(frame, PreviewState::Streaming { lines: &preview_lines[tail_start..] }))?;
        }

        if event::poll(POLL_INTERVAL)? {
            if let Event::Key(key) = event::read()? {
                if key_is_ctrl_c(&key) {
                    handle.abort();
                    ensure_preview_height(terminal, &mut current_height, 0)?;
                    insert_history_line(terminal, "(interrompido)", dim_style)?;
                    insert_history_line(terminal, "", Style::default())?;
                    return Ok(None);
                }
            }
        }
    }
}

/// Same purple as the desktop app's `--color-accent` in dark mode (`desktop/src/App.css`) — the
/// one brand color this project actually has, reused here instead of the unrelated orange/yellow
/// this module picked ad hoc before. Terminals default to a dark background far more often than
/// light, so the dark-mode shade (lighter, more legible on black) is the one used unconditionally
/// — there's no terminal-side equivalent of the app's light/dark media query to key off of. Kept
/// as plain `(r, g, b)` rather than either color type below, since it's shared between two
/// different color APIs: `owo_colors::OwoColorize::truecolor` for the plain `println!`ed banner,
/// and `ratatui::style::Color::Rgb` for the bordered boxes drawn through `ratatui`.
const BRAND: (u8, u8, u8) = (167, 139, 250);

/// Clears the visible screen *and* the terminal's scrollback (`ESC[3J` — supported by every
/// terminal this project targets: xterm, Alacritty, Kitty, Wezterm, GNOME Terminal, Windows
/// Terminal), then homes the cursor. Called once, before anything else is printed, so opening
/// Warden feels like starting a session — not a wall of old shell history with a banner tacked on
/// below it. Confirmed against a real pty that this doesn't disturb the input box's cursor-query
/// dance from the module doc comment: it runs before raw mode / the `Terminal` even exist.
fn clear_screen() -> io::Result<()> {
    print!("\x1b[2J\x1b[3J\x1b[H");
    io::stdout().flush()
}

fn print_banner() {
    let (r, g, b) = BRAND;
    println!("{}", "Warden".truecolor(r, g, b).bold());
    println!("{}", "seu assistente pessoal — escreva algo abaixo (↑ para o histórico, Ctrl+D pra sair)".dimmed());
    // A thin rule, sized to the real terminal width (not a hardcoded guess) — separates the
    // one-time header from the conversation that scrolls below it, so the top of the screen reads
    // as a session's title bar rather than two unrelated lines of text.
    let width = crossterm::terminal::size().map(|(cols, _)| cols).unwrap_or(60) as usize;
    println!("{}", "─".repeat(width).truecolor(r, g, b).dimmed());
    println!();
}

/// Per-session state for the slash-commands (`/models`, `/agents`) — lives only for the lifetime
/// of one `run()` call, on top of (not persisted like) `config.toml` itself. Holds only chosen
/// *ids*, never a resolved `Arc<dyn ModelProvider>` or persona string: `resolve_turn_context`
/// re-reads the config fresh from disk before every turn that needs it, the same "never cache a
/// resolved object across calls" pattern the desktop's `send_message` IPC command already uses to
/// avoid staleness after a provider/agent is renamed or removed mid-session.
struct CliSession {
    config_path: Option<PathBuf>,
    provider_id: Option<String>,
    agent_id: Option<String>,
}

/// Reads `config.toml` fresh — never cached across turns/commands (see `CliSession`'s doc
/// comment). A missing file (no path configured, or a path that doesn't exist yet — e.g. before
/// the very first `/models add`) is treated as an empty config, not an error; a malformed file
/// still surfaces as one.
fn load_fresh_config(config_path: Option<&Path>) -> anyhow::Result<FileConfig> {
    match config_path {
        Some(path) => load_config_from_path(path, false),
        None => Ok(FileConfig::default()),
    }
}

/// Renders one card as a `/models`/`/agents` command's output (a list, a confirmation, or an
/// error) — same card machinery (`insert_card`) and trailing spacer as the user/assistant/error
/// cards in `run()`'s own loop, wrapped through `list_body_rows` so a long line never overflows
/// the card's border.
fn render_message_card(terminal: &mut CliTerminal, title: &str, style: Style, lines: Vec<(String, Style)>) -> anyhow::Result<()> {
    insert_card(terminal, vec![(title.to_string(), style)], style, list_body_rows(lines), None)?;
    terminal.insert_before(1, |_buf| {})?;
    Ok(())
}

fn accent_style() -> Style {
    Style::default().fg(accent_color())
}

fn error_style() -> Style {
    Style::default().fg(Color::Red)
}

fn dim_style() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}

/// Prompts one wizard field via `read_field`, translating `LineOutcome` into `Option<String>` —
/// `None` means the user cancelled (`Ctrl+D` on an empty field), which every wizard step treats as
/// "abandon the whole wizard", not just this field.
async fn prompt_field(terminal: &mut CliTerminal, title: &str, initial: &str) -> anyhow::Result<Option<String>> {
    match read_field(terminal, title, initial).await? {
        LineOutcome::Submitted(text) => Ok(Some(text)),
        LineOutcome::Exit => Ok(None),
    }
}

type TurnContext = (Option<Arc<dyn ModelProvider>>, Option<String>);

/// Resolves what this turn should actually use, given the session's `provider_id`/`agent_id`
/// selections — a model override (only when it differs from whatever `run()`'s own `orchestrator`
/// parameter already is) and a persona to pass as `system_prompt`. Reloads config fresh only when
/// at least one selection is active; when neither is, returns `(None, None)` immediately with no
/// disk access at all, so a session that never touches `/models`/`/agents` behaves exactly as
/// before this feature existed. An explicit `/models use` always wins over an active agent's own
/// `provider_id` (which only pre-fills when nothing more specific was chosen) — same precedence
/// the desktop's chat header uses between its agent and provider selectors.
fn resolve_turn_context(session: &CliSession) -> anyhow::Result<TurnContext> {
    if session.provider_id.is_none() && session.agent_id.is_none() {
        return Ok((None, None));
    }

    let config = load_fresh_config(session.config_path.as_deref())?;

    let system_prompt = session.agent_id.as_ref().and_then(|id| config.agents.iter().find(|a| &a.id == id)).map(|a| a.persona.clone());

    let effective_provider_id = session.provider_id.clone().or_else(|| {
        session.agent_id.as_ref().and_then(|id| config.agents.iter().find(|a| &a.id == id)).and_then(|a| a.provider_id.clone())
    });

    let model_override = match effective_provider_id {
        Some(provider_id) => {
            let provider_config = config
                .providers
                .iter()
                .find(|p| p.id == provider_id)
                .ok_or_else(|| anyhow::anyhow!("provider '{provider_id}' não existe mais na configuração"))?;
            Some(build_model_provider(provider_config, None)?)
        }
        None => None,
    };

    Ok((model_override, system_prompt))
}

async fn cmd_help(terminal: &mut CliTerminal) -> anyhow::Result<()> {
    let style = Style::default();
    let lines = [
        "/exit, /quit — sair",
        "/help — esta lista",
        "/models — listar os modelos configurados",
        "/models use <id> — usar um modelo pro resto da sessão",
        "/models reset — voltar pro modelo com que o Warden foi iniciado",
        "/models add — cadastrar um novo modelo",
        "/models edit <id> — editar um modelo",
        "/models remove <id> — remover um modelo",
        "/agents — listar os agentes configurados",
        "/agents use <id> | none — usar um agente (ou nenhum) pro resto da sessão",
        "/agents create — criar um agente novo",
        "/agents edit <id> — editar um agente",
        "/agents remove <id> — remover um agente",
    ]
    .into_iter()
    .map(|line| (line.to_string(), style))
    .collect();
    render_message_card(terminal, "ajuda", accent_style(), lines)
}

fn provider_display_model(provider: &ProviderConfig) -> String {
    provider.model.clone().or_else(|| default_model_for(provider.kind).map(str::to_string)).unwrap_or_else(|| "?".to_string())
}

async fn cmd_models_list(terminal: &mut CliTerminal, session: &CliSession) -> anyhow::Result<()> {
    let config = load_fresh_config(session.config_path.as_deref())?;
    if config.providers.is_empty() {
        return render_message_card(terminal, "modelos", accent_style(), vec![("nenhum provider configurado ainda — use /models add".to_string(), Style::default())]);
    }
    let active = session.provider_id.clone().or_else(|| config.active_provider.clone());
    let lines = config
        .providers
        .iter()
        .map(|p| {
            let marker = if active.as_deref() == Some(p.id.as_str()) { " [ativo]" } else { "" };
            (format!("{} ({}) — {}{}", p.id, commands::kind_label(p.kind), provider_display_model(p), marker), Style::default())
        })
        .collect();
    render_message_card(terminal, "modelos", accent_style(), lines)
}

async fn cmd_models_use(terminal: &mut CliTerminal, session: &mut CliSession, id: String) -> anyhow::Result<()> {
    let config = load_fresh_config(session.config_path.as_deref())?;
    if !config.providers.iter().any(|p| p.id == id) {
        return render_message_card(terminal, "erro", error_style(), vec![(format!("provider '{id}' não encontrado — use /models pra ver a lista"), Style::default())]);
    }
    session.provider_id = Some(id.clone());
    render_message_card(terminal, "modelos", accent_style(), vec![(format!("modelo ativo agora: {id}"), Style::default())])
}

async fn cmd_models_reset(terminal: &mut CliTerminal, session: &mut CliSession) -> anyhow::Result<()> {
    session.provider_id = None;
    render_message_card(terminal, "modelos", accent_style(), vec![("modelo voltou ao padrão com que o Warden foi iniciado".to_string(), Style::default())])
}

/// Loops a single wizard field until it parses as a valid `Provider` kind, re-prompting with an
/// error card on an invalid entry. Returns `Ok(None)` if the user cancels.
async fn prompt_provider_kind(terminal: &mut CliTerminal, initial: Provider) -> anyhow::Result<Option<Provider>> {
    loop {
        let Some(input) = prompt_field(terminal, " kind: gemini | openai | anthropic | openai_compatible ", commands::kind_label(initial)).await? else {
            return Ok(None);
        };
        match commands::parse_provider_kind(&input) {
            Some(kind) => return Ok(Some(kind)),
            None => render_message_card(terminal, "erro", error_style(), vec![("kind inválido — use gemini, openai, anthropic ou openai_compatible".to_string(), Style::default())])?,
        }
    }
}

/// Loops a single wizard field until it's a non-blank id that doesn't collide with an existing
/// provider (other than `keep_if_same`, so editing a provider without renaming it doesn't trip
/// the uniqueness check against itself). Returns `Ok(None)` if the user cancels.
async fn prompt_provider_id(terminal: &mut CliTerminal, providers: &[ProviderConfig], initial: &str, keep_if_same: Option<&str>) -> anyhow::Result<Option<String>> {
    loop {
        let Some(input) = prompt_field(terminal, " id (nome único do provider) ", initial).await? else {
            return Ok(None);
        };
        let candidate = input.trim().to_string();
        if candidate.is_empty() {
            render_message_card(terminal, "erro", error_style(), vec![("id não pode ficar em branco".to_string(), Style::default())])?;
        } else if Some(candidate.as_str()) != keep_if_same && providers.iter().any(|p| p.id == candidate) {
            render_message_card(terminal, "erro", error_style(), vec![(format!("já existe um provider com id '{candidate}'"), Style::default())])?;
        } else {
            return Ok(Some(candidate));
        }
    }
}

async fn wizard_models_add(terminal: &mut CliTerminal, session: &mut CliSession) -> anyhow::Result<()> {
    let mut config = load_fresh_config(session.config_path.as_deref())?;

    let Some(kind) = prompt_provider_kind(terminal, Provider::Gemini).await? else {
        return render_message_card(terminal, "modelos", dim_style(), vec![("cadastro cancelado".to_string(), dim_style())]);
    };
    let Some(id) = prompt_provider_id(terminal, &config.providers, "", None).await? else {
        return render_message_card(terminal, "modelos", dim_style(), vec![("cadastro cancelado".to_string(), dim_style())]);
    };
    let Some(api_key) = prompt_field(terminal, " api key (em branco = nenhuma) ", "").await? else {
        return render_message_card(terminal, "modelos", dim_style(), vec![("cadastro cancelado".to_string(), dim_style())]);
    };
    let base_url = if kind == Provider::OpenaiCompatible {
        let Some(input) = prompt_field(terminal, " base url (obrigatório pra openai_compatible) ", "").await? else {
            return render_message_card(terminal, "modelos", dim_style(), vec![("cadastro cancelado".to_string(), dim_style())]);
        };
        non_empty(input)
    } else {
        None
    };
    let Some(model_input) = prompt_field(terminal, " model ", default_model_for(kind).unwrap_or("")).await? else {
        return render_message_card(terminal, "modelos", dim_style(), vec![("cadastro cancelado".to_string(), dim_style())]);
    };

    let had_active_provider = config.active_provider.is_some();
    config.providers.push(ProviderConfig { id: id.clone(), kind, api_key: non_empty(api_key), base_url, model: non_empty(model_input) });
    if !had_active_provider {
        config.active_provider = Some(id.clone());
    }

    save_config_or_report(session, &config).await?;
    render_message_card(terminal, "modelos", accent_style(), vec![(format!("provider '{id}' cadastrado"), Style::default())])
}

async fn wizard_models_edit(terminal: &mut CliTerminal, session: &mut CliSession, target_id: String) -> anyhow::Result<()> {
    let mut config = load_fresh_config(session.config_path.as_deref())?;
    let Some(index) = config.providers.iter().position(|p| p.id == target_id) else {
        return render_message_card(terminal, "erro", error_style(), vec![(format!("provider '{target_id}' não encontrado"), Style::default())]);
    };
    let current = config.providers[index].clone();

    let Some(kind) = prompt_provider_kind(terminal, current.kind).await? else {
        return render_message_card(terminal, "modelos", dim_style(), vec![("edição cancelada".to_string(), dim_style())]);
    };
    let Some(new_id) = prompt_provider_id(terminal, &config.providers, &current.id, Some(&current.id)).await? else {
        return render_message_card(terminal, "modelos", dim_style(), vec![("edição cancelada".to_string(), dim_style())]);
    };
    let Some(api_key) = prompt_field(terminal, " api key (em branco = nenhuma) ", current.api_key.as_deref().unwrap_or("")).await? else {
        return render_message_card(terminal, "modelos", dim_style(), vec![("edição cancelada".to_string(), dim_style())]);
    };
    let base_url = if kind == Provider::OpenaiCompatible {
        let Some(input) = prompt_field(terminal, " base url ", current.base_url.as_deref().unwrap_or("")).await? else {
            return render_message_card(terminal, "modelos", dim_style(), vec![("edição cancelada".to_string(), dim_style())]);
        };
        non_empty(input)
    } else {
        None
    };
    let model_default = current.model.clone().unwrap_or_else(|| default_model_for(kind).unwrap_or("").to_string());
    let Some(model_input) = prompt_field(terminal, " model ", &model_default).await? else {
        return render_message_card(terminal, "modelos", dim_style(), vec![("edição cancelada".to_string(), dim_style())]);
    };

    let old_id = current.id.clone();
    config.providers[index] = ProviderConfig { id: new_id.clone(), kind, api_key: non_empty(api_key), base_url, model: non_empty(model_input) };
    if new_id != old_id {
        rename_provider_cascade(&mut config, &old_id, &new_id);
        if session.provider_id.as_deref() == Some(old_id.as_str()) {
            session.provider_id = Some(new_id.clone());
        }
    }

    save_config_or_report(session, &config).await?;
    render_message_card(terminal, "modelos", accent_style(), vec![(format!("provider '{new_id}' atualizado"), Style::default())])
}

async fn cmd_models_remove(terminal: &mut CliTerminal, session: &mut CliSession, id: String) -> anyhow::Result<()> {
    let mut config = load_fresh_config(session.config_path.as_deref())?;
    let before = config.providers.len();
    config.providers.retain(|p| p.id != id);
    if config.providers.len() == before {
        return render_message_card(terminal, "erro", error_style(), vec![(format!("provider '{id}' não encontrado"), Style::default())]);
    }
    remove_provider_references(&mut config, &id);
    if session.provider_id.as_deref() == Some(id.as_str()) {
        session.provider_id = None;
    }

    save_config_or_report(session, &config).await?;
    render_message_card(terminal, "modelos", accent_style(), vec![(format!("provider '{id}' removido"), Style::default())])
}

async fn cmd_agents_list(terminal: &mut CliTerminal, session: &CliSession) -> anyhow::Result<()> {
    let config = load_fresh_config(session.config_path.as_deref())?;
    if config.agents.is_empty() {
        return render_message_card(terminal, "agentes", accent_style(), vec![("nenhum agente configurado ainda — use /agents create".to_string(), Style::default())]);
    }
    let lines = config
        .agents
        .iter()
        .map(|a| {
            let preview: String = a.persona.chars().take(48).collect();
            let preview = if a.persona.chars().count() > 48 { format!("{preview}…") } else { preview };
            let provider = a.provider_id.clone().unwrap_or_else(|| "-".to_string());
            let marker = if session.agent_id.as_deref() == Some(a.id.as_str()) { " [ativo]" } else { "" };
            (format!("{} ({}) — {}{}", a.id, provider, preview, marker), Style::default())
        })
        .collect();
    render_message_card(terminal, "agentes", accent_style(), lines)
}

async fn cmd_agents_use(terminal: &mut CliTerminal, session: &mut CliSession, id: Option<String>) -> anyhow::Result<()> {
    let Some(id) = id else {
        session.agent_id = None;
        return render_message_card(terminal, "agentes", accent_style(), vec![("nenhum agente ativo agora".to_string(), Style::default())]);
    };
    let config = load_fresh_config(session.config_path.as_deref())?;
    if !config.agents.iter().any(|a| a.id == id) {
        return render_message_card(terminal, "erro", error_style(), vec![(format!("agente '{id}' não encontrado — use /agents pra ver a lista"), Style::default())]);
    }
    session.agent_id = Some(id.clone());
    render_message_card(terminal, "agentes", accent_style(), vec![(format!("agente ativo agora: {id}"), Style::default())])
}

/// Loops a single wizard field until it's a non-blank id that doesn't collide with an existing
/// agent (other than `keep_if_same`). Returns `Ok(None)` if the user cancels.
async fn prompt_agent_id(terminal: &mut CliTerminal, agents: &[AgentConfig], initial: &str, keep_if_same: Option<&str>) -> anyhow::Result<Option<String>> {
    loop {
        let Some(input) = prompt_field(terminal, " id (nome único do agente) ", initial).await? else {
            return Ok(None);
        };
        let candidate = input.trim().to_string();
        if candidate.is_empty() {
            render_message_card(terminal, "erro", error_style(), vec![("id não pode ficar em branco".to_string(), Style::default())])?;
        } else if Some(candidate.as_str()) != keep_if_same && agents.iter().any(|a| a.id == candidate) {
            render_message_card(terminal, "erro", error_style(), vec![(format!("já existe um agente com id '{candidate}'"), Style::default())])?;
        } else {
            return Ok(Some(candidate));
        }
    }
}

/// Loops a single wizard field until it's blank (= no default provider) or a real provider id.
/// Returns `Ok(None)` if the user cancels the wizard (distinct from `Ok(Some(None))`, "no
/// default provider chosen").
async fn prompt_agent_provider_id(terminal: &mut CliTerminal, providers: &[ProviderConfig], initial: &str) -> anyhow::Result<Option<Option<String>>> {
    loop {
        let Some(input) = prompt_field(terminal, " provider padrão (em branco = nenhum) ", initial).await? else {
            return Ok(None);
        };
        let candidate = input.trim().to_string();
        if candidate.is_empty() {
            return Ok(Some(None));
        } else if providers.iter().any(|p| p.id == candidate) {
            return Ok(Some(Some(candidate)));
        } else {
            render_message_card(terminal, "erro", error_style(), vec![(format!("provider '{candidate}' não existe — deixe em branco ou use /models pra ver a lista"), Style::default())])?;
        }
    }
}

async fn wizard_agents_create(terminal: &mut CliTerminal, session: &mut CliSession) -> anyhow::Result<()> {
    let mut config = load_fresh_config(session.config_path.as_deref())?;

    let Some(id) = prompt_agent_id(terminal, &config.agents, "", None).await? else {
        return render_message_card(terminal, "agentes", dim_style(), vec![("criação cancelada".to_string(), dim_style())]);
    };
    let Some(persona) = prompt_field(terminal, " persona (uma linha — como o agente deve se comportar) ", "").await? else {
        return render_message_card(terminal, "agentes", dim_style(), vec![("criação cancelada".to_string(), dim_style())]);
    };
    let Some(provider_id) = prompt_agent_provider_id(terminal, &config.providers, "").await? else {
        return render_message_card(terminal, "agentes", dim_style(), vec![("criação cancelada".to_string(), dim_style())]);
    };

    config.agents.push(AgentConfig { id: id.clone(), persona, provider_id });

    save_config_or_report(session, &config).await?;
    render_message_card(terminal, "agentes", accent_style(), vec![(format!("agente '{id}' criado"), Style::default())])
}

async fn wizard_agents_edit(terminal: &mut CliTerminal, session: &mut CliSession, target_id: String) -> anyhow::Result<()> {
    let mut config = load_fresh_config(session.config_path.as_deref())?;
    let Some(index) = config.agents.iter().position(|a| a.id == target_id) else {
        return render_message_card(terminal, "erro", error_style(), vec![(format!("agente '{target_id}' não encontrado"), Style::default())]);
    };
    let current = config.agents[index].clone();

    let Some(new_id) = prompt_agent_id(terminal, &config.agents, &current.id, Some(&current.id)).await? else {
        return render_message_card(terminal, "agentes", dim_style(), vec![("edição cancelada".to_string(), dim_style())]);
    };
    let Some(persona) = prompt_field(terminal, " persona ", &current.persona).await? else {
        return render_message_card(terminal, "agentes", dim_style(), vec![("edição cancelada".to_string(), dim_style())]);
    };
    let Some(provider_id) = prompt_agent_provider_id(terminal, &config.providers, current.provider_id.as_deref().unwrap_or("")).await? else {
        return render_message_card(terminal, "agentes", dim_style(), vec![("edição cancelada".to_string(), dim_style())]);
    };

    let old_id = current.id.clone();
    config.agents[index] = AgentConfig { id: new_id.clone(), persona, provider_id };
    if new_id != old_id && session.agent_id.as_deref() == Some(old_id.as_str()) {
        session.agent_id = Some(new_id.clone());
    }

    save_config_or_report(session, &config).await?;
    render_message_card(terminal, "agentes", accent_style(), vec![(format!("agente '{new_id}' atualizado"), Style::default())])
}

async fn cmd_agents_remove(terminal: &mut CliTerminal, session: &mut CliSession, id: String) -> anyhow::Result<()> {
    let mut config = load_fresh_config(session.config_path.as_deref())?;
    let before = config.agents.len();
    config.agents.retain(|a| a.id != id);
    if config.agents.len() == before {
        return render_message_card(terminal, "erro", error_style(), vec![(format!("agente '{id}' não encontrado"), Style::default())]);
    }
    if session.agent_id.as_deref() == Some(id.as_str()) {
        session.agent_id = None;
    }

    save_config_or_report(session, &config).await?;
    render_message_card(terminal, "agentes", accent_style(), vec![(format!("agente '{id}' removido"), Style::default())])
}

fn non_empty(text: String) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Saves `config` to `session.config_path`, surfacing "no config path available" (only possible
/// if `dirs::config_dir()` itself returned `None` — no `$HOME`/`$XDG_CONFIG_HOME`) as the same
/// kind of error card every other failure in a wizard gets, via `?` bubbling out of the caller.
async fn save_config_or_report(session: &CliSession, config: &FileConfig) -> anyhow::Result<()> {
    let path = session
        .config_path
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("não foi possível localizar um arquivo de configuração pra salvar (sem diretório de config no sistema)"))?;
    save_config(path, config)?;
    Ok(())
}

async fn handle_command(command: Command, terminal: &mut CliTerminal, session: &mut CliSession) -> anyhow::Result<()> {
    match command {
        Command::Exit => unreachable!("Command::Exit is handled by the caller before dispatch"),
        Command::Help => cmd_help(terminal).await,
        Command::ModelsList => cmd_models_list(terminal, session).await,
        Command::ModelsUse(id) => cmd_models_use(terminal, session, id).await,
        Command::ModelsReset => cmd_models_reset(terminal, session).await,
        Command::ModelsAdd => wizard_models_add(terminal, session).await,
        Command::ModelsEdit(id) => wizard_models_edit(terminal, session, id).await,
        Command::ModelsRemove(id) => cmd_models_remove(terminal, session, id).await,
        Command::AgentsList => cmd_agents_list(terminal, session).await,
        Command::AgentsUse(id) => cmd_agents_use(terminal, session, id).await,
        Command::AgentsCreate => wizard_agents_create(terminal, session).await,
        Command::AgentsEdit(id) => wizard_agents_edit(terminal, session, id).await,
        Command::AgentsRemove(id) => cmd_agents_remove(terminal, session, id).await,
    }
}

pub async fn run(orchestrator: &Orchestrator, history_path: Option<&Path>, config_path: Option<PathBuf>) -> anyhow::Result<()> {
    let mut line_history = history_path.map(load_history).unwrap_or_default();
    let mut history: Vec<Message> = Vec::new();

    clear_screen()?;
    print_banner();

    let _raw_mode = RawModeGuard::enable()?;
    // One `Terminal`, built once here and reused for both the input box and the "thinking" box
    // for the whole session, instead of rebuilt per turn (see the module doc comment for why a
    // fresh one per call used to hang/error: its construction queries the terminal's cursor
    // position, which contends with reading key events — reused here means that query only ever
    // happens once, before the loop below starts reading any key at all).
    let mut terminal = new_inline_terminal()?;

    let mut session = CliSession { config_path, provider_id: None, agent_id: None };

    loop {
        let input = match read_line(&mut line_history, &mut terminal).await? {
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

        match commands::parse_command(trimmed) {
            ParseOutcome::NotACommand => {}
            ParseOutcome::Recognized(Command::Exit) => break,
            ParseOutcome::Recognized(command) => {
                // A wizard/list command's own validation failures (unknown id, blank field, ...)
                // already return `Ok(())` after rendering their own error card — this only
                // catches a lower-level failure (config file unreadable, no config path at all)
                // that bubbled up via `?`, rendering it the same way `run_turn`'s own `Err` arm
                // does below, instead of ending the whole session over a recoverable mistake.
                if let Err(err) = handle_command(command, &mut terminal, &mut session).await {
                    render_message_card(&mut terminal, "erro", error_style(), vec![(format!("{err:#}"), Style::default())])?;
                }
                continue;
            }
            ParseOutcome::Unrecognized(raw) => {
                render_message_card(&mut terminal, "erro", error_style(), vec![(format!("comando desconhecido: {raw} — digite /help pra ver os comandos disponíveis"), Style::default())])?;
                continue;
            }
        }

        // Same card structure as the assistant's own reply (`insert_card`) — muted border/title
        // rather than a bright color, matching how understated an echoed prompt reads in a normal
        // chat UI (the color budget is reserved for the assistant's own markdown, not for
        // restating what the user just typed).
        let dim_style = Style::default().add_modifier(Modifier::DIM);
        insert_card(&mut terminal, vec![("você".to_string(), dim_style)], dim_style, plain_body_rows(trimmed), None)?;
        terminal.insert_before(1, |_buf| {})?;

        let (model_override, system_prompt) = match resolve_turn_context(&session) {
            Ok(resolved) => resolved,
            Err(err) => {
                render_message_card(&mut terminal, "erro", error_style(), vec![(format!("{err:#}"), Style::default())])?;
                continue;
            }
        };

        match run_turn(orchestrator, &history, trimmed, &mut terminal, model_override, system_prompt.as_deref()).await {
            Ok(Some(outcome)) => {
                history.push(Message::user(trimmed));
                history.push(Message::assistant(outcome.content));
            }
            Ok(None) => {}
            Err(err) => {
                let error_border = Style::default().fg(Color::Red);
                insert_card(&mut terminal, vec![("erro".to_string(), error_border)], error_border, plain_body_rows(&format!("{err:#}")), None)?;
                terminal.insert_before(1, |_buf| {})?;
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

    fn plain_text(spans: &[(String, Style)]) -> String {
        spans.iter().map(|(t, _)| t.as_str()).collect()
    }

    #[test]
    fn parse_inline_extracts_bold_italic_and_code_spans() {
        let spans = parse_inline("normal **bold** and *italic* and `code` done", Style::default());
        assert_eq!(plain_text(&spans), "normal bold and italic and code done");
        assert!(spans.iter().any(|(t, s)| t == "bold" && s.add_modifier == Modifier::BOLD));
        assert!(spans.iter().any(|(t, s)| t == "italic" && s.add_modifier == Modifier::ITALIC));
        assert!(spans.iter().any(|(t, _)| t == "code"));
    }

    #[test]
    fn parse_inline_leaves_an_unclosed_marker_as_literal_text() {
        let spans = parse_inline("this **never closes", Style::default());
        assert_eq!(plain_text(&spans), "this **never closes");
    }

    #[test]
    fn classify_and_strip_recognizes_headers_and_bullets() {
        let mut in_code = false;
        assert!(matches!(classify_and_strip("# Title", &mut in_code), Some((LineKind::Header, ref rest)) if rest == "Title"));
        assert!(matches!(classify_and_strip("- item", &mut in_code), Some((LineKind::Bullet, ref rest)) if rest == "item"));
        assert!(matches!(classify_and_strip("plain text", &mut in_code), Some((LineKind::Plain, ref rest)) if rest == "plain text"));
    }

    #[test]
    fn classify_and_strip_toggles_code_block_state_across_calls() {
        let mut in_code = false;
        assert!(classify_and_strip("```rust", &mut in_code).is_none());
        assert!(in_code);
        assert!(matches!(classify_and_strip("fn main() {}", &mut in_code), Some((LineKind::Code, _))));
        assert!(classify_and_strip("```", &mut in_code).is_none());
        assert!(!in_code);
    }

    #[test]
    fn wrap_spans_breaks_on_word_boundaries_and_preserves_style() {
        let spans = vec![("hello world foo".to_string(), Style::default())];
        let rows = wrap_spans(&spans, 8);
        let row_texts: Vec<String> = rows.iter().map(|row| row.iter().map(|(t, _)| t.as_str()).collect()).collect();
        assert_eq!(row_texts, vec!["hello ", "world ", "foo"]);
    }

    #[test]
    fn wrap_spans_hard_breaks_a_single_word_longer_than_the_width() {
        let spans = vec![("supercalifragilistic".to_string(), Style::default())];
        let rows = wrap_spans(&spans, 5);
        assert!(rows.iter().all(|row| row.iter().map(|(t, _)| UnicodeWidthStr::width(t.as_str())).sum::<usize>() <= 5));
        let rejoined: String = rows.iter().flatten().map(|(t, _)| t.as_str()).collect();
        assert_eq!(rejoined, "supercalifragilistic");
    }

    #[test]
    fn card_width_shrinks_to_fit_a_short_message_instead_of_the_full_terminal() {
        let body = vec![vec![("ok".to_string(), Style::default())]];
        assert_eq!(card_width(6, &body, 80), 12);
    }

    #[test]
    fn card_width_grows_up_to_its_widest_row_but_never_past_max_width() {
        let short_body = vec![vec![("hello there".to_string(), Style::default())]];
        assert_eq!(card_width(6, &short_body, 80), 15);

        let long_row = "x".repeat(200);
        let long_body = vec![vec![(long_row, Style::default())]];
        assert_eq!(card_width(6, &long_body, 80), 80);
    }
}
