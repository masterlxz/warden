import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// https://vite.dev/config/
// The build (`dist/`) is compiled into the hub binary (`crates/warden-server/src/web_ui.rs`) and
// served on the hub's own port, so the page and the WebSocket share one origin. For `npm run dev`,
// point the page at a running hub with `VITE_HUB_URL=ws://127.0.0.1:7420 npm run dev`.
export default defineConfig({
  plugins: [react()],
});
