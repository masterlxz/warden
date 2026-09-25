/**
 * Turns files the user picks, drops or pastes into what a `chat` turn carries (P78):
 * - images, resized to at most `MAX_IMAGE_SIDE` px and re-encoded as JPEG unless already small
 *   (a GIF goes as is, to keep its animation);
 * - PDFs, as they are;
 * - text files, read here and put into the message itself as a fenced block — no provider needs
 *   anything new for that, and the model sees the file name.
 * The hub checks types and the total size again (`chat_input.rs`); the limits here match it.
 */

import type { Attachment } from "./messages";

export const MAX_IMAGE_SIDE = 2048;
/** The hub's cap on a turn's attachments, in base64 (`MAX_ATTACHMENTS_BASE64_BYTES`). */
export const MAX_ATTACHMENTS_BASE64_BYTES = 12 * 1024 * 1024;
export const MAX_ATTACHMENTS_PER_TURN = 10;
const MAX_TEXT_FILE_BYTES = 200 * 1024;
/** An image already this small and within `MAX_IMAGE_SIDE` is sent untouched. */
const KEEP_IMAGE_BYTES = 1024 * 1024;
const JPEG_QUALITY = 0.85;

const IMAGE_TYPES = ["image/png", "image/jpeg", "image/webp", "image/gif"];
const TEXT_EXTENSIONS = /\.(txt|md|markdown|csv|tsv|json|jsonl|ya?ml|toml|xml|html?|css|js|jsx|ts|tsx|py|rs|go|rb|java|kt|c|h|cpp|hpp|cs|php|sh|sql|log|ini|conf|env)$/i;

/** One thing in the composer, ready to send. */
export type PendingAttachment =
  | { kind: "media"; name: string; attachment: Attachment }
  | { kind: "text"; name: string; text: string };

export class AttachmentError extends Error {}

export async function prepareFile(file: File): Promise<PendingAttachment> {
  const name = file.name || "arquivo";
  if (IMAGE_TYPES.includes(file.type)) return { kind: "media", name, attachment: await prepareImage(file) };
  if (file.type === "application/pdf") {
    return { kind: "media", name, attachment: { mimeType: "application/pdf", data: await toBase64(file) } };
  }
  if (file.type.startsWith("text/") || file.type === "application/json" || TEXT_EXTENSIONS.test(name)) {
    if (file.size > MAX_TEXT_FILE_BYTES) {
      throw new AttachmentError(`${name} é grande demais para ir como texto (máximo ${MAX_TEXT_FILE_BYTES / 1024} KB)`);
    }
    return { kind: "text", name, text: await file.text() };
  }
  throw new AttachmentError(`${name}: tipo não suportado (imagens, PDF e arquivos de texto)`);
}

async function prepareImage(file: File): Promise<Attachment> {
  if (file.type === "image/gif") return { mimeType: file.type, data: await toBase64(file) };
  const bitmap = await createImageBitmap(file);
  try {
    const scale = Math.min(1, MAX_IMAGE_SIDE / Math.max(bitmap.width, bitmap.height));
    if (scale === 1 && file.size <= KEEP_IMAGE_BYTES) return { mimeType: file.type, data: await toBase64(file) };
    const canvas = document.createElement("canvas");
    canvas.width = Math.round(bitmap.width * scale);
    canvas.height = Math.round(bitmap.height * scale);
    const context = canvas.getContext("2d");
    if (!context) throw new AttachmentError("este navegador não conseguiu processar a imagem");
    // JPEG has no transparency: a white background instead of black.
    context.fillStyle = "#fff";
    context.fillRect(0, 0, canvas.width, canvas.height);
    context.drawImage(bitmap, 0, 0, canvas.width, canvas.height);
    const blob = await new Promise<Blob | null>((resolve) => canvas.toBlob(resolve, "image/jpeg", JPEG_QUALITY));
    if (!blob) throw new AttachmentError("este navegador não conseguiu processar a imagem");
    return { mimeType: "image/jpeg", data: await toBase64(blob) };
  } finally {
    bitmap.close();
  }
}

/** Raw base64, without the `data:...;base64,` prefix. */
export function toBase64(blob: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => {
      const url = reader.result as string;
      resolve(url.slice(url.indexOf(",") + 1));
    };
    reader.onerror = () => reject(reader.error ?? new Error("could not read the file"));
    reader.readAsDataURL(blob);
  });
}

/** Why these can't go in one turn, or null when they can. */
export function checkLimits(pending: PendingAttachment[]): string | null {
  const media = pending.filter((p) => p.kind === "media");
  if (media.length > MAX_ATTACHMENTS_PER_TURN) return `no máximo ${MAX_ATTACHMENTS_PER_TURN} imagens/PDFs por mensagem`;
  const total = media.reduce((sum, p) => sum + p.attachment.data.length, 0);
  if (total > MAX_ATTACHMENTS_BASE64_BYTES) {
    return `anexos grandes demais (${(total / 1024 / 1024).toFixed(1)} MB; o máximo é ${MAX_ATTACHMENTS_BASE64_BYTES / 1024 / 1024} MB por mensagem)`;
  }
  return null;
}

/** The message as sent: the typed text, then each text file as a fenced block under its name. */
export function composeMessage(draft: string, pending: PendingAttachment[]): string {
  const blocks = pending.flatMap((p) => {
    if (p.kind !== "text") return [];
    // A fence longer than any run of backticks in the file, so the file can't close it early.
    const longestRun = Math.max(0, ...(p.text.match(/`+/g) ?? []).map((run) => run.length));
    const fence = "`".repeat(Math.max(3, longestRun + 1));
    return [`📎 ${p.name}\n${fence}\n${p.text}\n${fence}`];
  });
  return [draft, ...blocks].filter((part) => part.trim() !== "").join("\n\n");
}

export function mediaOf(pending: PendingAttachment[]): Attachment[] {
  return pending.flatMap((p) => (p.kind === "media" ? [p.attachment] : []));
}
