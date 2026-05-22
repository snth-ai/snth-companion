import { useEffect, useMemo, useState } from "react"
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import { Button } from "@/components/ui/button"
import { Badge } from "@/components/ui/badge"
import { Layers, RefreshCw, ChevronRight, AlertTriangle, FileText, Save, Loader2 } from "lucide-react"
import {
  fetchContextSessions,
  fetchContextSnapshots,
  fetchSynthConfigFiles,
  saveSynthConfigFile,
  type CtxSnapshot,
  type SynthConfigFiles,
} from "@/lib/api"

// Cost rates (USD per 1M input tokens). Used by the Wave D cost
// estimator. Conservative defaults — model rates change; this is a
// rough operator-orienting figure, not billing truth. Lookup is
// model-name-substring; unknown falls back to GPT-5.5 ($5/M) since
// most of the fleet runs on it.
const COST_RATES_USD_PER_1M_INPUT: Array<{ match: RegExp; rate: number; label: string }> = [
  { match: /gpt-5\.5-pro/i, rate: 30.0, label: "gpt-5.5-pro" },
  { match: /gpt-5\.5/i, rate: 5.0, label: "gpt-5.5" },
  { match: /gpt-5\.4-mini/i, rate: 0.4, label: "gpt-5.4-mini" },
  { match: /gpt-5\.4/i, rate: 2.5, label: "gpt-5.4" },
  { match: /claude-sonnet/i, rate: 3.0, label: "claude-sonnet" },
  { match: /claude-haiku/i, rate: 1.0, label: "claude-haiku" },
  { match: /gemini-3\.1-flash-lite/i, rate: 0.075, label: "gemini-3.1-flash-lite" },
  { match: /gemini-3/i, rate: 0.3, label: "gemini-3" },
  { match: /glm-5/i, rate: 1.4, label: "zai-glm-5" },
]

function lookupRate(model: string): { rate: number; label: string } {
  for (const r of COST_RATES_USD_PER_1M_INPUT) {
    if (r.match.test(model)) return { rate: r.rate, label: r.label }
  }
  return { rate: 5.0, label: "default (gpt-5.5)" }
}

const EDITABLE_FILES: Array<{ key: keyof SynthConfigFiles; filename: string; label: string; hint: string }> = [
  { key: "soul_md", filename: "SOUL.md", label: "SOUL.md", hint: "Identity DNA — name, temperament, reaction rules" },
  { key: "rules_md", filename: "RULES.md", label: "RULES.md", hint: "Behavior rules + hard NO list" },
  { key: "heartbeat_md", filename: "HEARTBEAT.md", label: "HEARTBEAT.md", hint: "What synth checks on heartbeat tick" },
  { key: "memory_md", filename: "MEMORY.md", label: "MEMORY.md", hint: "Long-term persistent notes" },
  { key: "agents_md", filename: "AGENTS.md", label: "AGENTS.md", hint: "Coding-agent style guide for self-edits" },
]

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

      {/* Workspace MD editor — Wave D. Independent of any session
          being selected; always visible at the bottom of the page so
          operators can edit identity/rules/heartbeat docs without
          juggling tabs. */}
      <ConfigEditor />

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
                    {(() => {
                      const { rate, label } = lookupRate(latest.model ?? "")
                      const tokens = Math.round(latest.total_bytes / 4)
                      const cost = (tokens / 1_000_000) * rate
                      return (
                        <div className="text-xs mt-1">
                          <span className="font-mono">${cost.toFixed(4)}</span>
                          <span className="text-muted-foreground"> /turn @ {label}</span>
                        </div>
                      )
                    })()}
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

              {/* placeholder marker — history table follows */}
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

