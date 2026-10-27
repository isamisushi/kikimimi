import { defineConfig, loadEnv } from "vite";
import react from "@vitejs/plugin-react";
export default defineConfig(({ mode }) => ({
  plugins: [react()],
  server: {
    host: "127.0.0.1",
    port: 5174,
    proxy: { "/web": { target: loadEnv(mode, ".", "KIKIMIMI_").KIKIMIMI_API_URL || "http://127.0.0.1:8789", changeOrigin: true } },
  },
  build: { outDir: "dist", sourcemap: true },
}));
