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

// Links must open in the user's default browser, not navigate the app's own webview away.
function MarkdownLink(props: AnchorHTMLAttributes<HTMLAnchorElement>) {
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
          <div className="message-bubble-content">
            <ReactMarkdown remarkPlugins={[remarkGfm]} components={{ a: MarkdownLink }}>
              {message.content}
            </ReactMarkdown>
          </div>
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
              <img key={index} src={`data:${attachment.mimeType};base64,${attachment.data}`} alt="" />
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
