import { useEffect, useState, useMemo } from "react"
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Switch } from "@/components/ui/switch"
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import { Input } from "@/components/ui/input"
import {
  fetchSynthTools,
  toggleSynthTool,
  type SynthToolEntry,
} from "@/lib/api"
import { toast } from "sonner"

// SynthToolsPage — per-synth view of the LLM tools the paired synth's
// container actually loads (memory, wiki, schedule, …). Differs from
// the existing /tools page which lists COMPANION-side tools (bash,
// fs, calendar — running on this Mac).
//
// The user can toggle individual tools off for THIS synth only.
// Globally-disabled tools (operator-controlled) show a locked
// indicator and the toggle is greyed out.

// Capability-based grouping — mirrors the hub admin /instances/tools
// page (openpaw-internal `tools/capabilities.go` policy). Same labels
// as hub so an operator switching between the two surfaces sees the
// identical bucket structure.
const groupOrder: Array<{ key: string; title: string }> = [
  { key: "mac", title: "Mac integration (remote_* / companion_*)" },
  { key: "mobile", title: "iPhone (mobile_*)" },
  { key: "spotify", title: "Spotify" },
  { key: "post_to_channel", title: "Channel posting" },
  { key: "identity", title: "Identity" },
  { key: "wiki", title: "Wiki" },
  { key: "memory", title: "Memory" },
  { key: "media", title: "Media (send_*, image_*, etc)" },
  { key: "web", title: "Web" },
  { key: "self", title: "Self-edit / Workspace" },
  { key: "skill", title: "User skills" },
  { key: "other", title: "Other" },
]

function groupKey(t: SynthToolEntry): string {
  // User skills override category — they're authored separately and
  // operators usually want to see them as a distinct group regardless
  // of what their tool name suggests.
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
  const [busyTool, setBusyTool] = useState<string | null>(null)

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
      // Optimistic update — the synth's poller picks the change up
      // within ~2 min, but the hub-side row is already correct.
      setTools((prev) =>
        prev
          ? prev.map((x) =>
              x.name === t.name ? { ...x, disabled: next } : x,
            )
          : prev,
      )
      toast.success(
        next ? `${t.name} disabled` : `${t.name} enabled`,
        {
          description: "Synth picks up the change on its next config poll (≤2 min).",
        },
      )
    } catch (e) {
      toast.error("Toggle failed", { description: String(e) })
    } finally {
      setBusyTool(null)
    }
  }

  // Group-level batch toggle. Loops toggleSynthTool one-by-one through
  // each non-locked tool in the group. Sequential rather than parallel
  // so the hub's audit log keeps a clean ordering + we surface partial
  // failures cleanly (a row that errors stops the rest).
  const [busyGroup, setBusyGroup] = useState<string | null>(null)
  const onBatchToggle = async (
    groupKeyVal: string,
    rows: SynthToolEntry[],
    next: boolean,
  ) => {
    const actionable = rows.filter((t) => !t.disabled_global && t.disabled !== next)
    if (actionable.length === 0) return
    setBusyGroup(groupKeyVal)
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
    setBusyGroup(null)
    if (firstErr) {
      toast.error(`Group toggle stopped after ${ok} ok`, { description: firstErr })
    } else {
      toast.success(
        next
          ? `Disabled ${ok} tools in group`
          : `Enabled ${ok} tools in group`,
        { description: "Synth picks up changes on next config poll (≤2 min)." },
      )
    }
  }

  const filtered = useMemo(() => {
    if (!tools) return []
    const q = filter.trim().toLowerCase()
    if (!q) return tools
    return tools.filter(
      (t) =>
        t.name.toLowerCase().includes(q) ||
        t.description.toLowerCase().includes(q),
    )
  }, [tools, filter])

  const grouped = useMemo(() => {
    const out: Record<string, SynthToolEntry[]> = {}
    for (const t of filtered) {
      const k = groupKey(t)
      ;(out[k] ??= []).push(t)
    }
    return out
  }, [filtered])

  const stats = useMemo(() => {
    if (!tools) return { total: 0, disabled: 0, locked: 0 }
    let disabled = 0
    let locked = 0
    for (const t of tools) {
      if (t.disabled) disabled++
      if (t.disabled_global) locked++
    }
    return { total: tools.length, disabled, locked }
  }, [tools])

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

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-2xl font-semibold tracking-tight">Synth Tools</h1>
        <p className="text-sm text-muted-foreground mt-1">
          The LLM tools the paired synth's container loads. Toggle a tool off
          to hide it from this synth's prompt — saves context + prevents the
          model from calling it. Operator-locked (globally disabled) tools
          are shown but can't be re-enabled here.
        </p>
        {synthId && (
          <p className="text-xs text-muted-foreground mt-2">
            Paired with <code className="font-mono">{synthId}</code>
          </p>
        )}
      </div>

      <div className="grid grid-cols-3 gap-3">
        <SummaryTile label="Tools loaded" value={stats.total} />
        <SummaryTile
          label="Disabled (effective)"
          value={stats.disabled}
          danger={stats.disabled > 0}
        />
        <SummaryTile label="Locked by operator" value={stats.locked} />
      </div>

      <Input
        placeholder="Filter by name or description…"
        value={filter}
        onChange={(e) => setFilter(e.target.value)}
        className="max-w-md"
      />

      {tools.length === 0 && (
        <Alert>
          <AlertTitle>No tools yet</AlertTitle>
          <AlertDescription>
            The synth hasn't reported its catalog. This usually means the
            synth is restarting — give it ~10 seconds and reload.
          </AlertDescription>
        </Alert>
      )}

      {groupOrder.map(({ key, title }) => {
        const rows = grouped[key]
        if (!rows || rows.length === 0) return null
        const enabledCount = rows.filter(
          (t) => !t.disabled && !t.disabled_global,
        ).length
        const disabledHereCount = rows.filter(
          (t) => t.disabled && !t.disabled_global,
        ).length
        const lockedCount = rows.filter((t) => t.disabled_global).length
        const groupBusy = busyGroup === key
        return (
          <div key={key} className="space-y-3">
            <div className="flex items-center justify-between gap-3 rounded-md border bg-muted/40 px-3 py-2">
              <div>
                <h2 className="text-sm font-semibold tracking-tight text-foreground">
                  {title}
                </h2>
                <p className="text-xs text-muted-foreground mt-0.5 tabular-nums">
                  {enabledCount} enabled · {disabledHereCount} off · {lockedCount > 0 ? `${lockedCount} locked · ` : ""}{rows.length} total
                </p>
              </div>
              <div className="flex items-center gap-2 shrink-0">
                <Button
                  variant="destructive"
                  size="sm"
                  disabled={groupBusy || enabledCount === 0}
                  onClick={() => {
                    if (
                      window.confirm(
                        `Disable all ${enabledCount} enabled tools in "${title}" on this synth?`,
                      )
                    ) {
                      void onBatchToggle(key, rows, true)
                    }
                  }}
                >
                  Disable all
                </Button>
                <Button
                  variant="default"
                  size="sm"
                  disabled={groupBusy || disabledHereCount === 0}
                  onClick={() => void onBatchToggle(key, rows, false)}
                >
                  Enable all
                </Button>
              </div>
            </div>
            <div className="grid grid-cols-1 gap-3">
              {rows.map((t) => (
                <ToolRow
                  key={t.name}
                  tool={t}
                  busy={busyTool === t.name || groupBusy}
                  onToggle={(next) => onToggle(t, next)}
                />
              ))}
            </div>
          </div>
        )
      })}
    </div>
  )
}

