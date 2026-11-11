import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
// @ts-expect-error type error without @types/node package
import process from "node:process";
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(() => ({
  plugins: [react()],

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    // 2a. `/api` is `curly serve` (ziman/src/main/ziman/server/). PROXIED, not reached by an
    //     absolute URL, because the server refuses a request whose `Origin` does not match its
    //     `Host` — these commands read and write the user's files, and a server that accepted
    //     them from any origin would hand every open tab a file-writing API. Through the proxy
    //     the call leaves this dev server's own origin and arrives same-origin.
    //
    //     **`changeOrigin` must stay false** (Vite's default, so it is simply not set): rewriting
    //     `Host` to the target would make it disagree with the browser's `Origin` and every
    //     command would be refused. Set CURLY_PORT to match a `curly serve --port`.
    proxy: {
      "/api": `http://127.0.0.1:${process.env.CURLY_PORT || 8787}`,
    },
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },
}));
