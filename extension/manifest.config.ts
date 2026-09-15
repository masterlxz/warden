import { defineManifest } from "@crxjs/vite-plugin";

// Manifest V3 only (Chrome-only for this slice — Fase 8.1/8.2, see project/PHASE.md). `storage`
// is the only permission this slice needs (persist connection settings + device id); no
// `host_permissions`/`scripting` yet — those arrive with the DOM tools in Fase 8.3-8.6.
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
  permissions: ["storage"],
});
