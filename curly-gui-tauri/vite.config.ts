import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
// @ts-expect-error type error without @types/node package
import process from "node:process";

// https://vite.dev/config/
export default defineConfig(() => ({
  plugins: [react()],

  server: {
    // **A fixed port, kept after the Tauri shell was deleted.** It was `strictPort` because
    // `tauri dev` expected to find the dev server exactly here; it stays because the alternative
    // is Vite silently picking another port while `curly serve`'s origin check is configured for
    // this one -- a failure that looks like the API being broken rather than like the wrong port.
    port: 1420,
    strictPort: true,
    // `/api` is `curly serve` (ziman/src/main/ziman/server/). PROXIED, not reached by an absolute
    // URL, because the server refuses a request whose `Origin` does not match its `Host` — these
    // commands read and write the user's files, and a server that accepted them from any origin
    // would hand every open tab a file-writing API. Through the proxy the call leaves this dev
    // server's own origin and arrives same-origin.
    //
    // **`changeOrigin` must stay false** (Vite's default, so it is simply not set): rewriting
    // `Host` to the target would make it disagree with the browser's `Origin` and every command
    // would be refused. Set CURLY_PORT to match a `curly serve --port`.
    proxy: {
      "/api": `http://127.0.0.1:${process.env.CURLY_PORT || 8787}`,
    },
  },
}));
