import { defineManifest } from "@crxjs/vite-plugin";

// Manifest V3 only (Chrome-only for this slice — Fase 8.1/8.2, see project/PHASE.md). `storage`
// persists connection settings + device id (Fase 8.1/8.2). `scripting` + `activeTab` (Fase
// 8.3-8.6, DOM tools) — deliberately NOT `host_permissions: ["<all_urls>"]`: `activeTab` only
// grants access to the current tab after a user gesture (opening the side panel counts) and that
// access drops when the tab navigates to a different page. Minimal-permission trade-off chosen
// over always-on access — see `background/tools/dom_executor.ts` for the resulting error path.
// `sidePanel` — chat UI is a docked side panel, not a popup that closes on blur (see
// `background/index.ts`'s `setPanelBehavior` call for the click-to-open wiring).
// `system.network` (Fase 9.1, redefined) — `background/discovery.ts` reads
// `chrome.system.network.getNetworkInterfaces()` to learn this device's local subnet, so it can
// sweep the LAN for a warden-server hub instead of the user typing an IP by hand. Only reads
// interface addresses, never touches page content.
// `tabGroups` (P69) — lets the DOM tools act on more than one tab at a time via a dedicated
// "Warden" tab group (`background/tab_group.ts`), same idea as Claude in Chrome's own tab group.
// Confirmed with the user before building this: the user adds each tab one at a time from the
// side panel, and that click is itself the gesture that grants `activeTab` for that tab — this
// permission only lets the extension *organize* tabs visually, it grants no content access by
// itself. No broadening of `activeTab`'s reach, no `tabs`/`host_permissions` added.
export default defineManifest({
  manifest_version: 3,
  name: "Warden",
  version: "0.1.0",
  description: "Chat com o Warden a partir do navegador.",
  side_panel: {
    default_path: "src/sidepanel/index.html",
  },
  background: {
    service_worker: "src/background/index.ts",
    type: "module",
  },
  permissions: ["storage", "scripting", "activeTab", "sidePanel", "system.network", "tabGroups"],
});
