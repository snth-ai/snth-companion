import path from "node:path"
import fs from "node:fs"
import os from "node:os"
import { defineConfig } from "vite"
import react from "@vitejs/plugin-react"
import tailwindcss from "@tailwindcss/vite"

// Built artifacts land in ../internal/ui/dist so Go's embed.FS picks
// them up directly without a copy step. base="./" so index.html loads
// assets via relative paths — works at the companion's random port
// and at any future sub-path.
//
// Dev proxy forwards non-UI requests (/api/*, /health, /channels/save,
// /login/*, /keys/*, etc.) to the running companion process. Port is
// read from ~/Library/Application Support/snth-companion/lock.json so
// hot-reload works without hand-editing vite.config each launch.
// COMPANION_DEV_PORT env var overrides.

function resolveCompanionDevURL(): string {
  const envPort = process.env.COMPANION_DEV_PORT
  if (envPort) return `http://127.0.0.1:${envPort}`
  try {
    const lockPath = path.join(
      os.homedir(),
      "Library/Application Support/snth-companion/lock.json",
    )
    const raw = fs.readFileSync(lockPath, "utf-8")
    const lock = JSON.parse(raw)
    if (lock.ui_url) return lock.ui_url as string
  } catch {
    // no companion running — proxy target stays the default
  }
  return "http://127.0.0.1:60276"
}

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "src"),
    },
  },
  base: "./",
  build: {
    outDir: path.resolve(__dirname, "../internal/ui/dist"),
    emptyOutDir: true,
  },
  server: {
    port: 5173,
    proxy: {
      "/api": { target: resolveCompanionDevURL(), changeOrigin: true },
      "/health": { target: resolveCompanionDevURL(), changeOrigin: true },
      "/pair": { target: resolveCompanionDevURL(), changeOrigin: true },
      "/unpair": { target: resolveCompanionDevURL(), changeOrigin: true },
      "/channels": { target: resolveCompanionDevURL(), changeOrigin: true },
      "/keys/save": { target: resolveCompanionDevURL(), changeOrigin: true },
      "/login": { target: resolveCompanionDevURL(), changeOrigin: true },
      "/sandbox": { target: resolveCompanionDevURL(), changeOrigin: true },
    },
  },
})
