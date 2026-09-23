import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { crx } from "@crxjs/vite-plugin";
import { manifestFor, PANEL_PAGE, type TargetBrowser } from "./manifest.config";

// https://vite.dev/config/
// `--mode firefox` (`npm run build:firefox`) builds the Firefox flavor into its own `dist-firefox/`,
// so both unpacked builds can sit side by side (P69 item 2). Any other mode is Chrome, as before.
export default defineConfig(({ mode }) => {
  const browser: TargetBrowser = mode === "firefox" ? "firefox" : "chrome";
  return {
    plugins: [react(), crx({ manifest: manifestFor(browser), browser })],
    build: {
      outDir: browser === "firefox" ? "dist-firefox" : "dist",
      // crxjs doesn't pick up `sidebar_action.default_panel` on its own (see `manifest.config.ts`).
      rollupOptions: browser === "firefox" ? { input: { panel: PANEL_PAGE } } : undefined,
    },
  };
});
