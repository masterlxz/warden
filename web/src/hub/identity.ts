/**
 * This browser's identity as a paired device (P78): a `deviceId` made once, a name, and the token
 * the hub issued in `helloAck` (P36). Kept in `localStorage`, which can be missing or throw
 * (private windows, blocked site data) — then every visit just pairs again.
 */

const STORAGE_KEY = "warden.web.identity";

export interface Identity {
  deviceId: string;
  deviceName: string;
  deviceToken?: string;
}

export function loadIdentity(): Identity {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (raw) {
      const parsed = JSON.parse(raw) as Partial<Identity>;
      if (typeof parsed.deviceId === "string" && parsed.deviceId) {
        return {
          deviceId: parsed.deviceId,
          deviceName: typeof parsed.deviceName === "string" && parsed.deviceName ? parsed.deviceName : defaultDeviceName(),
          ...(typeof parsed.deviceToken === "string" && parsed.deviceToken && { deviceToken: parsed.deviceToken }),
        };
      }
    }
  } catch {
    // unreadable storage: fall through to a fresh identity
  }
  const fresh: Identity = { deviceId: newDeviceId(), deviceName: defaultDeviceName() };
  saveIdentity(fresh);
  return fresh;
}

export function saveIdentity(identity: Identity): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(identity));
  } catch {
    // best effort — see the module doc
  }
}

/** `crypto.randomUUID` only exists on secure origins, and a LAN hub is usually plain `http://` —
 * `getRandomValues` works on both. */
function newDeviceId(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  return `web-${Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("")}`;
}

/** "Navegador (Firefox, Linux)" — just enough to tell devices apart in the hub's device list. */
export function defaultDeviceName(): string {
  const ua = navigator.userAgent;
  const browser = /Edg\//.test(ua)
    ? "Edge"
    : /Firefox\//.test(ua)
      ? "Firefox"
      : /Chrome\//.test(ua)
        ? "Chrome"
        : /Safari\//.test(ua)
          ? "Safari"
          : null;
  const os = /Android/.test(ua)
    ? "Android"
    : /iPhone|iPad/.test(ua)
      ? "iOS"
      : /Windows/.test(ua)
        ? "Windows"
        : /Mac OS X/.test(ua)
          ? "macOS"
          : /Linux/.test(ua)
            ? "Linux"
            : null;
  const detail = [browser, os].filter(Boolean).join(", ");
  return detail ? `Navegador (${detail})` : "Navegador";
}
