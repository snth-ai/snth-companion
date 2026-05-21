import { useEffect, useMemo, useState } from "react"
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import { Button } from "@/components/ui/button"
import { Badge } from "@/components/ui/badge"
import { Layers, RefreshCw, ChevronRight, AlertTriangle } from "lucide-react"
import {
  fetchContextSessions,
  fetchContextSnapshots,
  type CtxSnapshot,
} from "@/lib/api"

// Context — per-turn breakdown of what's filling the LLM context for
// the bound synth. Fetches from synth /api/context-snapshot through
// the hub /api/hub/synth-fetch passthrough.
//
// Mirrors the hub admin "Context" page in shape (treemap + history
// table) but rendered with shadcn primitives in the companion shell.

type Cell = {
  label: string
  bytes: number
  percent: number
  color: string
  group: "system" | "history" | "tools" | "dynamic"
}

function humanBytes(n: number): string {
  if (n < 1024) return `${n} B`
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`
  return `${(n / 1024 / 1024).toFixed(1)} MB`
}

function ctxHash(key: string, hueBase: number, sat: number, light: number) {
  let h = 5381
  for (let i = 0; i < key.length; i++) h = (h * 33 + key.charCodeAt(i)) >>> 0
  const hue = ((hueBase + (h % 60) - 30 + 360) % 360) | 0
  return `hsl(${hue} ${sat}% ${light}%)`
}

function buildCells(s: CtxSnapshot): Cell[] {
  const cells: Cell[] = []
  for (const sec of s.system_prompt?.sections ?? []) {
    cells.push({
      label: "system." + sec.key,
      bytes: sec.bytes,
      percent: 0,
      color: ctxHash(sec.key, 215, 60, 38),
      group: "system",
    })
  }
  const h = s.message_history
  if (h?.user_bytes)
    cells.push({ label: "history.user", bytes: h.user_bytes, percent: 0, color: ctxHash("user", 142, 55, 40), group: "history" })
  if (h?.assistant_bytes)
    cells.push({ label: "history.assistant", bytes: h.assistant_bytes, percent: 0, color: ctxHash("assistant", 142, 55, 40), group: "history" })
  if (h?.tool_result_bytes)
    cells.push({ label: "history.tool_result", bytes: h.tool_result_bytes, percent: 0, color: ctxHash("tool_result", 142, 55, 40), group: "history" })
  if (h?.system_bytes)
    cells.push({ label: "history.system", bytes: h.system_bytes, percent: 0, color: ctxHash("history_system", 142, 55, 40), group: "history" })
  if (s.tool_schemas_bytes)
    cells.push({ label: "tool_schemas", bytes: s.tool_schemas_bytes, percent: 0, color: "hsl(28 70% 42%)", group: "tools" })
  if (s.dynamic_context_bytes)
    cells.push({ label: "dynamic_context", bytes: s.dynamic_context_bytes, percent: 0, color: "hsl(280 50% 42%)", group: "dynamic" })

  const total = s.total_bytes || cells.reduce((a, c) => a + c.bytes, 0)
  for (const c of cells) {
    c.percent = total > 0 ? (c.bytes / total) * 100 : 0
  }
  cells.sort((a, b) => b.bytes - a.bytes)
  return cells
}

export function ContextPage() {
  const [sessions, setSessions] = useState<string[] | null>(null)
  const [session, setSession] = useState<string | null>(null)
  const [snaps, setSnaps] = useState<CtxSnapshot[] | null>(null)
  const [err, setErr] = useState<string | null>(null)
  const [loading, setLoading] = useState(false)

  const refreshSessions = async () => {
    setLoading(true)
    setErr(null)
    try {
      const list = await fetchContextSessions()
      setSessions(list)
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }

  const loadSession = async (s: string) => {
    setLoading(true)
    setErr(null)
    setSession(s)
    try {
      const out = await fetchContextSnapshots(s)
      setSnaps(out)
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }

  useEffect(() => {
    void refreshSessions()
  }, [])

  const latest = snaps?.[snaps.length - 1]
  const cells = useMemo(() => (latest ? buildCells(latest) : []), [latest])

  return (
    <div className="flex flex-col gap-4">
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-2xl font-semibold tracking-tight flex items-center gap-2">
            <Layers className="h-6 w-6 text-primary" />
            Context
          </h1>
          <p className="text-sm text-muted-foreground">
            Per-turn breakdown of what fills the LLM context for the bound synth.
          </p>
        </div>
        <Button variant="outline" size="sm" onClick={() => session ? loadSession(session) : refreshSessions()} disabled={loading}>
          <RefreshCw className={`h-4 w-4 mr-2 ${loading ? "animate-spin" : ""}`} />
          Refresh
        </Button>
      </div>

      {err && (
        <Alert variant="destructive">
          <AlertTriangle className="h-4 w-4" />
          <AlertTitle>Couldn't load context</AlertTitle>
          <AlertDescription>{err}</AlertDescription>
        </Alert>
      )}

      {!session && (
        <Card>
          <CardHeader>
            <CardTitle className="text-base">Sessions with captured snapshots</CardTitle>
          </CardHeader>
          <CardContent>
            {sessions === null && <p className="text-sm text-muted-foreground">Loading…</p>}
            {sessions && sessions.length === 0 && (
              <p className="text-sm text-muted-foreground">
                No snapshots captured yet. Snapshots populate after the synth processes at least one message turn after restart.
              </p>
            )}
            {sessions && sessions.length > 0 && (
              <ul className="flex flex-col gap-1">
                {sessions.map((s) => (
                  <li key={s}>
                    <button
                      onClick={() => loadSession(s)}
                      className="w-full text-left flex items-center justify-between px-3 py-2 rounded-md bg-muted hover:bg-accent transition-colors group"
                    >
                      <span className="font-mono text-sm">{s}</span>
                      <ChevronRight className="h-4 w-4 opacity-50 group-hover:opacity-100" />
                    </button>
                  </li>
                ))}
              </ul>
            )}
          </CardContent>
        </Card>
      )}

      {session && (
        <>
          <div className="flex items-center gap-2">
            <Button variant="ghost" size="sm" onClick={() => { setSession(null); setSnaps(null) }}>
              ← all sessions
            </Button>
            <span className="font-mono text-sm text-muted-foreground">{session}</span>
          </div>

          {!latest && !loading && (
            <Card>
              <CardContent className="py-8 text-sm text-muted-foreground text-center">
                No snapshots in this session yet.
              </CardContent>
            </Card>
          )}

          {latest && (
            <>
              <Card>
                <CardHeader className="flex flex-row items-start justify-between space-y-0">
                  <div>
                    <CardTitle className="text-base flex items-center gap-2">
                      Latest snapshot
                      <Badge variant="secondary">{latest.system_prompt?.mode}</Badge>
                    </CardTitle>
                    <p className="text-xs text-muted-foreground mt-1">
                      {new Date(latest.built_at).toLocaleString()} ·{" "}
                      <span className="font-mono">{latest.provider}/{latest.model}</span>
                    </p>
                  </div>
                  <div className="text-right">
                    <div className="text-sm font-mono">{humanBytes(latest.total_bytes)}</div>
                    <div className="text-xs text-muted-foreground">
                      ~{humanBytes(Math.round(latest.total_bytes / 4))} tokens
                    </div>
                  </div>
                </CardHeader>
                <CardContent>
                  <div className="flex flex-wrap gap-1 rounded-md p-1 min-h-[340px] bg-muted/40 border">
                    {cells.map((c) => (
                      <div
                        key={c.label}
                        className="rounded-sm p-2.5 text-white overflow-hidden flex flex-col justify-end min-w-[88px] min-h-[88px]"
                        style={{
                          flexGrow: Math.max(c.bytes, 1),
                          flexBasis: 0,
                          backgroundColor: c.color,
                        }}
                        title={`${c.label} · ${humanBytes(c.bytes)} (${c.percent.toFixed(1)}%)`}
                      >
                        <div className="text-xs font-semibold opacity-95 break-words leading-tight">{c.label}</div>
                        <div className="text-[10.5px] opacity-75 mt-0.5">
                          {humanBytes(c.bytes)} · {c.percent.toFixed(1)}%
                        </div>
                      </div>
                    ))}
                  </div>
                </CardContent>
              </Card>

              <Card>
                <CardHeader>
                  <CardTitle className="text-sm uppercase tracking-wide text-muted-foreground">Legend</CardTitle>
                </CardHeader>
                <CardContent>
                  <table className="w-full text-sm">
                    <thead>
                      <tr className="text-xs uppercase text-muted-foreground">
                        <th className="text-left font-medium py-1.5">Section</th>
                        <th className="text-left font-medium py-1.5">Group</th>
                        <th className="text-right font-medium py-1.5">Bytes</th>
                        <th className="text-right font-medium py-1.5">%</th>
                      </tr>
                    </thead>
                    <tbody>
                      {cells.map((c) => (
                        <tr key={c.label} className="border-t border-border/50">
                          <td className="py-1.5">
                            <span className="inline-block w-3 h-3 rounded-sm mr-2 align-[-1px]" style={{ backgroundColor: c.color }} />
                            {c.label}
                          </td>
                          <td className="py-1.5 text-muted-foreground">{c.group}</td>
                          <td className="py-1.5 text-right font-mono tabular-nums">{humanBytes(c.bytes)}</td>
                          <td className="py-1.5 text-right font-mono tabular-nums">{c.percent.toFixed(1)}%</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </CardContent>
              </Card>

              {snaps && snaps.length > 1 && (
                <Card>
                  <CardHeader>
                    <CardTitle className="text-sm uppercase tracking-wide text-muted-foreground">
                      History · {snaps.length} snapshots
                    </CardTitle>
                  </CardHeader>
                  <CardContent>
                    <table className="w-full text-sm">
                      <thead>
                        <tr className="text-xs uppercase text-muted-foreground">
                          <th className="text-left font-medium py-1.5">built_at</th>
                          <th className="text-left font-medium py-1.5">mode</th>
                          <th className="text-right font-medium py-1.5">total</th>
                          <th className="text-right font-medium py-1.5">system</th>
                          <th className="text-right font-medium py-1.5">history</th>
                          <th className="text-right font-medium py-1.5">tools</th>
                          <th className="text-right font-medium py-1.5">dynamic</th>
                        </tr>
                      </thead>
                      <tbody>
                        {snaps.map((s, i) => (
                          <tr key={i} className="border-t border-border/50">
                            <td className="py-1.5 font-mono tabular-nums text-muted-foreground">
                              {new Date(s.built_at).toLocaleTimeString()}
                            </td>
                            <td className="py-1.5 text-muted-foreground">{s.system_prompt?.mode}</td>
                            <td className="py-1.5 text-right font-mono tabular-nums">{humanBytes(s.total_bytes)}</td>
                            <td className="py-1.5 text-right font-mono tabular-nums">{humanBytes(s.system_prompt?.total_bytes ?? 0)}</td>
                            <td className="py-1.5 text-right font-mono tabular-nums">{humanBytes(s.message_history?.total_bytes ?? 0)}</td>
                            <td className="py-1.5 text-right font-mono tabular-nums">{humanBytes(s.tool_schemas_bytes ?? 0)}</td>
                            <td className="py-1.5 text-right font-mono tabular-nums">{humanBytes(s.dynamic_context_bytes ?? 0)}</td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  </CardContent>
                </Card>
              )}
            </>
          )}
        </>
      )}
    </div>
  )
}
