/**
 * Voice input (P78): records the microphone with `MediaRecorder`, for the hub to transcribe. The
 * browser only exposes the microphone on a secure origin — `https://` (the hub with Tailscale TLS,
 * P36) or `localhost` — so on a plain `http://` LAN address `canRecord()` is false.
 */

import { toBase64 } from "./attachments";
import type { Attachment } from "./messages";

/** In order of preference; Chrome/Firefox record webm/opus, Safari mp4. */
const RECORDING_TYPES = ["audio/webm;codecs=opus", "audio/webm", "audio/mp4", "audio/ogg;codecs=opus"];

export function canRecord(): boolean {
  return window.isSecureContext && typeof MediaRecorder !== "undefined" && navigator.mediaDevices?.getUserMedia !== undefined;
}

export class VoiceRecorder {
  private readonly chunks: Blob[] = [];

  private constructor(
    private readonly recorder: MediaRecorder,
    private readonly stream: MediaStream,
  ) {
    recorder.addEventListener("dataavailable", (event) => {
      if (event.data.size > 0) this.chunks.push(event.data);
    });
  }

  /** Asks for the microphone (the browser may prompt) and starts recording. */
  static async start(): Promise<VoiceRecorder> {
    const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
    const mimeType = RECORDING_TYPES.find((type) => MediaRecorder.isTypeSupported(type));
    const recorder = new MediaRecorder(stream, mimeType ? { mimeType } : undefined);
    const voice = new VoiceRecorder(recorder, stream);
    recorder.start();
    return voice;
  }

  /** Stops and hands back the recording. */
  stop(): Promise<Attachment> {
    return new Promise((resolve, reject) => {
      this.recorder.addEventListener(
        "stop",
        () => {
          this.release();
          const blob = new Blob(this.chunks, { type: this.recorder.mimeType || "audio/webm" });
          if (blob.size === 0) {
            reject(new Error("nada foi gravado"));
            return;
          }
          toBase64(blob).then((data) => resolve({ mimeType: blob.type, data }), reject);
        },
        { once: true },
      );
      this.recorder.stop();
    });
  }

  /** Stops without keeping anything (e.g. the page is leaving). */
  cancel(): void {
    if (this.recorder.state !== "inactive") this.recorder.stop();
    this.release();
  }

  /** Turns the microphone off — the browser's "recording" indicator goes away. */
  private release(): void {
    for (const track of this.stream.getTracks()) track.stop();
  }
}
