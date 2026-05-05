import { useEffect, useMemo, useState } from "react"
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { Button } from "@/components/ui/button"
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import { Input } from "@/components/ui/input"
import {
  fetchMiniApps,
  miniAppAsk,
  miniAppRunURL,
  synthFetch,
  type MiniAppListEntry,
  type MiniAppManifest,
} from "@/lib/api"
import { toast } from "sonner"

// Apps — synth-authored mini-apps (Wave 9, v0.4.39+).
//
// The grid lists every mini-app the bound synth has authored. Click a
// tile and the iframe takes over the page; back-button (top-left)
// closes it. The iframe runs the bootstrap HTML the synth serves at
// /app/mini/<slug>; companion proxies that through hub →
// /api/hub/mini-app/<slug>. The bootstrap loads the $mini bridge that
// posts messages to window.parent — we handle them here and forward to
// the synth via /api/hub/synth-fetch (whitelisted /api/* paths only).
//
// Same iframe sandbox the Telegram side uses: sandbox="allow-scripts"
// (no same-origin, no forms, no popups). The bridge is the only path
// out of the iframe.

type RequestEvent = {
  __mini_request: true
  slug: string
  id: string
  method: string
  args: Record<string, unknown>
}

type Reply = {
  __mini_response: true
  id: string
  data?: unknown
  error?: string
}

const STORAGE_PREFIX = "mini:"

function storageKey(slug: string, key: string) {
  return `${STORAGE_PREFIX}${slug}:${key}`
}

function tileBackground(m: MiniAppManifest) {
  // Fall back to a slate gradient if the synth didn't pick a tint.
  const c = m.tint || "#475569"
  return {
    background: `linear-gradient(135deg, ${c} 0%, color-mix(in srgb, ${c} 70%, #000) 100%)`,
  }
}

export function AppsPage() {
  const [apps, setApps] = useState<MiniAppListEntry[] | null>(null)
  const [err, setErr] = useState<string | null>(null)
  const [filter, setFilter] = useState("")
  const [openSlug, setOpenSlug] = useState<string | null>(null)

  const load = async () => {
    try {
      const d = await fetchMiniApps()
      setApps(d.apps ?? [])
      setErr(null)
    } catch (e) {
      setErr(String((e as Error).message ?? e))
    }
  }

  useEffect(() => {
    void load()
  }, [])

  // $mini bridge — listens for postMessage from the iframe.
  useEffect(() => {
    const handler = async (ev: MessageEvent) => {
      const msg = ev.data as RequestEvent | undefined
      if (!msg || !msg.__mini_request) return
      if (!openSlug || msg.slug !== openSlug) return
      const reply: Reply = { __mini_response: true, id: msg.id }
      try {
        reply.data = await dispatchMiniRequest(msg.method, msg.args || {}, openSlug)
      } catch (e) {
        reply.error = (e as Error).message ?? String(e)
      }
      try {
        ;(ev.source as WindowProxy | null)?.postMessage(reply, "*")
      } catch {
        // iframe gone.
      }
    }
    window.addEventListener("message", handler)
    return () => window.removeEventListener("message", handler)
  }, [openSlug])

  const filtered = useMemo(() => {
    if (!apps) return []
    const q = filter.trim().toLowerCase()
    if (!q) return apps
    return apps.filter(
      (a) =>
        a.manifest.name?.toLowerCase().includes(q) ||
        a.manifest.slug?.toLowerCase().includes(q) ||
        a.manifest.description?.toLowerCase().includes(q),
    )
  }, [apps, filter])

  if (openSlug) {
    return <MiniAppFrame slug={openSlug} onClose={() => setOpenSlug(null)} />
  }

  return (
    <div className="space-y-4">
      <div className="flex items-end justify-between gap-4 flex-wrap">
        <div>
          <h1 className="text-2xl font-semibold tracking-tight">Apps</h1>
          <p className="text-sm text-muted-foreground mt-1">
            Mini-apps your synth has authored. Same surface as the Telegram
            home screen, running in a sandboxed iframe.
          </p>
        </div>
        <Input
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          placeholder="Filter…"
          className="max-w-xs"
        />
      </div>

      {err && (
        <Alert variant="destructive">
          <AlertTitle>Couldn't load mini-apps</AlertTitle>
          <AlertDescription>{err}</AlertDescription>
        </Alert>
      )}

      {apps !== null && apps.length === 0 && (
        <Card>
          <CardContent className="py-10 text-center text-sm text-muted-foreground">
            Your synth hasn't authored any mini-apps yet. They show up here
            automatically when she creates one with the <code>create_mini_app</code>{" "}
            tool.
          </CardContent>
        </Card>
      )}

      {filtered.length > 0 && (
        <div className="grid grid-cols-2 md:grid-cols-3 lg:grid-cols-4 gap-3">
          {filtered.map((entry) => {
            const m = entry.manifest
            return (
              <button
                key={m.slug}
                onClick={() => setOpenSlug(m.slug)}
                className="text-left transition-transform hover:scale-[1.02] focus:outline-none focus:ring-2 focus:ring-primary rounded-xl"
              >
                <Card className="overflow-hidden">
                  <div
                    className="aspect-[3/2] flex items-center justify-center text-5xl"
                    style={tileBackground(m)}
                  >
                    <span className="drop-shadow-sm">{m.icon || "•"}</span>
                  </div>
                  <CardHeader className="pb-1 pt-3">
                    <CardTitle className="text-sm">{m.name || m.slug}</CardTitle>
                  </CardHeader>
                  <CardContent className="pt-0 pb-3">
                    {m.description ? (
                      <p className="text-xs text-muted-foreground line-clamp-2">
                        {m.description}
                      </p>
                    ) : (
                      <p className="text-xs text-muted-foreground italic">
                        no description
                      </p>
                    )}
                    {entry.manifest_error && (
                      <p className="text-xs text-red-400 mt-2">
                        manifest error: {entry.manifest_error}
                      </p>
                    )}
                  </CardContent>
                </Card>
              </button>
            )
          })}
        </div>
      )}
    </div>
  )
}

