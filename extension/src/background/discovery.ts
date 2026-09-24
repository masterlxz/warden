/**
 * Fase 9.1 (redefined) — TypeScript port of `crates/warden-server-protocol/src/discovery.rs`'s
 * LAN sweep. The extension can't call Rust like the desktop (Tauri) and mobile
 * (`flutter_rust_bridge`) slices do, so this reimplements the same `Discover`/`DiscoverAck` wire
 * exchange directly over `WebSocket` — the server side is unchanged, all three clients speak the
 * same protocol.
 */

import { encode, decode, type ServerMessage } from "../protocol/messages";

export interface DiscoveredHub {
  host: string;
  port: number;
  serverName: string;
  /** Set when the hub only accepts wss:// (P36) — where to connect instead of ws://host:port. */
  secureUrl?: string;
}

/** A TLS-only hub (P36) answers plain ws:// only on this path; others ignore the path. Same
 * constant as `warden_server_protocol::tls::DISCOVER_PATH`. */
const DISCOVER_PATH = "/discover";

const PROBE_TIMEOUT_MS = 800;
const CONCURRENCY = 50; // same value as the Rust sweep

/** Sweeps every local network for a hub listening on `port`. A single pass (~1-2s) — same
 * reasoning as the Rust side: no code is exchanged, so any host can answer, and there's no
 * "phone might not be ready yet" to retry for — a "Procurar" click is the retry. */
export async function discoverHubs(port: number): Promise<DiscoveredHub[]> {
  const hosts = await candidateHosts();
  const results = await mapWithConcurrency(hosts, CONCURRENCY, (host) => probeOne(host, port));
  return results.filter((hub): hub is DiscoveredHub => hub !== null);
}

/** Every host on the /24 of every local, non-loopback IPv4 interface — mirrors
 * `warden_truthid::lan::candidate_hosts()`'s "if-addrs then expand" shape, built here from
 * `chrome.system.network.getNetworkInterfaces()` since that's the only way an extension can learn
 * its own local IP (no `if-addrs` equivalent in this context). */
async function candidateHosts(): Promise<string[]> {
  const interfaces = await chrome.system.network.getNetworkInterfaces();
  const hosts = new Set<string>();
  for (const iface of interfaces) {
    const octets = iface.address.split(".");
    if (octets.length !== 4 || iface.address.startsWith("127.")) continue; // IPv6 or loopback
    for (let last = 1; last <= 254; last++) {
      hosts.add(`${octets[0]}.${octets[1]}.${octets[2]}.${last}`);
    }
  }
  return [...hosts];
}

/** One connect+Discover+DiscoverAck attempt against a single `host:port`. `null` covers every
 * "this wasn't a hub" outcome (nothing listening, timed out, malformed/unexpected reply) — none of
 * those should abort the sweep, since a different host on the LAN might still answer. */
function probeOne(host: string, port: number): Promise<DiscoveredHub | null> {
  return new Promise((resolve) => {
    let settled = false;
    const socket = new WebSocket(`ws://${host}:${port}${DISCOVER_PATH}`);

    const finish = (result: DiscoveredHub | null) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      socket.close();
      resolve(result);
    };

    const timer = setTimeout(() => finish(null), PROBE_TIMEOUT_MS);

    socket.addEventListener("open", () => socket.send(encode({ type: "discover" })));
    socket.addEventListener("message", (event) => {
      let reply: ServerMessage;
      try {
        reply = decode(event.data as string);
      } catch {
        finish(null);
        return;
      }
      finish(reply.type === "discoverAck" ? { host, port, serverName: reply.serverName, secureUrl: reply.secureUrl } : null);
    });
    socket.addEventListener("error", () => finish(null));
    socket.addEventListener("close", () => finish(null));
  });
}

/** Runs `fn` over `items` with at most `limit` in flight at once — a plain `Promise.all` would
 * open all 254 sockets of a /24 at the same time, wasteful and hard on the service worker. */
async function mapWithConcurrency<T, R>(items: T[], limit: number, fn: (item: T) => Promise<R>): Promise<R[]> {
  const results: R[] = new Array(items.length);
  let next = 0;
  async function worker() {
    while (next < items.length) {
      const index = next++;
      results[index] = await fn(items[index]);
    }
  }
  await Promise.all(Array.from({ length: Math.min(limit, items.length) }, worker));
  return results;
}
