import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { resolve } from "node:path";

// Three pages: the main window, the approval window and the island. The dev server binds
// to loopback only; `tauri dev` expects it on 1420.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    host: "127.0.0.1",
    port: 1420,
    strictPort: true,
  },
  preview: {
    host: "127.0.0.1",
    port: 1421,
    strictPort: true,
  },
  build: {
    target: ["es2021", "safari15"],
    rollupOptions: {
      input: {
        main: resolve(__dirname, "index.html"),
        approval: resolve(__dirname, "approval.html"),
        island: resolve(__dirname, "island.html"),
      },
    },
  },
});
