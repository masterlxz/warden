import { useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { invoke } from "@tauri-apps/api/core";
import { AttachIcon, CloseIcon, MicIcon, SendIcon } from "./Icons";
import type { Attachment } from "../types";

interface MessageInputProps {
  onSend: (content: string, attachments: Attachment[]) => void;
  /** Bumped by the parent whenever the active conversation changes, so the composer refocuses. */
  focusKey?: string | null;
  disabled?: boolean;
}

/** Candidates in preference order — the first one WebKitGTK/Chromium/Firefox actually supports
 * wins. Whisper accepts all of webm/ogg/mp4, so any of these works once picked. */
const RECORDING_MIME_CANDIDATES = ["audio/webm;codecs=opus", "audio/webm", "audio/ogg;codecs=opus", "audio/ogg", "audio/mp4"];

function pickRecordingMimeType(): string | undefined {
  return RECORDING_MIME_CANDIDATES.find((type) => typeof MediaRecorder !== "undefined" && MediaRecorder.isTypeSupported(type));
}

function blobToBase64(blob: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onloadend = () => {
      const result = reader.result as string;
      resolve(result.slice(result.indexOf(",") + 1));
    };
    reader.onerror = () => reject(reader.error);
    reader.readAsDataURL(blob);
  });
}

function MessageInput({ onSend, focusKey, disabled }: MessageInputProps) {
  const [draft, setDraft] = useState("");
  const [attachments, setAttachments] = useState<Attachment[]>([]);
  const [attachError, setAttachError] = useState<string | null>(null);
  const [isRecording, setIsRecording] = useState(false);
  const [isTranscribing, setIsTranscribing] = useState(false);
  const [recordError, setRecordError] = useState<string | null>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const mediaRecorderRef = useRef<MediaRecorder | null>(null);
  const chunksRef = useRef<Blob[]>([]);

  useEffect(() => {
    textareaRef.current?.focus();
  }, [focusKey]);

  function submit() {
    const content = draft.trim();
    if ((content === "" && attachments.length === 0) || disabled) return;
    onSend(content, attachments);
    setDraft("");
    setAttachments([]);
  }

  async function handleAttach() {
    setAttachError(null);
    const selected = await open({
      multiple: true,
      filters: [{ name: "Images", extensions: ["png", "jpg", "jpeg", "webp", "gif"] }],
    });
    if (!selected) return;

    const paths = Array.isArray(selected) ? selected : [selected];
    for (const path of paths) {
      try {
        const attachment = await invoke<Attachment>("read_attachment", { path });
        setAttachments((prev) => [...prev, attachment]);
      } catch (err) {
        setAttachError(String(err));
      }
    }
  }

  function removeAttachment(index: number) {
    setAttachments((prev) => prev.filter((_, i) => i !== index));
  }

  async function startRecording() {
    setRecordError(null);
    try {
      const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
      const mimeType = pickRecordingMimeType();
      const recorder = mimeType ? new MediaRecorder(stream, { mimeType }) : new MediaRecorder(stream);
      chunksRef.current = [];

      recorder.ondataavailable = (e) => {
        if (e.data.size > 0) chunksRef.current.push(e.data);
      };
      recorder.onstop = () => {
        stream.getTracks().forEach((track) => track.stop());
        void handleRecordingStopped(recorder.mimeType || mimeType || "audio/webm");
      };

      mediaRecorderRef.current = recorder;
      recorder.start();
      setIsRecording(true);
    } catch (err) {
      setRecordError(err instanceof Error ? err.message : String(err));
    }
  }

  function stopRecording() {
    mediaRecorderRef.current?.stop();
    setIsRecording(false);
  }

  async function handleRecordingStopped(mimeType: string) {
    const blob = new Blob(chunksRef.current, { type: mimeType });
    chunksRef.current = [];
    if (blob.size === 0) return;

    setIsTranscribing(true);
    try {
      const data = await blobToBase64(blob);
      const text = await invoke<string>("transcribe_audio", { audio: { mimeType, data } });
      const trimmed = text.trim();
      if (trimmed !== "") {
        setDraft((prev) => (prev.trim() === "" ? trimmed : `${prev} ${trimmed}`));
        textareaRef.current?.focus();
      }
    } catch (err) {
      setRecordError(String(err));
    } finally {
      setIsTranscribing(false);
    }
  }

  function handleMicClick() {
    if (isRecording) {
      stopRecording();
    } else {
      void startRecording();
    }
  }

  return (
    <div className="chat-input-dock">
      {attachError && <p className="chat-attach-error" role="alert">{attachError}</p>}
      {recordError && <p className="chat-attach-error" role="alert">{recordError}</p>}
      {attachments.length > 0 && (
        <div className="chat-attachment-preview">
          {attachments.map((attachment, index) => (
            <div className="chat-attachment-thumb" key={index}>
              <img src={`data:${attachment.mimeType};base64,${attachment.data}`} alt="" />
              <button
                type="button"
                className="chat-attachment-remove"
                aria-label="Remove attachment"
                onClick={() => removeAttachment(index)}
              >
                <CloseIcon size={11} />
              </button>
            </div>
          ))}
        </div>
      )}
      <form
        className="chat-input-form"
        onSubmit={(e) => {
          e.preventDefault();
          submit();
        }}
      >
        <button type="button" className="chat-attach-btn" aria-label="Attach image" onClick={handleAttach} disabled={disabled || isRecording}>
          <AttachIcon size={17} />
        </button>
        <button
          type="button"
          className={`chat-mic-btn${isRecording ? " chat-mic-btn--recording" : ""}`}
          aria-label={isRecording ? "Stop recording" : "Record voice message"}
          onClick={handleMicClick}
          disabled={disabled || isTranscribing}
        >
          <MicIcon size={17} />
        </button>
        <textarea
          ref={textareaRef}
          className="chat-input-textarea"
          aria-label="Message"
          placeholder={isTranscribing ? "Transcribing…" : "Message Warden…"}
          value={draft}
          onChange={(e) => setDraft(e.currentTarget.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              submit();
            }
          }}
          disabled={disabled || isTranscribing}
          rows={1}
        />
        <button
          type="submit"
          className="chat-send-btn"
          aria-label="Send message"
          disabled={disabled || (draft.trim() === "" && attachments.length === 0)}
        >
          <SendIcon size={17} />
        </button>
      </form>
      <p className="chat-input-hint">Warden can make mistakes. Check important info.</p>
    </div>
  );
}

export default MessageInput;
