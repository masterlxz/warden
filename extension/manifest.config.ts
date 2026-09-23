import { defineManifest } from "@crxjs/vite-plugin";

export type TargetBrowser = "chrome" | "firefox";

// The chat panel page, shared by both targets — Chrome mounts it as `side_panel`, Firefox as
// `sidebar_action`. `vite.config.ts` also needs it: crxjs only bundles HTML it finds under keys it
// knows (`side_panel` yes, `sidebar_action` no), so the Firefox build adds it as an explicit input.
export const PANEL_PAGE = "src/sidepanel/index.html";

// Manifest V3 on both targets (Chrome since Fase 8.1/8.2, Firefox since P69 item 2 — one codebase,
// two builds: `npm run build` → `dist/`, `npm run build:firefox` → `dist-firefox/`).
// `storage` persists connection settings + device id (Fase 8.1/8.2). `scripting` + `activeTab` (Fase
// 8.3-8.6, DOM tools) — deliberately NOT `host_permissions: ["<all_urls>"]`: `activeTab` only
// grants access to the current tab after a user gesture (opening the side panel counts) and that
// access drops when the tab navigates to a different page. Minimal-permission trade-off chosen
// over always-on access — see `background/tools/dom_executor.ts` for the resulting error path.
// `tabGroups` (P69) — lets the DOM tools act on more than one tab at a time via a dedicated
// "Warden" tab group (`background/tab_group.ts`), same idea as Claude in Chrome's own tab group.
// Confirmed with the user before building this: the user adds each tab one at a time from the
// side panel, and that click is itself the gesture that grants `activeTab` for that tab — this
// permission only lets the extension *organize* tabs visually, it grants no content access by
// itself. No broadening of `activeTab`'s reach, no `tabs`/`host_permissions` added.
const SHARED_PERMISSIONS = ["storage", "scripting", "activeTab", "tabGroups"];

// Chrome-only permissions, dropped from the Firefox build (no such API there):
// `sidePanel` — chat UI is a docked side panel, not a popup that closes on blur (see
// `background/platform.ts`'s `setPanelBehavior` call for the click-to-open wiring).
// `system.network` (Fase 9.1, redefined) — `background/discovery.ts` reads
// `chrome.system.network.getNetworkInterfaces()` to learn this device's local subnet, so it can
// sweep the LAN for a warden-server hub instead of the user typing an IP by hand. Only reads
// interface addresses, never touches page content.
const CHROME_ONLY_PERMISSIONS = ["sidePanel", "system.network"];

const BASE = {
  manifest_version: 3,
  name: "Warden",
  version: "0.1.0",
  description: "Chat com o Warden a partir do navegador.",
  // No `default_popup` on purpose — the icon opens the panel (`background/platform.ts`). Firefox
  // needs the key to show a toolbar button at all; Chrome shows one either way.
  action: { default_title: "Warden" },
};

export function manifestFor(target: TargetBrowser) {
  if (target === "firefox") {
    return defineManifest({
      ...BASE,
      // Firefox has no MV3 service workers — crxjs's `browser: "firefox"` emits this as an event
      // page (`background.scripts`). The page stays alive while the sidebar is open (an open view
      // keeps it from unloading); once closed it can be suspended when idle, dropping the
      // WebSocket — the same accepted gap as an evicted Chrome service worker (see `index.ts`).
      background: { scripts: ["src/background/index.ts"] },
      permissions: SHARED_PERMISSIONS,
      // `sidebar_action` isn't in crxjs's manifest type (Chrome has no such key) — added below.
      // 140: `tabGroups` (what the multi-tab tools rely on) landed in 139, and 140 is the first
      // version that knows `data_collection_permissions` — which AMO now requires. Also an ESR.
      browser_specific_settings: {
        gecko: {
          id: "warden@warden.local",
          strict_min_version: "140.0",
          data_collection_permissions: { required: ["none"] },
        },
      },
      ...{ sidebar_action: { default_panel: PANEL_PAGE, default_title: "Warden" } },
    });
  }
  return defineManifest({
    ...BASE,
    side_panel: {
      default_path: PANEL_PAGE,
    },
    background: {
      service_worker: "src/background/index.ts",
      type: "module",
    },
    permissions: [...SHARED_PERMISSIONS, ...CHROME_ONLY_PERMISSIONS],
  });
}
