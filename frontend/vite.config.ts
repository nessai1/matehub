import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

// In production the frontend is served by hub at the same origin, so it
// hits /api/hub, /api/chat, /api/video directly through Caddy. In dev,
// `vite dev` is on :3000 and the Rust services are on :3002/:3003/:4000
// -- the proxy below makes the same relative URLs work locally.
//
// `ws: true` is required for the gateway/presence WebSockets; without it
// Vite proxies the upgrade handshake but not the actual frame stream.
//
// `rewrite` strips the /api/<svc> prefix the same way Caddy's
// handle_path does in prod, so the upstream sees /v1/... and /ws/...
// regardless of how it was reached.

const apiProxy = (target: string, prefix: string) => ({
  target,
  changeOrigin: true,
  ws: true,
  rewrite: (p: string) => p.replace(prefix, ""),
});

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    tsconfigPaths: true,
  },
  server: {
    port: 3000,
    strictPort: false,
    proxy: {
      "/api/hub": apiProxy("http://localhost:3002", "/api/hub"),
      "/api/chat": apiProxy("http://localhost:3003", "/api/chat"),
      "/api/video": apiProxy("http://localhost:4000", "/api/video"),
      // Runtime config endpoint also lives on hub.
      "/config.json": "http://localhost:3002",
    },
  },
  build: {
    outDir: "dist",
    sourcemap: true,
  },
});
