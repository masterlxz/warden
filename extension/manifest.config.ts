import { defineManifest } from "@crxjs/vite-plugin";

// Manifest V3 only (Chrome-only for this slice — Fase 8.1/8.2, see project/PHASE.md). `storage`
// persists connection settings + device id (Fase 8.1/8.2). `scripting` + `activeTab` (Fase
// 8.3-8.6, DOM tools) — deliberately NOT `host_permissions: ["<all_urls>"]`: `activeTab` only
// grants access to the current tab after a user gesture (opening the popup counts) and that
// access drops when the tab navigates to a different page. Minimal-permission trade-off chosen
// over always-on access — see `background/tools/dom_executor.ts` for the resulting error path.
export default defineManifest({
  manifest_version: 3,
  name: "Warden",
  version: "0.1.0",
  description: "Chat com o Warden a partir do navegador.",
  action: {
    default_popup: "src/popup/index.html",
  },
  background: {
    service_worker: "src/background/index.ts",
    type: "module",
  },
  permissions: ["storage", "scripting", "activeTab"],
});
