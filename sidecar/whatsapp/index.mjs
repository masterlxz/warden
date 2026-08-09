// Baileys glue for Warden's WhatsApp channel (Fase 3). Talks to the parent `warden-whatsapp`
// Rust process over stdin/stdout, one JSON object per line:
//   sidecar -> Rust (stdout): {"type":"connected"}
//              {"type":"disconnected","loggedOut":bool}
//              {"type":"message","chatId":"...","senderName":"...","text":"..."|null}
//   Rust -> sidecar (stdin):  {"type":"send","chatId":"...","text":"..."}
// The QR code (first-run pairing) is written as a PNG next to the auth state and the file path
// logged to stderr — not rendered as terminal ASCII art. A terminal QR needs the half-block
// Unicode trick to look square, which depends on the terminal font's exact character aspect
// ratio; when that assumption is off the code renders visibly stretched, and while a generic
// camera QR reader tolerates that distortion, WhatsApp's own in-app scanner does not. A real PNG
// has no such dependency. Either way this stays off stdout, which is the JSON-lines IPC channel
// to the Rust parent — printing anything else there would corrupt the protocol.
// Reconnect logic (WhatsApp-level, via DisconnectReason) stays entirely in this script; Rust
// only ever sees the high-level "connected"/"disconnected" events.

import fs from "node:fs";
import path from "node:path";
import readline from "node:readline";

import { DisconnectReason, fetchLatestBaileysVersion, makeWASocket, useMultiFileAuthState } from "baileys";
import pino from "pino";
import QRCode from "qrcode";

const authDir = process.env.WARDEN_WHATSAPP_AUTH_DIR;
if (!authDir) {
  console.error("WARDEN_WHATSAPP_AUTH_DIR not set");
  process.exit(1);
}
fs.mkdirSync(authDir, { recursive: true });

const logger = pino({ level: "silent" });

function emit(event) {
  process.stdout.write(`${JSON.stringify(event)}\n`);
}

function extractText(message) {
  if (!message) return null;
  return (
    message.conversation ??
    message.extendedTextMessage?.text ??
    message.imageMessage?.caption ??
    message.videoMessage?.caption ??
    null
  );
}

let sock = null;

async function connect() {
  const { state, saveCreds } = await useMultiFileAuthState(authDir);
  const { version } = await fetchLatestBaileysVersion();

  sock = makeWASocket({ version, logger, auth: state });

  sock.ev.on("creds.update", saveCreds);

  sock.ev.on("connection.update", (update) => {
    const { connection, lastDisconnect, qr } = update;

    if (qr) {
      const qrPath = path.join(authDir, "qr.png");
      QRCode.toFile(qrPath, qr, { width: 512 })
        .then(() => {
          console.error(
            `QR code saved to ${qrPath} — open it in an image viewer and scan with WhatsApp (Settings > Linked Devices > Link a Device).`
          );
        })
        .catch((err) => console.error("failed to generate QR code image:", err));
    }

    if (connection === "open") {
      emit({ type: "connected" });
    } else if (connection === "close") {
      const statusCode = lastDisconnect?.error?.output?.statusCode;
      const loggedOut = statusCode === DisconnectReason.loggedOut;
      emit({ type: "disconnected", loggedOut });
      if (!loggedOut) {
        connect().catch((err) => {
          console.error("failed to reconnect:", err);
          process.exit(1);
        });
      }
    }
  });

  sock.ev.on("messages.upsert", ({ messages, type }) => {
    if (type !== "notify") return;
    for (const msg of messages) {
      if (msg.key.fromMe) continue;
      const chatId = msg.key.remoteJid;
      if (!chatId) continue;
      emit({ type: "message", chatId, senderName: msg.pushName ?? null, text: extractText(msg.message) });
    }
  });
}

const rl = readline.createInterface({ input: process.stdin });
rl.on("line", async (line) => {
  if (!line.trim()) return;

  let command;
  try {
    command = JSON.parse(line);
  } catch (err) {
    console.error("failed to parse command from stdin:", err);
    return;
  }

  if (command.type === "send" && sock) {
    try {
      await sock.sendMessage(command.chatId, { text: command.text });
    } catch (err) {
      console.error(`failed to send message to ${command.chatId}:`, err);
    }
  }
});

connect().catch((err) => {
  console.error("fatal error starting WhatsApp sidecar:", err);
  process.exit(1);
});
