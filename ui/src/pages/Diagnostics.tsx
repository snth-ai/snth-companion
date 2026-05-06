import { useEffect, useMemo, useState } from "react"
import { Card, CardContent } from "@/components/ui/card"
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import { Button } from "@/components/ui/button"
import { Badge } from "@/components/ui/badge"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import {
  Activity,
  AlertTriangle,
  ChevronDown,
  ChevronRight,
  Copy,
  RefreshCw,
} from "lucide-react"
import {
  fetchTraces,
  fetchTraceRaw,
  type TraceRow,
  type TraceRawResponse,
} from "@/lib/api"
import { toast } from "sonner"

// Diagnostics — per-turn observability for the bound synth.
//
// Mirrors the hub /traces page but scoped to one synth and rendered
// in the companion shell. Synth records every processMessage /
// heartbeat / dreaming / scheduled_act tick into trace_rows; this page
// fetches via /api/my/synth-fetch → /api/traces, computes stats, lays
// out the timeline + per-turn waterfall.

type Period = "1h" | "6h" | "24h" | "7d"
const PERIODS: Period[] = ["1h", "6h", "24h", "7d"]

const KIND_COLOR: Record<string, string> = {
  user: "bg-blue-900 text-blue-200",
  heartbeat: "bg-emerald-900 text-emerald-200",
  dreaming: "bg-purple-900 text-purple-200",
  scheduled_act: "bg-orange-900 text-orange-200",
}

const KIND_BORDER: Record<string, string> = {
  user: "border-l-blue-500",
  heartbeat: "border-l-emerald-500",
  dreaming: "border-l-purple-500",
  scheduled_act: "border-l-orange-500",
}

function periodWindow(p: Period): { from: Date; to: Date } {
  const to = new Date()
  const from = new Date(to)
  switch (p) {
    case "1h":
      from.setHours(from.getHours() - 1)
      break
    case "6h":
      from.setHours(from.getHours() - 6)
      break
    case "24h":
      from.setHours(from.getHours() - 24)
      break
    case "7d":
      from.setDate(from.getDate() - 7)
      break
  }
  return { from, to }
}

