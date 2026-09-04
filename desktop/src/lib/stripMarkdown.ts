/** Strips common CommonMark/GFM syntax from an assistant message before handing it to TTS
 * (P28 part 3) — otherwise "tts-1" reads asterisks, brackets and backticks out loud. Regex-based
 * rather than a full markdown AST pass (`remark`/`strip-markdown`): good enough for what a model
 * response actually produces, no new dependency for the same minimal-diff reason `transcribe.rs`
 * skipped an audio-format library. */
export function stripMarkdown(text: string): string {
  return text
    .replace(/```[a-zA-Z0-9]*\n?([\s\S]*?)```/g, "$1") // fenced code blocks: keep the content
    .replace(/`([^`]+)`/g, "$1") // inline code
    .replace(/!\[([^\]]*)\]\([^)]*\)/g, "$1") // images: keep alt text
    .replace(/\[([^\]]+)\]\([^)]*\)/g, "$1") // links: keep link text
    .replace(/^#{1,6}\s+/gm, "") // headings
    .replace(/^\s*>\s?/gm, "") // blockquotes
    .replace(/^\s*[-*+]\s+/gm, "") // unordered list markers
    .replace(/^\s*\d+\.\s+/gm, "") // ordered list markers
    .replace(/(\*\*|__)(.*?)\1/g, "$2") // bold
    .replace(/(\*|_)(.*?)\1/g, "$2") // italic
    .replace(/~~(.*?)~~/g, "$1") // strikethrough
    .trim();
}
