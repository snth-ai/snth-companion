import { useEffect, useMemo, useState } from "react"
import {
  Card,
  CardContent,
} from "@/components/ui/card"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Switch } from "@/components/ui/switch"
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import { Input } from "@/components/ui/input"
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet"
import {
  fetchSynthTools,
  toggleSynthTool,
  type SynthToolEntry,
} from "@/lib/api"
import { toast } from "sonner"

// SynthToolsPage — per-synth view of the LLM tools the paired synth's
// container actually loads. Replaces the old vertical 11-section
// layout with a compact full-width grid + filter chips + Sheet drawer
// for tool details. See atlas/18-companion.md Wave 13 for the design.

// Capability-based grouping — mirrors the hub admin /instances/tools
// page (openpaw-internal `tools/capabilities.go` policy).
type CategoryKey =
  | "all"
  | "mac"
  | "mobile"
  | "spotify"
  | "post_to_channel"
  | "identity"
  | "wiki"
  | "memory"
  | "media"
  | "web"
  | "self"
  | "skill"
  | "other"

const categoryOrder: Array<{ key: CategoryKey; title: string; short: string }> = [
  { key: "all", title: "All", short: "All" },
  { key: "mac", title: "Mac integration (remote_* / companion_*)", short: "Mac" },
  { key: "mobile", title: "iPhone (mobile_*)", short: "iPhone" },
  { key: "spotify", title: "Spotify", short: "Spotify" },
  { key: "post_to_channel", title: "Channel posting", short: "Channel" },
  { key: "identity", title: "Identity", short: "Identity" },
  { key: "wiki", title: "Wiki", short: "Wiki" },
  { key: "memory", title: "Memory", short: "Memory" },
  { key: "media", title: "Media (send_*, image_*, etc)", short: "Media" },
  { key: "web", title: "Web", short: "Web" },
  { key: "self", title: "Self-edit / Workspace", short: "Self-edit" },
  { key: "skill", title: "User skills", short: "Skills" },
  { key: "other", title: "Other", short: "Other" },
]

// estimateToolTokens approximates how much LLM context this tool's
// schema burns. The synth's ForLLM serializes each tool as
// `{"type":"function","function":{"name":..,"description":..,
// "parameters":..}}` — we model that envelope (~30 chars) + the
// three variable fields, then divide by ~4 chars/token (Anthropic
// + OpenAI tokenizers land near that for English/JSON).
//
// Not exact (real tokenization varies per provider and per locale),
// but it's the same chars/4 heuristic the Context tab uses for
// total-context cost — operators get a consistent picture across
// the two surfaces.
function estimateToolTokens(t: SynthToolEntry): number {
  let bytes = 30 // {"type":"function","function":{...}} envelope
  bytes += t.name.length + 12 // "name": "..."
  bytes += t.description.length + 18 // "description": "..."
  try {
    const params = typeof t.parameters === "string" ? t.parameters : JSON.stringify(t.parameters ?? {})
    bytes += params.length + 17 // "parameters": ...
  } catch {
    // ignore — parameters unparseable, skip
  }
  return Math.max(1, Math.round(bytes / 4))
}

function fmtTok(n: number): string {
  if (n < 1000) return `${n} tok`
  return `${(n / 1000).toFixed(1)}K tok`
}

function categorize(t: SynthToolEntry): CategoryKey {
  if (t.source === "skill") return "skill"
  const name = t.name
  if (name.startsWith("remote_") || name.startsWith("companion_")) return "mac"
  if (name.startsWith("mobile_")) return "mobile"
  if (name === "spotify") return "spotify"
  if (name === "post_to_channel") return "post_to_channel"
  if (name === "identity_promote") return "identity"
  if (name.startsWith("wiki_")) return "wiki"
  if (name.startsWith("memory_")) return "memory"
  if (
    name.startsWith("send_") ||
    name.startsWith("image_") ||
    name === "generate_image" ||
    name === "react" ||
    name === "tts" ||
    name === "transcribe" ||
    name === "media_download" ||
    name === "youtube_transcript" ||
    name === "dj" ||
    name === "giphy" ||
    name === "edit_message" ||
    name === "delete_message"
  ) {
    return "media"
  }
  if (name === "web_search" || name === "web_read") return "web"
  if (
    name === "read_file" ||
    name === "write_file" ||
    name === "ls" ||
    name === "find" ||
    name === "grep" ||
    name === "exec" ||
    name === "self_edit" ||
    name === "update_workspace_file" ||
    name === "publish_page" ||
    name === "unpublish_page" ||
    name === "list_pages" ||
    name === "create_skill" ||
    name === "skill_patch" ||
    name === "self_docs" ||
    name === "install_package"
  ) {
    return "self"
  }
  return "other"
}