function fmtMs(ms: number): string {
  if (!ms) return "0ms"
  if (ms < 1000) return `${ms}ms`
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)}s`
  const m = Math.floor(ms / 60_000)
  const s = Math.round((ms % 60_000) / 1000)
  return `${m}m${s}s`
}

function fmtBytes(b: number): string {
  if (!b) return "0"
  if (b < 1024) return `${b}B`
  if (b < 1024 * 1024) return `${(b / 1024).toFixed(1)}K`
  return `${(b / 1024 / 1024).toFixed(1)}M`
}

function fmtTime(iso: string): string {
  const d = new Date(iso)
  return (
    d.toLocaleDateString("sv-SE") + " " + d.toLocaleTimeString("en-GB")
  )
}

function pctile(arr: number[], p: number): number {
  if (!arr.length) return 0
  const sorted = [...arr].sort((a, b) => a - b)
  const idx = Math.min(
    sorted.length - 1,
    Math.ceil((p / 100) * sorted.length) - 1,
  )
  return sorted[Math.max(0, idx)]
}

export function DiagnosticsPage() {
  const [traces, setTraces] = useState<TraceRow[]>([])
  const [sessions, setSessions] = useState<string[]>([])
  const [period, setPeriod] = useState<Period>("24h")
  const [kind, setKind] = useState<string>("")
  const [outcome, setOutcome] = useState<string>("")
  const [session, setSession] = useState<string>("")
  const [hasError, setHasError] = useState(false)
  const [minDurMs, setMinDurMs] = useState<number>(0)
  const [loading, setLoading] = useState(false)
  const [err, setErr] = useState<string | null>(null)
  const [expanded, setExpanded] = useState<Record<string, boolean>>({})
  const [rawCache, setRawCache] = useState<
    Record<string, TraceRawResponse | { error: string } | "loading">
  >({})

  const load = async () => {
    setLoading(true)
    setErr(null)
    try {
      const w = periodWindow(period)
      const r = await fetchTraces({
        from: w.from,
        to: w.to,
        limit: 2000,
      })
      setTraces(r.traces ?? [])
      setSessions(r.sessions ?? [])
    } catch (e) {
      setErr(String((e as Error).message ?? e))
    } finally {
      setLoading(false)
    }
  }

  useEffect(() => {
    void load()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [period])

  // Client-side filter — server returns the full window then we narrow.
  const filtered = useMemo(() => {
    return traces.filter((t) => {
      if (kind && t.kind !== kind) return false
      if (outcome && t.outcome !== outcome) return false
      if (session && t.session_id !== session) return false
      if (hasError && !t.error_text) return false
      if (minDurMs && t.duration_ms < minDurMs) return false
      return true
    })
  }, [traces, kind, outcome, session, hasError, minDurMs])

  const stats = useMemo(() => {
    const total = filtered.length
    const totalCost = filtered.reduce(
      (s, t) => s + (t.llm_cost_usd || 0),
      0,
    )
    const errors = filtered.filter(
      (t) => t.outcome === "error" || t.outcome === "panic",
    ).length
    const durs = filtered.map((t) => t.duration_ms || 0)
    const avg = durs.length
      ? Math.round(durs.reduce((a, b) => a + b, 0) / durs.length)
      : 0
    return {
      total,
      totalCost,
      errors,
      avg,
      p95: pctile(durs, 95),
      p99: pctile(durs, 99),
      errorPct: total ? (100 * errors) / total : 0,
    }
  }, [filtered])

  const toggle = (id: string) =>
    setExpanded((e) => ({ ...e, [id]: !e[id] }))

  const loadRaw = async (id: string) => {
    if (rawCache[id] && rawCache[id] !== "loading") {
      // toggle off
      setRawCache((c) => {
        const n = { ...c }
        delete n[id]
        return n
      })
      return
    }
    setRawCache((c) => ({ ...c, [id]: "loading" }))
    try {
      const r = await fetchTraceRaw(id)
      setRawCache((c) => ({ ...c, [id]: r }))
    } catch (e) {
      setRawCache((c) => ({
        ...c,
        [id]: { error: String((e as Error).message ?? e) },
      }))
    }
  }

  const copyId = (id: string) => {
    void navigator.clipboard.writeText(id)
    toast.success(`copied trace_id ${id.slice(0, 8)}`)
  }

  return (
    <div className="space-y-3">
      <div className="flex items-end justify-between gap-3 flex-wrap">
        <div>
          <h1 className="text-2xl font-semibold tracking-tight">
            Diagnostics
          </h1>
          <p className="text-sm text-muted-foreground mt-1">
            Per-turn traces — every processMessage, heartbeat, dreaming
            tick, scheduled_act. Click a row to expand its waterfall + tool
            calls + raw log.
          </p>
        </div>
        <div className="flex items-center gap-2">
          <select
            value={period}
            onChange={(e) => setPeriod(e.target.value as Period)}
            className="text-sm bg-card border border-border rounded px-2 py-1.5"
          >
            {PERIODS.map((p) => (
              <option key={p} value={p}>
                last {p}
              </option>
            ))}
          </select>
          <Button
            size="sm"
            variant="secondary"
            onClick={() => void load()}
            disabled={loading}
          >
            <RefreshCw
              className={"h-4 w-4 mr-1" + (loading ? " animate-spin" : "")}
            />
            refresh
          </Button>
        </div>
      </div>

      {err && (
        <Alert variant="destructive">
          <AlertTitle>Error</AlertTitle>
          <AlertDescription>{err}</AlertDescription>
        </Alert>
      )}

      <div className="grid grid-cols-2 md:grid-cols-3 lg:grid-cols-6 gap-2">
        <StatCard label="Total turns" value={String(stats.total)} />
        <StatCard label="Avg duration" value={fmtMs(stats.avg)} />
        <StatCard label="p95 duration" value={fmtMs(stats.p95)} />
        <StatCard label="p99 duration" value={fmtMs(stats.p99)} />
        <StatCard
          label="Total cost"
          value={"$" + stats.totalCost.toFixed(4)}
          accent="text-emerald-400"
        />
        <StatCard
          label="Errors"
          value={`${stats.errors} (${stats.errorPct.toFixed(1)}%)`}
          accent={stats.errors > 0 ? "text-red-400" : ""}
        />
      </div>

      <Card>
        <CardContent className="p-3">
          <div className="flex flex-wrap items-end gap-2">
            <FilterSelect
              label="Kind"
              value={kind}
              onChange={setKind}
              options={[
                { value: "", label: "any" },
                { value: "user", label: "user" },
                { value: "heartbeat", label: "heartbeat" },
                { value: "dreaming", label: "dreaming" },
                { value: "scheduled_act", label: "scheduled_act" },
              ]}
            />
            <FilterSelect
              label="Outcome"
              value={outcome}
              onChange={setOutcome}
              options={[
                { value: "", label: "any" },
                { value: "reply_sent", label: "reply_sent" },
                { value: "no_reply", label: "no_reply" },
                { value: "silent", label: "silent" },
                { value: "blocked", label: "blocked" },
                { value: "error", label: "error" },
                { value: "panic", label: "panic" },
                { value: "skipped", label: "skipped" },
              ]}
            />
            <FilterSelect
              label="Session"
              value={session}
              onChange={setSession}
              options={[
                { value: "", label: "any" },
                ...sessions.map((s) => ({ value: s, label: s })),
              ]}
            />
            <div className="space-y-1">
              <Label htmlFor="min-dur" className="text-xs uppercase tracking-wider text-muted-foreground">
                min duration (ms)
              </Label>
              <Input
                id="min-dur"
                type="number"
                min={0}
                value={minDurMs || ""}
                onChange={(e) => setMinDurMs(Number(e.target.value) || 0)}
                className="h-8 w-28"
                placeholder="0"
              />
            </div>
            <label className="flex items-center gap-2 text-sm cursor-pointer h-8 px-2">
              <input
                type="checkbox"
                checked={hasError}
                onChange={(e) => setHasError(e.target.checked)}
              />
              has error
            </label>
            <span className="ml-auto text-xs text-muted-foreground">
              {filtered.length} of {traces.length} traces
            </span>
          </div>
        </CardContent>
      </Card>

      {filtered.length === 0 && !loading ? (
        <Card>
          <CardContent className="p-10 text-center text-sm text-muted-foreground italic">
            no traces in this window
          </CardContent>
        </Card>
      ) : (
        <div className="space-y-1">
          {filtered.map((t) => (
            <TurnRow
              key={t.trace_id}
              trace={t}
              expanded={!!expanded[t.trace_id]}
              onToggle={() => toggle(t.trace_id)}
              onCopyId={() => copyId(t.trace_id)}
              raw={rawCache[t.trace_id]}
              onLoadRaw={() => void loadRaw(t.trace_id)}
            />
          ))}
        </div>
      )}
    </div>
  )
}

function StatCard({
  label,
  value,
  accent,
}: {
  label: string
  value: string
  accent?: string
}) {
  return (
    <Card>
      <CardContent className="p-3">
        <div className={"text-xl font-semibold " + (accent ?? "")}>{value}</div>
        <div className="text-[11px] uppercase tracking-wider text-muted-foreground mt-1">
          {label}
        </div>
      </CardContent>
    </Card>
  )
}

function FilterSelect({
  label,
  value,
  onChange,
  options,
}: {
  label: string
  value: string
  onChange: (v: string) => void
  options: Array<{ value: string; label: string }>
}) {
  return (
    <div className="space-y-1">
      <Label className="text-xs uppercase tracking-wider text-muted-foreground">
        {label}
      </Label>
      <select
        value={value}
        onChange={(e) => onChange(e.target.value)}
        className="text-sm bg-card border border-border rounded px-2 h-8"
      >
        {options.map((o) => (
          <option key={o.value} value={o.value}>
            {o.label}
          </option>
        ))}
      </select>
    </div>
  )
}

function summary(t: TraceRow): string {
  const parts: string[] = []
  if (t.memory_recall_count > 0) parts.push(`${t.memory_recall_count} recall`)
  if (t.memory_loop_entities > 0)
    parts.push(`${t.memory_loop_entities} entities`)
  if (t.wiki_pages > 0) parts.push(`${t.wiki_pages} wiki`)
  if (t.tool_calls?.length) parts.push(`${t.tool_calls.length} tools`)
  if (t.compact_ms > 0) parts.push(`compact ${fmtMs(t.compact_ms)}`)
  if (t.llm_calls > 0) parts.push(`${t.llm_calls} llm`)
  if (t.outcome === "silent") parts.push("HEARTBEAT_OK")
  if (t.outcome === "error" || t.outcome === "panic")
    parts.push(
      "⚠ " + (t.error_text || t.outcome_reason || "").slice(0, 60),
    )
  if (t.reply_preview) parts.push(`"${t.reply_preview.slice(0, 80)}"`)
  return parts.join(" · ")
}

function MiniBar({ t }: { t: TraceRow }) {
  if (!t.duration_ms)
    return <div className="h-1.5 w-48 rounded bg-muted" />
  const total =
    (t.memory_recall_ms || 0) +
    (t.memory_loop_ms || 0) +
    (t.wiki_ms || 0) +
    (t.compact_ms || 0)
  const llmAndTools = Math.max(0, t.duration_ms - total)
  const seg = (color: string, ms: number) =>
    ms > 0 ? (
      <div
        className={color}
        style={{ width: `${(100 * ms) / t.duration_ms}%` }}
      />
    ) : null
  return (
    <div
      className="flex h-1.5 rounded overflow-hidden bg-muted/50 w-48 lg:w-64 shrink-0"
      title={`recall ${fmtMs(t.memory_recall_ms)} · memLoop ${fmtMs(
        t.memory_loop_ms,
      )} · wiki ${fmtMs(t.wiki_ms)} · compact ${fmtMs(
        t.compact_ms,
      )} · llm+tools ${fmtMs(llmAndTools)}`}
    >
      {seg("bg-emerald-500", t.memory_recall_ms)}
      {seg("bg-amber-500", t.memory_loop_ms)}
      {seg("bg-cyan-500", t.wiki_ms)}
      {seg("bg-indigo-500", t.compact_ms)}
      {seg("bg-violet-500", llmAndTools)}
    </div>
  )
}

function TurnRow({
  trace: t,
  expanded,
  onToggle,
  onCopyId,
  raw,
  onLoadRaw,
}: {
  trace: TraceRow
  expanded: boolean
  onToggle: () => void
  onCopyId: () => void
  raw?: TraceRawResponse | { error: string } | "loading"
  onLoadRaw: () => void
}) {
  const isError = t.outcome === "error" || t.outcome === "panic"
  const borderClass = isError
    ? "border-l-red-500"
    : KIND_BORDER[t.kind] ?? "border-l-slate-600"
  return (
    <div>
      <button
        onClick={onToggle}
        className={
          "w-full flex items-center gap-3 px-3 py-2 border-l-4 bg-card/40 hover:bg-card transition rounded-r text-left " +
          borderClass
        }
      >
        {expanded ? (
          <ChevronDown className="h-3.5 w-3.5 text-muted-foreground shrink-0" />
        ) : (
          <ChevronRight className="h-3.5 w-3.5 text-muted-foreground shrink-0" />
        )}
        <Badge
          className={
            "text-[10px] uppercase tracking-wider " +
            (KIND_COLOR[t.kind] ?? "bg-slate-700 text-slate-200")
          }
          variant="secondary"
        >
          {t.kind.replace("_", " ")}
        </Badge>
        <span className="font-mono text-xs text-muted-foreground w-20 shrink-0">
          {t.trace_id.slice(0, 8)}
        </span>
        <span className="font-mono text-xs text-muted-foreground/80 w-40 shrink-0">
          {fmtTime(t.started_at)}
        </span>
        <span className="font-mono text-xs w-16 text-right shrink-0">
          {fmtMs(t.duration_ms)}
        </span>
        <span className="font-mono text-xs text-emerald-400 w-16 text-right shrink-0">
          {t.llm_cost_usd > 0 ? "$" + t.llm_cost_usd.toFixed(4) : "—"}
        </span>
        <MiniBar t={t} />
        <span className="text-xs text-muted-foreground truncate flex-1 min-w-0">
          {isError && (
            <AlertTriangle className="h-3.5 w-3.5 text-red-400 inline mr-1" />
          )}
          {summary(t)}
        </span>
      </button>

      {expanded && (
        <div className="bg-card border border-border border-t-0 rounded-b p-4 mb-2 space-y-4">
          <Waterfall t={t} />

          <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-3">
            <DetailCard title="Memory">
              <Row k="recall" v={`${t.memory_recall_count} · ${fmtBytes(t.memory_recall_bytes)} · ${fmtMs(t.memory_recall_ms)}`} />
              <Row k="memLoop" v={`${t.memory_loop_entities} ents · ${fmtBytes(t.memory_loop_bytes)} · ${fmtMs(t.memory_loop_ms)}`} />
              <Row k="wiki" v={`${t.wiki_pages} pgs · ${fmtBytes(t.wiki_bytes)} · ${fmtMs(t.wiki_ms)}`} />
              {t.compact_ms > 0 && <Row k="compact" v={fmtMs(t.compact_ms)} />}
            </DetailCard>

            <DetailCard title={`Tools (${t.tool_calls?.length ?? 0})`}>
              {t.tool_calls?.length ? (
                t.tool_calls.map((tc, i) => (
                  <Row
                    key={i}
                    k={tc.name + (tc.ok ? "" : " ⚠")}
                    v={fmtMs(tc.duration_ms)}
                  />
                ))
              ) : (
                <div className="text-xs italic text-muted-foreground">
                  none
                </div>
              )}
            </DetailCard>

            <DetailCard title="LLM">
              <Row k="calls" v={String(t.llm_calls)} />
              <Row k="prompt tok" v={t.llm_prompt_tokens.toLocaleString()} />
              <Row k="cached tok" v={t.llm_cached_tokens.toLocaleString()} />
              <Row k="output tok" v={t.llm_output_tokens.toLocaleString()} />
              <Row k="cost" v={"$" + t.llm_cost_usd.toFixed(4)} />
            </DetailCard>

            <DetailCard title="Meta">
              <Row k="session" v={t.session_id || "—"} />
              <Row k="outcome" v={t.outcome} />
              {t.outcome_reason && <Row k="reason" v={t.outcome_reason} />}
              {t.rss_bytes > 0 && <Row k="rss" v={fmtBytes(t.rss_bytes)} />}
              <Row k="input" v={fmtBytes(t.input_bytes)} />
              <Row k="output" v={fmtBytes(t.output_bytes)} />
            </DetailCard>
          </div>

          {t.reply_preview && (
            <div className="border-l-2 border-blue-500 bg-background p-3 rounded-r text-sm italic text-foreground/80 max-h-32 overflow-auto">
              "{t.reply_preview}"
            </div>
          )}

          {t.error_text && (
            <div className="border-l-2 border-red-500 bg-background p-3 rounded-r text-sm font-mono text-red-300 whitespace-pre-wrap">
              {t.error_text}
            </div>
          )}

          <div className="flex items-center gap-2 pt-1">
            <Button size="sm" variant="secondary" onClick={onLoadRaw}>
              <Activity className="h-4 w-4 mr-1" />
              {raw && raw !== "loading" ? "hide raw log" : "show raw log"}
            </Button>
            <Button size="sm" variant="ghost" onClick={onCopyId}>
              <Copy className="h-4 w-4 mr-1" />
              copy trace_id
            </Button>
          </div>

          {raw === "loading" && (
            <div className="text-xs text-muted-foreground italic">
              loading raw log…
            </div>
          )}
          {raw && raw !== "loading" && "error" in raw && (
            <div className="text-xs text-red-400 italic">{raw.error}</div>
          )}
          {raw && raw !== "loading" && "lines" in raw && (
            <div className="bg-background border border-border rounded p-3 max-h-96 overflow-auto font-mono text-[10px] text-muted-foreground/90 leading-relaxed">
              <div className="text-muted-foreground mb-2">
                {raw.line_count} lines
                {raw.path && ` from ${raw.path}`}
              </div>
              {raw.lines.map((ln, i) => (
                <div key={i} className="whitespace-pre-wrap">
                  {ln}
                </div>
              ))}
            </div>
          )}
        </div>
      )}
    </div>
  )
}

function Waterfall({ t }: { t: TraceRow }) {
  const segs: Array<[string, number, string, string]> = [
    [
      "memory.recall",
      t.memory_recall_ms,
      "bg-emerald-500",
      `${t.memory_recall_count} results · ${fmtBytes(t.memory_recall_bytes)}`,
    ],
    [
      "memory.loop",
      t.memory_loop_ms,
      "bg-amber-500",
      `${t.memory_loop_entities} entities · ${fmtBytes(t.memory_loop_bytes)}`,
    ],
    [
      "memory.wiki",
      t.wiki_ms,
      "bg-cyan-500",
      `${t.wiki_pages} pages · ${fmtBytes(t.wiki_bytes)}`,
    ],
    ["compact", t.compact_ms, "bg-indigo-500", ""],
  ]
  const parallelMax = Math.max(
    t.memory_recall_ms,
    t.memory_loop_ms,
    t.wiki_ms,
    1,
  )
  const llmAndTools = Math.max(
    0,
    t.duration_ms - parallelMax - t.compact_ms,
  )
  segs.push([
    "llm + tools",
    llmAndTools,
    "bg-violet-500",
    `${t.llm_calls} llm calls`,
  ])
  if (t.tool_calls?.length) {
    const totTool = t.tool_calls.reduce((s, x) => s + x.duration_ms, 0)
    segs.push([
      "tools (sum)",
      totTool,
      "bg-pink-500",
      `${t.tool_calls.length} calls`,
    ])
  }
  const totalSeg =
    Math.max(t.duration_ms, ...segs.map(([, ms]) => ms)) || 1
  return (
    <div className="space-y-1 font-mono text-[11px]">
      {segs
        .filter(([, ms]) => ms > 0)
        .map(([label, ms, color, hint]) => {
          const w = (100 * ms) / totalSeg
          return (
            <div key={label} className="flex items-center gap-2">
              <span className="w-32 text-muted-foreground shrink-0">
                {label}
              </span>
              <div className="flex-1 h-3.5 bg-background rounded relative overflow-hidden">
                <div
                  className={"h-full rounded " + color}
                  style={{ width: `${w}%` }}
                />
                <div className="absolute left-2 top-0 leading-[14px] text-[10px] text-foreground whitespace-nowrap">
                  {fmtMs(ms)}
                  {hint && " · " + hint}
                </div>
              </div>
            </div>
          )
        })}
    </div>
  )
}

function DetailCard({
  title,
  children,
}: {
  title: string
  children: React.ReactNode
}) {
  return (
    <div className="bg-background border border-border rounded p-3">
      <div className="text-xs font-semibold uppercase tracking-wider text-foreground mb-2">
        {title}
      </div>
      <div className="space-y-1">{children}</div>
    </div>
  )
}

function Row({ k, v }: { k: string; v: string }) {
  return (
    <div className="flex justify-between text-xs">
      <span className="text-muted-foreground">{k}</span>
      <span className="font-mono text-foreground">{v}</span>
    </div>
  )
}