function MiniAppFrame({ slug, onClose }: { slug: string; onClose: () => void }) {
  const url = useMemo(() => miniAppRunURL(slug), [slug])
  return (
    <div className="fixed inset-0 z-30 bg-background flex flex-col">
      <header className="flex items-center gap-3 px-4 py-2 border-b border-border bg-card/40">
        <Button variant="ghost" size="sm" onClick={onClose}>
          ← Back
        </Button>
        <div className="text-sm text-muted-foreground font-mono">{slug}</div>
      </header>
      <iframe
        title={slug}
        src={url}
        // allow-scripts: required for the bootstrap to run.
        // Intentionally NOT allow-same-origin: the bridge is the
        // sanctioned data path; same-origin would let the iframe touch
        // companion JS state directly.
        sandbox="allow-scripts"
        className="flex-1 w-full bg-white"
      />
    </div>
  )
}

// dispatchMiniRequest implements the same surface webapp/js/mini_apps.js
// exposes inside Telegram. Methods supported here:
//   fetch         — proxy through synth-fetch (whitelisted /api/*)
//   askSynth      — LLM via /api/mini-apps/ask
//   storage.{get,set,remove,keys}  — namespaced localStorage
//   toast         — sonner
//   haptic        — no-op (no Apple haptic API in WebView)
//   close         — return ok (parent closes iframe via state)
//   nav.push/pop  — depth tracked client-side; not surfaced yet
async function dispatchMiniRequest(
  method: string,
  args: Record<string, unknown>,
  slug: string,
): Promise<unknown> {
  switch (method) {
    case "fetch": {
      const path = String(args.path || "")
      if (!path.startsWith("/api/")) {
        throw new Error("path must start with /api/")
      }
      const opts = (args.opts as Record<string, unknown> | undefined) || {}
      const resp = await synthFetch(path, String(opts.method || "GET"), opts.body)
      return { status: resp.status, ok: resp.ok, body: resp.body }
    }
    case "askSynth": {
      const prompt = String(args.prompt || "")
      if (!prompt) throw new Error("prompt is required")
      const fmt = (args.options as { format?: string } | undefined)?.format
      const r = await miniAppAsk(slug, prompt, fmt)
      if (r.error) throw new Error(r.error)
      return { text: r.text, tokens: r.tokens ?? 0, format: r.format ?? "text" }
    }
    case "storage.get": {
      const k = String(args.key || "")
      const raw = localStorage.getItem(storageKey(slug, k))
      if (raw == null) return null
      try {
        return JSON.parse(raw)
      } catch {
        return raw
      }
    }
    case "storage.set": {
      const k = String(args.key || "")
      localStorage.setItem(storageKey(slug, k), JSON.stringify(args.value))
      return { ok: true }
    }
    case "storage.remove": {
      const k = String(args.key || "")
      localStorage.removeItem(storageKey(slug, k))
      return { ok: true }
    }
    case "storage.keys": {
      const prefix = `${STORAGE_PREFIX}${slug}:`
      const out: string[] = []
      for (let i = 0; i < localStorage.length; i++) {
        const k = localStorage.key(i)
        if (k && k.startsWith(prefix)) out.push(k.slice(prefix.length))
      }
      return out
    }
    case "toast": {
      const kind = (args.kind as string) || "info"
      const msg = String(args.msg || "")
      if (kind === "error") toast.error(msg)
      else if (kind === "success") toast.success(msg)
      else toast(msg)
      return { ok: true }
    }
    case "haptic":
      return { ok: false } // no native haptic in webview
    case "close":
      // Iframe asked to close; the host page handles the actual close
      // via its own back-button + onClose. Just ack.
      return { ok: true }
    case "nav.push":
    case "nav.pop":
      // Multi-screen navigation not implemented in this MVP — most
      // mini-apps are single-screen. Ack so apps that try don't break.
      return { ok: true, depth: 0 }
    default:
      throw new Error(`unknown $mini method: ${method}`)
  }
}