export function SynthToolsPage() {
  const [tools, setTools] = useState<SynthToolEntry[] | null>(null)
  const [synthId, setSynthId] = useState<string>("")
  const [err, setErr] = useState<string | null>(null)
  const [filter, setFilter] = useState("")
  const [activeCat, setActiveCat] = useState<CategoryKey>("all")
  const [busyTool, setBusyTool] = useState<string | null>(null)
  const [busyGroup, setBusyGroup] = useState<boolean>(false)
  const [openTool, setOpenTool] = useState<SynthToolEntry | null>(null)

  const load = async () => {
    try {
      const d = await fetchSynthTools()
      setTools(d.tools ?? [])
      setSynthId(d.synth_id)
      setErr(null)
    } catch (e) {
      setErr(String(e))
    }
  }

  useEffect(() => {
    load()
  }, [])

  const onToggle = async (t: SynthToolEntry, next: boolean) => {
    if (t.disabled_global) return
    setBusyTool(t.name)
    try {
      await toggleSynthTool(t.name, next)
      setTools((prev) =>
        prev
          ? prev.map((x) =>
              x.name === t.name ? { ...x, disabled: next } : x,
            )
          : prev,
      )
      // Keep openTool in sync if the Sheet is showing this tool.
      setOpenTool((cur) => (cur && cur.name === t.name ? { ...cur, disabled: next } : cur))
    } catch (e) {
      toast.error("Toggle failed", { description: String(e) })
    } finally {
      setBusyTool(null)
    }
  }

  // Counts per category — drive the chip labels + summary tile.
  const countsByCat = useMemo(() => {
    const out: Partial<Record<CategoryKey, number>> = { all: tools?.length ?? 0 }
    if (tools) {
      for (const t of tools) {
        const k = categorize(t)
        out[k] = (out[k] ?? 0) + 1
      }
    }
    return out
  }, [tools])

  // Filtered view — applies search + category chip together.
  const visible = useMemo(() => {
    if (!tools) return []
    const q = filter.trim().toLowerCase()
    return tools.filter((t) => {
      if (activeCat !== "all" && categorize(t) !== activeCat) return false
      if (q) {
        if (
          !t.name.toLowerCase().includes(q) &&
          !t.description.toLowerCase().includes(q)
        ) {
          return false
        }
      }
      return true
    })
  }, [tools, filter, activeCat])

  const stats = useMemo(() => {
    if (!tools) return { total: 0, disabled: 0, locked: 0, activeTokens: 0, totalTokens: 0 }
    let disabled = 0
    let locked = 0
    let activeTokens = 0 // tokens of tools currently sent to LLM (not disabled, not locked)
    let totalTokens = 0
    for (const t of tools) {
      const tok = estimateToolTokens(t)
      totalTokens += tok
      if (t.disabled) disabled++
      if (t.disabled_global) locked++
      if (!t.disabled && !t.disabled_global) activeTokens += tok
    }
    return { total: tools.length, disabled, locked, activeTokens, totalTokens }
  }, [tools])

  // Batch toggle over the *currently visible* subset (filter + category).
  const onBatchToggle = async (next: boolean) => {
    const actionable = visible.filter(
      (t) => !t.disabled_global && t.disabled !== next,
    )
    if (actionable.length === 0) return
    setBusyGroup(true)
    let ok = 0
    let firstErr: string | null = null
    for (const t of actionable) {
      try {
        await toggleSynthTool(t.name, next)
        setTools((prev) =>
          prev
            ? prev.map((x) =>
                x.name === t.name ? { ...x, disabled: next } : x,
              )
            : prev,
        )
        ok++
      } catch (e) {
        firstErr = `${t.name}: ${String(e)}`
        break
      }
    }
    setBusyGroup(false)
    if (firstErr) {
      toast.error(`Batch stopped after ${ok} ok`, { description: firstErr })
    } else {
      toast.success(
        next ? `Disabled ${ok} tools` : `Enabled ${ok} tools`,
        { description: "Synth picks up changes on next config poll (≤2 min)." },
      )
    }
  }

  if (err && !tools) {
    return (
      <Alert variant="destructive">
        <AlertTitle>Can't load tools</AlertTitle>
        <AlertDescription>{err}</AlertDescription>
      </Alert>
    )
  }
  if (!tools) {
    return <div className="text-sm text-muted-foreground">Loading…</div>
  }

  const activeMeta = categoryOrder.find((c) => c.key === activeCat)!
  const enabledInView = visible.filter((t) => !t.disabled && !t.disabled_global).length
  const disabledInView = visible.filter((t) => t.disabled && !t.disabled_global).length
  const activeTokensInView = visible.reduce(
    (acc, t) => acc + (!t.disabled && !t.disabled_global ? estimateToolTokens(t) : 0),
    0,
  )

  return (
    <div className="w-full max-w-none space-y-4 px-4">
      <div className="flex items-baseline justify-between gap-4">
        <div>
          <h1 className="text-2xl font-semibold tracking-tight">Synth Tools</h1>
          {synthId && (
            <p className="text-xs text-muted-foreground mt-1">
              Paired with <code className="font-mono">{synthId}</code> · click any cell for description + schema
            </p>
          )}
        </div>
        <div className="flex items-center gap-3 text-sm tabular-nums">
          <span className="text-muted-foreground">Total</span>
          <span className="font-semibold text-lg">{stats.total}</span>
          <span className="text-muted-foreground ml-3">Disabled</span>
          <span className="font-semibold text-lg text-destructive">{stats.disabled}</span>
          {stats.locked > 0 && (
            <>
              <span className="text-muted-foreground ml-3">Locked</span>
              <span className="font-semibold text-lg">{stats.locked}</span>
            </>
          )}
          <span
            className="text-muted-foreground ml-3"
            title={`Active schema tokens (per cache-prefix write). Total inc. disabled: ${fmtTok(stats.totalTokens)}. Estimated via chars/4 heuristic — see Context tab for full breakdown.`}
          >
            Schema
          </span>
          <span className="font-semibold text-lg">{fmtTok(stats.activeTokens)}</span>
        </div>
      </div>

      <Input
        placeholder="Filter by name or description…"
        value={filter}
        onChange={(e) => setFilter(e.target.value)}
        className="max-w-md"
      />

      <div className="flex flex-wrap items-center gap-1.5">
        {categoryOrder.map(({ key, short }) => {
          if (key !== "all" && (countsByCat[key] ?? 0) === 0) return null
          const isActive = activeCat === key
          const count = countsByCat[key] ?? 0
          return (
            <button
              key={key}
              onClick={() => setActiveCat(key)}
              className={`px-2.5 py-1 text-xs rounded-md border transition-colors ${
                isActive
                  ? "bg-primary text-primary-foreground border-primary"
                  : "bg-muted hover:bg-accent border-transparent"
              }`}
            >
              {short} <span className={isActive ? "opacity-80" : "text-muted-foreground"}>· {count}</span>
            </button>
          )
        })}
        {activeCat !== "all" && (
          <div className="ml-auto flex items-center gap-2">
            <span className="text-xs text-muted-foreground tabular-nums">
              {enabledInView} on · {disabledInView} off · {fmtTok(activeTokensInView)}
            </span>
            <Button
              variant="destructive"
              size="sm"
              disabled={busyGroup || enabledInView === 0}
              onClick={() => {
                if (
                  window.confirm(
                    `Disable all ${enabledInView} enabled tools in "${activeMeta.title}" on this synth?`,
                  )
                ) {
                  void onBatchToggle(true)
                }
              }}
            >
              Disable all
            </Button>
            <Button
              variant="default"
              size="sm"
              disabled={busyGroup || disabledInView === 0}
              onClick={() => void onBatchToggle(false)}
            >
              Enable all
            </Button>
          </div>
        )}
      </div>

      {visible.length === 0 && (
        <Alert>
          <AlertTitle>No tools match</AlertTitle>
          <AlertDescription>
            {tools.length === 0
              ? "The synth hasn't reported its catalog. This usually means the synth is restarting — give it ~10 seconds and reload."
              : "Try a different filter or category."}
          </AlertDescription>
        </Alert>
      )}

      <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 gap-2">
        {visible.map((t) => (
          <ToolCell
            key={t.name}
            tool={t}
            busy={busyTool === t.name || busyGroup}
            onToggle={(next) => onToggle(t, next)}
            onOpen={() => setOpenTool(t)}
          />
        ))}
      </div>

      <Sheet open={!!openTool} onOpenChange={(o) => !o && setOpenTool(null)}>
        <SheetContent>
          {openTool && (
            <>
              <SheetHeader>
                <SheetTitle>{openTool.name}</SheetTitle>
                <SheetDescription>
                  <div className="flex flex-wrap items-center gap-2 mt-1">
                    <Badge variant="outline" className="text-xs">{openTool.source}</Badge>
                    {openTool.scope && (
                      <Badge variant="outline" className="text-xs">scope: {openTool.scope}</Badge>
                    )}
                    {openTool.disabled_global && (
                      <Badge variant="destructive">locked by operator</Badge>
                    )}
                    {openTool.active_variant && (
                      <Badge variant="secondary">variant: {openTool.active_variant}</Badge>
                    )}
                  </div>
                </SheetDescription>
              </SheetHeader>

              <div className="flex items-center justify-between gap-3 border rounded-md px-3 py-2 bg-muted/40">
                <div className="text-sm">
                  {openTool.disabled
                    ? "Disabled on this synth"
                    : openTool.disabled_global
                      ? "Globally disabled by operator"
                      : "Enabled"}
                </div>
                <Switch
                  checked={!openTool.disabled}
                  onCheckedChange={(checked) => onToggle(openTool, !checked)}
                  disabled={openTool.disabled_global || busyTool === openTool.name}
                  aria-label={`Toggle ${openTool.name}`}
                />
              </div>

              <div className="flex-1 overflow-y-auto space-y-4">
                <div>
                  <h3 className="text-xs uppercase tracking-wide text-muted-foreground font-medium mb-1.5">Description</h3>
                  <p className="text-sm leading-relaxed whitespace-pre-wrap">{openTool.description}</p>
                </div>
                <div>
                  <h3 className="text-xs uppercase tracking-wide text-muted-foreground font-medium mb-1.5">Parameters (JSON Schema)</h3>
                  <pre className="bg-muted/50 rounded p-2 overflow-x-auto text-[11px] leading-snug">
                    {JSON.stringify(openTool.parameters, null, 2)}
                  </pre>
                </div>
              </div>
            </>
          )}
        </SheetContent>
      </Sheet>
    </div>
  )
}

