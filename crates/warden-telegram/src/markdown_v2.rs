//! CommonMark → Telegram MarkdownV2 (P19). Telegram's reserved-character rules:
//! <https://core.telegram.org/bots/api#markdownv2-style> — a message is rejected outright if a
//! reserved character isn't escaped correctly, which is why `TelegramClient::send_message`
//! (`telegram.rs`) always keeps a plain-text fallback around a call to `to_markdown_v2`.
//!
//! Uses a real CommonMark parser (`pulldown-cmark`) instead of regex — regex breaks on any
//! nesting (bold inside a list item, code inside a link, etc.). `ENABLE_STRIKETHROUGH` is the only
//! extension turned on, to match the desktop's own `remark-gfm` (`MessageBubble.tsx`).
//! Deliberately NOT `ENABLE_TABLES`/`ENABLE_TASKLISTS`: without them, `pulldown-cmark` treats that
//! syntax as plain paragraph text, which the text escaper below already degrades into something
//! readable — Telegram has no native table rendering to target anyway.

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

/// Reserved outside any entity — escaped with a preceding `\`. Backslash itself is in this set.
const RESERVED_IN_TEXT: &[char] = &['_', '*', '[', ']', '(', ')', '~', '`', '>', '#', '+', '-', '=', '|', '{', '}', '.', '!', '\\'];

fn escape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if RESERVED_IN_TEXT.contains(&c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Inside `code`/`pre` entities Telegram only requires escaping backtick and backslash.
fn escape_code(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c == '`' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Inside the `(...)` of a link, Telegram only requires escaping `)` and `\`.
fn escape_link_url(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c == ')' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

enum ListKind {
    Unordered,
    Ordered(u64),
}

/// Converts `source` (CommonMark, as the model writes it) into Telegram's MarkdownV2 dialect.
pub fn to_markdown_v2(source: &str) -> String {
    let parser = Parser::new_ext(source, Options::ENABLE_STRIKETHROUGH);

    let mut out = String::new();
    let mut list_stack: Vec<ListKind> = Vec::new();
    let mut link_url: Option<String> = None;
    // `pulldown-cmark` delivers a code block's body as plain `Event::Text` (`Event::Code` is only
    // for inline spans) — this flag routes those through `escape_code` instead of `escape_text`.
    let mut in_code_block = false;
    // Byte offsets into `out` where a blockquote started, so `TagEnd::BlockQuote` can slice out
    // everything emitted since and re-prefix each line with `>` — the only way to apply a
    // per-line marker to content that may itself contain nested entities/newlines.
    let mut blockquote_starts: Vec<usize> = Vec::new();

    for event in parser {
        match event {
            Event::Start(tag) => match tag {
                Tag::Strong => out.push('*'),
                Tag::Emphasis => out.push('_'),
                Tag::Strikethrough => out.push('~'),
                Tag::Heading { .. } => out.push('*'),
                Tag::BlockQuote(_) => blockquote_starts.push(out.len()),
                Tag::Link { dest_url, .. } => {
                    link_url = Some(dest_url.to_string());
                    out.push('[');
                }
                Tag::CodeBlock(kind) => {
                    in_code_block = true;
                    out.push_str("```");
                    if let CodeBlockKind::Fenced(lang) = kind {
                        out.push_str(&lang);
                    }
                    out.push('\n');
                }
                Tag::List(start) => list_stack.push(match start {
                    Some(n) => ListKind::Ordered(n),
                    None => ListKind::Unordered,
                }),
                Tag::Item => {
                    let prefix = match list_stack.last_mut() {
                        Some(ListKind::Ordered(n)) => {
                            let prefix = format!("{n}\\. ");
                            *n += 1;
                            prefix
                        }
                        _ => "• ".to_string(),
                    };
                    out.push_str(&prefix);
                }
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Strong => out.push('*'),
                TagEnd::Emphasis => out.push('_'),
                TagEnd::Strikethrough => out.push('~'),
                TagEnd::Heading(_) => {
                    out.push('*');
                    out.push('\n');
                }
                TagEnd::BlockQuote(_) => {
                    if let Some(start) = blockquote_starts.pop() {
                        let inner = out.split_off(start);
                        for line in inner.trim_end_matches('\n').split('\n') {
                            out.push('>');
                            out.push_str(line);
                            out.push('\n');
                        }
                    }
                }
                TagEnd::Link => {
                    out.push(']');
                    out.push('(');
                    if let Some(url) = link_url.take() {
                        out.push_str(&escape_link_url(&url));
                    }
                    out.push(')');
                }
                TagEnd::CodeBlock => {
                    in_code_block = false;
                    out.push_str("```\n");
                }
                TagEnd::List(_) => {
                    list_stack.pop();
                }
                TagEnd::Item | TagEnd::Paragraph => out.push('\n'),
                _ => {}
            },
            Event::Text(text) => {
                if in_code_block {
                    out.push_str(&escape_code(&text));
                } else {
                    out.push_str(&escape_text(&text));
                }
            }
            Event::Code(text) => {
                out.push('`');
                out.push_str(&escape_code(&text));
                out.push('`');
            }
            Event::Html(html) | Event::InlineHtml(html) => out.push_str(&escape_text(&html)),
            Event::SoftBreak | Event::HardBreak => out.push('\n'),
            Event::Rule => out.push_str("\\-\\-\\-\n"),
            _ => {}
        }
    }

    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_with_reserved_characters_is_escaped() {
        assert_eq!(to_markdown_v2("1. done! (really)"), "1\\. done\\! \\(really\\)");
    }

    #[test]
    fn bold_and_italic() {
        assert_eq!(to_markdown_v2("**bold** and *italic*"), "*bold* and _italic_");
    }

    #[test]
    fn nested_bold_italic() {
        // pulldown-cmark parses `***x***` as Emphasis wrapping Strong — either nesting order is
        // valid MarkdownV2 (both entities still wrap the same text), this just locks in which one
        // the parser actually produces.
        assert_eq!(to_markdown_v2("***both***"), "_*both*_");
    }

    #[test]
    fn strikethrough() {
        assert_eq!(to_markdown_v2("~~gone~~"), "~gone~");
    }

    #[test]
    fn inline_code_escapes_only_backtick_and_backslash() {
        assert_eq!(to_markdown_v2("`a.b\\c`"), "`a.b\\\\c`");
    }

    #[test]
    fn fenced_code_block_keeps_the_language() {
        assert_eq!(to_markdown_v2("```rust\nfn main() {}\n```"), "```rust\nfn main() {}\n```");
    }

    #[test]
    fn link_escapes_text_and_url_differently() {
        // `<...>` is CommonMark's angle-bracket destination form — needed here so the `)` inside
        // the URL is part of the destination instead of closing the link early.
        assert_eq!(to_markdown_v2("[a.b](<https://x.com/a)b>)"), "[a\\.b](https://x.com/a\\)b)");
    }

    #[test]
    fn unordered_list() {
        assert_eq!(to_markdown_v2("- one\n- two"), "• one\n• two");
    }

    #[test]
    fn ordered_list_keeps_the_start_number() {
        assert_eq!(to_markdown_v2("5. one\n6. two"), "5\\. one\n6\\. two");
    }

    #[test]
    fn blockquote_prefixes_every_line() {
        assert_eq!(to_markdown_v2("> line one\n> line two"), ">line one\n>line two");
    }

    #[test]
    fn heading_becomes_bold() {
        assert_eq!(to_markdown_v2("# Title"), "*Title*");
    }
}
