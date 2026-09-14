import { useRef, useState } from "react";
import type { AnchorHTMLAttributes } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { openUrl } from "@tauri-apps/plugin-opener";
import { invoke } from "@tauri-apps/api/core";
import type { Attachment, ChatMessage } from "../types";
import { LogoMark, SpeakerIcon, StopIcon } from "./Icons";
import { stripMarkdown } from "../lib/stripMarkdown";

interface MessageBubbleProps {
  message: ChatMessage;
}

type SpeechState = "idle" | "loading" | "playing";

/** Per-message play button for TTS (P28 part 3) — synthesizes on click via the same Whisper API
 * key as voice input (`synthesize_speech` IPC), plays it, and toggles back to idle on a second
 * click (always restarts from the top next time, not a pause/resume). */
function SpeakButton({ text }: { text: string }) {
  const [state, setState] = useState<SpeechState>("idle");
  const [error, setError] = useState<string | null>(null);
  const audioRef = useRef<HTMLAudioElement | null>(null);

  async function handleClick() {
    if (state === "playing") {
      audioRef.current?.pause();
      if (audioRef.current) audioRef.current.currentTime = 0;
      setState("idle");
      return;
    }

    setError(null);
    setState("loading");
    try {
      const audio = await invoke<Attachment>("synthesize_speech", { text: stripMarkdown(text) });
      const element = new Audio(`data:${audio.mimeType};base64,${audio.data}`);
      element.onended = () => setState("idle");
      audioRef.current = element;
      await element.play();
      setState("playing");
    } catch (err) {
      setError(String(err));
      setState("idle");
    }
  }

  return (
    <>
      <button
        type="button"
        className={`message-speak-btn${state === "playing" ? " message-speak-btn--playing" : ""}`}
        aria-label={state === "playing" ? "Stop speaking" : "Speak this message"}
        onClick={handleClick}
        disabled={state === "loading"}
      >
        {state === "playing" ? <StopIcon size={14} /> : <SpeakerIcon size={14} />}
      </button>
      {error && <p className="chat-attach-error" role="alert">{error}</p>}
    </>
  );
}

/** Renders one `Attachment` as the right native media element for its `mimeType` — an `<img>`
 * for images (the only kind before P64 frente 2, user-attached only), and native browser
 * controls (no custom player needed, unlike `SpeakButton`) for audio/video that an MCP tool
 * produced during a turn. */
function AttachmentPreview({ attachment }: { attachment: Attachment }) {
  const src = `data:${attachment.mimeType};base64,${attachment.data}`;
  if (attachment.mimeType.startsWith("audio/")) {
    return <audio controls src={src} />;
  }
  if (attachment.mimeType.startsWith("video/")) {
    return <video controls src={src} />;
  }
  return <img src={src} alt="" />;
}

/** One "Open" button for a file `generate_document`/oversized MCP media (P64) wrote to disk this
 * turn — hands the path to the Rust `open_generated_file` command, which opens it with the OS
 * default app (and refuses anything outside the trusted generated-files directory). Same
 * inline-error-without-blocking-dialog pattern as `SpeakButton` above. */
function GeneratedFileButton({ path }: { path: string }) {
  const [error, setError] = useState<string | null>(null);
  const name = path.split(/[/\\]/).pop() || path;

  async function handleClick() {
    setError(null);
    try {
      await invoke("open_generated_file", { path });
    } catch (err) {
      setError(String(err));
    }
  }

  return (
    <>
      <button type="button" className="message-file-btn" onClick={handleClick}>
        📄 Open {name}
      </button>
      {error && <p className="chat-attach-error" role="alert">{error}</p>}
    </>
  );
}

// Links must open in the user's default browser, not navigate the app's own webview away.
// Exported for reuse by `VaultView` (P52 part 2), which renders markdown outside chat bubbles.
export function MarkdownLink(props: AnchorHTMLAttributes<HTMLAnchorElement>) {
  const { href, children, ...rest } = props;
  return (
    <a
      {...rest}
      href={href}
      onClick={(event) => {
        event.preventDefault();
        if (href) void openUrl(href);
      }}
    >
      {children}
    </a>
  );
}

function MessageBubble({ message }: MessageBubbleProps) {
  if (message.role === "assistant") {
    return (
      <div className="message-row message-row--assistant">
        <div className="message-avatar">
          <LogoMark size={18} />
        </div>
        <div className="message-assistant-body">
          {message.attachments && message.attachments.length > 0 && (
            <div className="message-bubble-attachments">
              {message.attachments.map((attachment, index) => (
                <AttachmentPreview key={index} attachment={attachment} />
              ))}
            </div>
          )}
          <div className="message-bubble-content">
            <ReactMarkdown remarkPlugins={[remarkGfm]} components={{ a: MarkdownLink }}>
              {message.content}
            </ReactMarkdown>
          </div>
          {message.generatedFiles && message.generatedFiles.length > 0 && (
            <div className="message-bubble-files">
              {message.generatedFiles.map((path, index) => (
                <GeneratedFileButton key={index} path={path} />
              ))}
            </div>
          )}
          <div className="message-bubble-footer">
            <SpeakButton text={message.content} />
            {message.usage && <span className="message-bubble-usage">{message.usage.totalTokens} tokens</span>}
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="message-row message-row--user">
      <div className="message-bubble message-bubble--user">
        {message.attachments && message.attachments.length > 0 && (
          <div className="message-bubble-attachments">
            {message.attachments.map((attachment, index) => (
              <AttachmentPreview key={index} attachment={attachment} />
            ))}
          </div>
        )}
        {message.content && (
          <div className="message-bubble-content">
            <ReactMarkdown remarkPlugins={[remarkGfm]} components={{ a: MarkdownLink }}>
              {message.content}
            </ReactMarkdown>
          </div>
        )}
      </div>
    </div>
  );
}

export default MessageBubble;
