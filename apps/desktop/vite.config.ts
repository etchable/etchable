import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;

// The Tauri backend lives in ./src-tauri (a member of the repo-root cargo
// workspace). Run `pnpm tauri dev` here or from the repo root.
export default defineConfig(async () => ({
  plugins: [react(), tailwindcss()],

  // Two pages, one per window: index.html is the dashboard, app.html the
  // workbench (see tauri.conf.json app.windows).
  build: {
    rollupOptions: {
      input: {
        dashboard: new URL("./index.html", import.meta.url).pathname,
        app: new URL("./app.html", import.meta.url).pathname,
      },
    },
  },

  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      ignored: ["**/src-tauri/**", "**/target/**"],
    },
  },
}));