function ToolRow({
  tool,
  busy,
  onToggle,
}: {
  tool: SynthToolEntry
  busy: boolean
  onToggle: (next: boolean) => void
}) {
  const [showSchema, setShowSchema] = useState(false)
  const dimmed = tool.disabled || tool.disabled_global

  return (
    <Card className={dimmed ? "opacity-70" : ""}>
      <CardHeader className="pb-3">
        <CardTitle className="flex items-center justify-between gap-3 text-sm font-mono">
          <span className="truncate">{tool.name}</span>
          <div className="flex items-center gap-2 shrink-0">
            {tool.disabled_global && (
              <Badge variant="destructive">locked by operator</Badge>
            )}
            {tool.active_variant && (
              <Badge variant="secondary">variant: {tool.active_variant}</Badge>
            )}
            <Badge variant="outline" className="text-xs">
              {tool.source}
            </Badge>
            <Switch
              checked={!tool.disabled}
              onCheckedChange={(checked) => onToggle(!checked)}
              disabled={tool.disabled_global || busy}
              aria-label={`Toggle ${tool.name}`}
            />
          </div>
        </CardTitle>
      </CardHeader>
      <CardContent className="text-xs text-muted-foreground space-y-2">
        <p className="leading-relaxed">{tool.description}</p>
        <div className="flex items-center gap-3 pt-1">
          <Button
            variant="ghost"
            size="sm"
            className="h-6 px-2 text-xs"
            onClick={() => setShowSchema((v) => !v)}
          >
            {showSchema ? "Hide" : "Show"} schema
          </Button>
          {tool.scope && (
            <span className="text-xs">
              scope: <code className="font-mono">{tool.scope}</code>
            </span>
          )}
        </div>
        {showSchema && (
          <pre className="bg-muted/50 rounded p-2 overflow-x-auto text-[11px] leading-snug">
            {JSON.stringify(tool.parameters, null, 2)}
          </pre>
        )}
      </CardContent>
    </Card>
  )
}

function SummaryTile({
  label,
  value,
  danger,
}: {
  label: string
  value: number
  danger?: boolean
}) {
  return (
    <Card>
      <CardContent className="pt-6">
        <div
          className={
            "text-3xl font-semibold tracking-tight " +
            (danger ? "text-destructive" : "")
          }
        >
          {value}
        </div>
        <div className="text-xs text-muted-foreground mt-1">{label}</div>
      </CardContent>
    </Card>
  )
}
