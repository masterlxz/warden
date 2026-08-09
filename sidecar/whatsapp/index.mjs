// Baileys glue for Warden's WhatsApp channel (Fase 3). Talks to the parent `warden-whatsapp`
// Rust process over stdin/stdout, one JSON object per line:
//   sidecar -> Rust (stdout): {"type":"connected"}
//              {"type":"disconnected","loggedOut":bool}
//              {"type":"message","chatId":"...","senderName":"...","text":"..."|null}
//   Rust -> sidecar (stdin):  {"type":"send","chatId":"...","text":"..."}
// The QR code (first-run pairing) is rendered straight to stderr via qrcode-terminal — a
// separate stream from the stdout channel above, so it never corrupts the JSON protocol. The
// parent process just inherits this sidecar's stderr so the user sees the QR appear directly.
// Reconnect logic (WhatsApp-level, via DisconnectReason) stays entirely in this script; Rust
// only ever sees the high-level "connected"/"disconnected" events.

import fs from "node:fs";
import readline from "node:readline";

import { DisconnectReason, fetchLatestBaileysVersion, makeWASocket, useMultiFileAuthState } from "baileys";
import pino from "pino";
import qrcodeTerminal from "qrcode-terminal";

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
      // A callback makes qrcode-terminal hand back the rendered string instead of printing it
      // via console.log — console.log writes to stdout, which is the JSON-lines IPC channel to
      // the Rust parent; printing the QR there would corrupt the protocol. stderr is safe.
      qrcodeTerminal.generate(qr, { small: true }, (rendered) => process.stderr.write(`${rendered}\n`));
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
