/**
 * `chrome.system.network` isn't in the `@types/chrome` version this project pins (`0.3.0`, which
 * ships `system.cpu`/`memory`/`storage`/`display` but not `system.network`) — augments the ambient
 * `chrome` namespace with just the one function `background/discovery.ts` needs (Fase 9.1,
 * redefined), matching the shape and Promise-overload convention `system.storage`'s declarations
 * already use in `node_modules/@types/chrome/index.d.ts`.
 */
declare namespace chrome.system.network {
  interface NetworkInterface {
    name: string;
    address: string;
    prefixLength: number;
  }

  function getNetworkInterfaces(): Promise<NetworkInterface[]>;
  function getNetworkInterfaces(callback: (networkInterfaces: NetworkInterface[]) => void): void;
}
