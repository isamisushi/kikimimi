import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Mock dev server (web/mock/server.mjs) listens on this port.
// See package.json "dev" / "mock" scripts. (Not 8787: that's the real
// kikimimi-cloud server's port per fly.toml, and this proxy target must never
// collide with a real instance someone has running locally.)
const MOCK_PORT = 8788;

export default defineConfig({
  plugins: [react()],
  server: {
    port: 5173,
    proxy: {
      "/web": {
        target: `http://localhost:${MOCK_PORT}`,
        changeOrigin: true,
      },
    },
  },
  build: {
    outDir: "dist",
    sourcemap: true,
  },
});