function ToolCell({
  tool,
  busy,
  onToggle,
  onOpen,
}: {
  tool: SynthToolEntry
  busy: boolean
  onToggle: (next: boolean) => void
  onOpen: () => void
}) {
  const dimmed = tool.disabled || tool.disabled_global
  const cat = categorize(tool)
  const catShort = categoryOrder.find((c) => c.key === cat)?.short ?? cat
  const tokens = estimateToolTokens(tool)
  return (
    <Card
      className={`relative cursor-pointer transition-colors hover:bg-accent/40 ${dimmed ? "opacity-60" : ""}`}
      onClick={onOpen}
    >
      <CardContent className="p-3 flex flex-col gap-1.5">
        <div className="flex items-start justify-between gap-2">
          <span className="font-mono text-xs font-semibold leading-tight break-all">{tool.name}</span>
          <div onClick={(e) => e.stopPropagation()}>
            <Switch
              checked={!tool.disabled}
              onCheckedChange={(checked) => onToggle(!checked)}
              disabled={tool.disabled_global || busy}
              aria-label={`Toggle ${tool.name}`}
            />
          </div>
        </div>
        <div className="flex items-center justify-between gap-1.5 text-[10px] text-muted-foreground">
          <div className="flex items-center gap-1.5">
            <span className="font-medium">{catShort}</span>
            {tool.disabled_global && (
              <Badge variant="destructive" className="text-[9px] px-1 py-0">locked</Badge>
            )}
            {tool.active_variant && (
              <Badge variant="secondary" className="text-[9px] px-1 py-0">{tool.active_variant}</Badge>
            )}
          </div>
          <span
            className={`tabular-nums font-mono ${dimmed ? "" : "text-muted-foreground"}`}
            title={`~${tokens} schema tokens in cache prefix per LLM call (chars/4 estimate)`}
          >
            {fmtTok(tokens)}
          </span>
        </div>
      </CardContent>
    </Card>
  )
}