// ConfigEditor — Wave D. Lets the operator view + edit the 5 workspace
// markdown files that the synth re-reads from disk on every turn
// (SOUL.md / RULES.md / HEARTBEAT.md / MEMORY.md / AGENTS.md). The
// synth side already exposes GET + POST /api/config gated to this
// allow-list; this is just the UI surface.
//
// One file at a time, simple tab strip on top. Save button only
// enabled when the textarea diverges from the loaded copy. Refresh
// re-pulls the live disk state (useful after a synth-side edit
// landed via heartbeat or self_edit).
function ConfigEditor() {
  const [files, setFiles] = useState<SynthConfigFiles | null>(null)
  const [active, setActive] = useState<keyof SynthConfigFiles>("soul_md")
  const [draft, setDraft] = useState<string>("")
  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)
  const [err, setErr] = useState<string | null>(null)
  const [savedAt, setSavedAt] = useState<number | null>(null)

  async function refresh() {
    setLoading(true)
    setErr(null)
    try {
      const f = await fetchSynthConfigFiles()
      setFiles(f)
      setDraft(f[active])
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }

  useEffect(() => { void refresh() }, [])

  // Switching tabs loads the live file content into the draft.
  useEffect(() => {
    if (files) setDraft(files[active])
  }, [active, files])

  const dirty = files !== null && draft !== files[active]
  const activeMeta = EDITABLE_FILES.find((f) => f.key === active)!

  async function save() {
    if (!files || !dirty) return
    setSaving(true)
    setErr(null)
    try {
      await saveSynthConfigFile(activeMeta.filename, draft)
      // Optimistic update so dirty flag clears immediately.
      setFiles({ ...files, [active]: draft })
      setSavedAt(Date.now())
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e))
    } finally {
      setSaving(false)
    }
  }

  const charCount = draft.length
  const tokenEst = Math.round(charCount / 4)

  return (
    <Card>
      <CardHeader>
        <CardTitle className="text-base flex items-center gap-2">
          <FileText className="h-4 w-4" />
          Workspace docs
        </CardTitle>
        <p className="text-xs text-muted-foreground mt-1">
          These files are read from the synth's <span className="font-mono">workspace/</span> dir
          and re-injected into the LLM context on every turn. Changes apply on the next turn —
          no restart needed.
        </p>
      </CardHeader>
      <CardContent className="flex flex-col gap-3">
        <div className="flex flex-wrap gap-1.5">
          {EDITABLE_FILES.map((f) => {
            const isActive = f.key === active
            const isDirty = files !== null && f.key === active && draft !== files[f.key]
            return (
              <button
                key={f.key}
                onClick={() => setActive(f.key)}
                className={`px-3 py-1.5 text-xs rounded-md border transition-colors ${
                  isActive
                    ? "bg-primary text-primary-foreground border-primary"
                    : "bg-muted hover:bg-accent border-transparent"
                }`}
              >
                <span className="font-mono">{f.label}</span>
                {isDirty && <span className="ml-1 text-yellow-300">●</span>}
              </button>
            )
          })}
          <Button variant="outline" size="sm" onClick={refresh} disabled={loading || saving} className="ml-auto">
            <RefreshCw className={`h-3.5 w-3.5 mr-1.5 ${loading ? "animate-spin" : ""}`} />
            Reload
          </Button>
        </div>

        <p className="text-xs text-muted-foreground -mt-1">{activeMeta.hint}</p>

        {err && (
          <Alert variant="destructive">
            <AlertTriangle className="h-4 w-4" />
            <AlertTitle>Failed</AlertTitle>
            <AlertDescription>{err}</AlertDescription>
          </Alert>
        )}

        <textarea
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          disabled={loading}
          spellCheck={false}
          className="w-full h-64 rounded-md border border-input bg-muted/40 px-3 py-2 text-sm font-mono leading-relaxed focus:outline-none focus:ring-2 focus:ring-ring focus:bg-background resize-y"
          placeholder={loading ? "Loading…" : `${activeMeta.filename} is empty — type to start editing.`}
        />

        <div className="flex items-center justify-between text-xs">
          <div className="text-muted-foreground font-mono tabular-nums">
            {charCount.toLocaleString()} chars · ~{tokenEst.toLocaleString()} tokens
            {dirty && <span className="text-yellow-400 ml-2">unsaved changes</span>}
          </div>
          <div className="flex items-center gap-2">
            {savedAt && Date.now() - savedAt < 4000 && (
              <span className="text-emerald-400 text-xs">saved ✓</span>
            )}
            <Button size="sm" onClick={save} disabled={!dirty || saving}>
              {saving ? (
                <><Loader2 className="h-3.5 w-3.5 mr-1.5 animate-spin" />Saving…</>
              ) : (
                <><Save className="h-3.5 w-3.5 mr-1.5" />Save</>
              )}
            </Button>
          </div>
        </div>
      </CardContent>
    </Card>
  )
}
