import { useEffect, useMemo, useState } from "react"
import { Card, CardContent } from "@/components/ui/card"
import { Input } from "@/components/ui/input"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import {
  editEntity,
  editFact,
  fetchAgentJournal,
  fetchFacts,
  fetchGraphExport,
  fetchJournal,
  fetchMemoryOverview,
  fetchQuarantine,
  fetchWhyRecalled,
  forgetFact,
  mergeEntities,
  type AgentJournalEntry,
  type FactItem,
  type GraphV2Node,
  type JournalItem,
  type MemoryOverview,
  type QuarantineEntry,
  type WhyRecalled,
} from "@/lib/api"
import { toast } from "sonner"

// Memory — the durable journal+facts layer (2026-06 redesign). Two views:
//   • Facts: the synth's atomic durable knowledge — a "Who she knows you as"
//     profile card built from user_fact rows, plus a searchable, kind-filtered
//     ledger of everything else (decisions, events, preferences, relationships).
//   • Journal: the chronological "what happened" timeline, written automatically
//     at every compaction (no tool calls, no synth participation).

const kindLabels: Record<string, string> = {
  user_fact: "about you",
  preference: "preference",
  decision: "decision",
  event: "event",
  outcome: "outcome",
  relationship: "relationship",
  state: "state",
  task: "task",
  reflection: "reflection",
  fact: "fact",
  relation: "relation",
}

function kindLabel(k: string) {
  return kindLabels[k] ?? (k || "fact")
}

export function MemoryPage() {
  const [tab, setTab] = useState<"facts" | "journal" | "overview" | "entities">("facts")

  // overview state (Wave 4.1)
  const [overview, setOverview] = useState<MemoryOverview | null>(null)
  const [agentJournal, setAgentJournal] = useState<AgentJournalEntry[]>([])
  const [quarantine, setQuarantine] = useState<QuarantineEntry[]>([])
  const [why, setWhy] = useState<WhyRecalled | null>(null)
  const [whyOpen, setWhyOpen] = useState(false)

  // facts edit/forget (Wave 4.1 tail #9)
  const [editingId, setEditingId] = useState<string | null>(null)
  const [editText, setEditText] = useState("")

  const doForget = async (f: FactItem) => {
    if (!f.claim_id) return
    if (!confirm(`Forget this fact?\n\n"${f.text}"`)) return
    try {
      await forgetFact(f.claim_id, f.scope, false)
      toast.success("Forgotten")
      void loadFacts()
    } catch (e) {
      toast.error(String((e as Error).message ?? e))
    }
  }
  const doEditSave = async (f: FactItem) => {
    if (!f.claim_id || !editText.trim()) return
    try {
      await editFact(f.claim_id, editText.trim())
      toast.success("Updated")
      setEditingId(null)
      void loadFacts()
    } catch (e) {
      toast.error(String((e as Error).message ?? e))
    }
  }

  // facts state
  const [profile, setProfile] = useState<FactItem[]>([])
  const [facts, setFacts] = useState<FactItem[] | null>(null)
  const [counts, setCounts] = useState<Record<string, number>>({})
  const [total, setTotal] = useState(0)
  const [enabled, setEnabled] = useState(true)
  const [kind, setKind] = useState<string>("")
  const [search, setSearch] = useState("")
  const [err, setErr] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  // journal state
  const [journal, setJournal] = useState<JournalItem[] | null>(null)
  const [journalTotal, setJournalTotal] = useState(0)

  const loadFacts = async () => {
    setBusy(true)
    try {
      const d = await fetchFacts({ kind: kind || undefined, q: search || undefined, limit: 300 })
      setFacts(d.facts ?? [])
      setCounts(d.counts ?? {})
      setTotal(d.total ?? 0)
      setEnabled(d.enabled ?? true)
      setErr(null)
    } catch (e) {
      setErr(String((e as Error).message ?? e))
    } finally {
      setBusy(false)
    }
  }

  const loadProfile = async () => {
    try {
      const d = await fetchFacts({ kind: "user_fact", limit: 60 })
      setProfile(d.facts ?? [])
    } catch {
      // non-fatal — the ledger still renders
    }
  }

  const loadJournal = async () => {
    try {
      const d = await fetchJournal({ limit: 120 })
      setJournal(d.journal ?? [])
      setJournalTotal(d.total ?? 0)
    } catch (e) {
      setErr(String((e as Error).message ?? e))
    }
  }

  useEffect(() => {
    void loadProfile()
  }, [])

  useEffect(() => {
    if (tab === "facts") void loadFacts()
    if (tab === "journal" && journal === null) void loadJournal()
    if (tab === "overview" && overview === null) {
      void (async () => {
        try {
          setOverview(await fetchMemoryOverview())
          const [aj, q] = await Promise.all([fetchAgentJournal(), fetchQuarantine()])
          setAgentJournal(aj.entries ?? [])
          setQuarantine(q.entries ?? [])
        } catch (e) {
          setErr(String((e as Error).message ?? e))
        }
      })()
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tab, kind])

  const kindChips = useMemo(
    () =>
      Object.entries(counts)
        .filter(([k]) => k !== "user_fact")
        .sort((a, b) => b[1] - a[1]),
    [counts],
  )

  return (
    <div className="space-y-4">
      <div className="flex items-end justify-between gap-4 flex-wrap">
        <div>
          <h1 className="text-2xl font-semibold tracking-tight">Memory</h1>
          <p className="text-sm text-muted-foreground">
            Durable journal + facts — what she knows, captured automatically as you talk.
          </p>
        </div>
        <div className="flex gap-2">
          <Button size="sm" variant={tab === "facts" ? "default" : "outline"} onClick={() => setTab("facts")}>
            Facts {total ? `· ${total}` : ""}
          </Button>
          <Button size="sm" variant={tab === "journal" ? "default" : "outline"} onClick={() => setTab("journal")}>
            Journal {journalTotal ? `· ${journalTotal}` : ""}
          </Button>
          <Button size="sm" variant={tab === "overview" ? "default" : "outline"} onClick={() => setTab("overview")}>
            Overview
          </Button>
          <Button size="sm" variant={tab === "entities" ? "default" : "outline"} onClick={() => setTab("entities")}>
            Entities
          </Button>
        </div>
      </div>

      {tab === "entities" && <EntitiesPanel />}

      {tab === "overview" && (
        <div className="space-y-4">
          <MemoryOverviewPanel
            ov={overview}
            onWhy={async (traceId) => {
              try {
                setWhy(await fetchWhyRecalled(traceId))
                setWhyOpen(true)
              } catch (e) {
                toast.error(String((e as Error).message ?? e))
              }
            }}
          />

          {whyOpen && why && (
            <Card className="border-sky-500/30 bg-sky-500/5">
              <CardContent className="pt-4">
                <div className="flex items-center justify-between mb-2">
                  <div className="text-xs uppercase tracking-wide text-sky-400">
                    why recalled — "{why.query}"
                  </div>
                  <Button size="sm" variant="ghost" className="h-6 px-2 text-xs" onClick={() => setWhyOpen(false)}>
                    close
                  </Button>
                </div>
                <div className="text-[11px] text-muted-foreground mb-2">{why.reason}</div>
                {why.items.length === 0 ? (
                  <div className="text-sm text-muted-foreground italic">no items recorded for this turn</div>
                ) : (
                  <ul className="space-y-1 text-sm">
                    {why.items.map((it, i) => (
                      <li key={i} className="leading-snug">
                        <Badge variant="secondary" className="text-[10px] mr-1.5">{it.kind || "claim"}</Badge>
                        {it.text}
                      </li>
                    ))}
                  </ul>
                )}
              </CardContent>
            </Card>
          )}

          {agentJournal.length > 0 && (
            <Card>
              <CardContent className="pt-4">
                <div className="text-xs uppercase tracking-wide text-muted-foreground mb-2">
                  agent journal (her self-reflections)
                </div>
                <ul className="space-y-2">
                  {agentJournal.map((a, i) => (
                    <li key={i} className="text-sm leading-snug">
                      <span className="text-[11px] font-mono text-muted-foreground mr-2">{(a.at || "").slice(0, 10)}</span>
                      {a.body}
                    </li>
                  ))}
                </ul>
              </CardContent>
            </Card>
          )}

          {quarantine.length > 0 && (
            <Card className="border-red-500/30 bg-red-500/5">
              <CardContent className="pt-4">
                <div className="text-xs uppercase tracking-wide text-red-400 mb-2">
                  quarantine (blocked as injection) · {quarantine.length}
                </div>
                <ul className="space-y-1 text-xs font-mono text-muted-foreground">
                  {quarantine.map((q, i) => (
                    <li key={i} className="leading-snug truncate">
                      {(q.reason || "")} — {q.payload.slice(0, 80)}
                    </li>
                  ))}
                </ul>
              </CardContent>
            </Card>
          )}
        </div>
      )}

      {!enabled && (
        <Alert>
          <AlertTitle>Durable memory not enabled</AlertTitle>
          <AlertDescription>This synth is not running the facts/journal layer yet.</AlertDescription>
        </Alert>
      )}
      {err && (
        <Alert variant="destructive">
          <AlertTitle>Couldn't load memory</AlertTitle>
          <AlertDescription>{err}</AlertDescription>
        </Alert>
      )}

      {tab === "facts" && (
        <>
          {profile.length > 0 && (
            <Card className="border-primary/30 bg-primary/5">
              <CardContent className="pt-5">
                <div className="text-xs uppercase tracking-wide text-muted-foreground mb-2">Who she knows you as</div>
                <ul className="space-y-1.5">
                  {profile.slice(0, 12).map((f) => (
                    <li key={f.id} className="text-sm leading-snug">
                      • {f.text}
                    </li>
                  ))}
                </ul>
              </CardContent>
            </Card>
          )}

          <div className="flex flex-wrap items-center gap-2">
            <Badge
              variant={kind === "" ? "default" : "outline"}
              className="cursor-pointer"
              onClick={() => setKind("")}
            >
              all · {total}
            </Badge>
            {kindChips.map(([k, n]) => (
              <Badge
                key={k}
                variant={kind === k ? "default" : "outline"}
                className="cursor-pointer"
                onClick={() => setKind(kind === k ? "" : k)}
              >
                {kindLabel(k)} · {n}
              </Badge>
            ))}
          </div>

          <div className="flex gap-2">
            <Input
              placeholder="Search facts…"
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") void loadFacts()
              }}
            />
            <Button variant="outline" onClick={() => void loadFacts()} disabled={busy}>
              Search
            </Button>
          </div>

          <div className="space-y-2">
            {facts?.map((f) => (
              <Card key={f.id} className="group">
                <CardContent className="py-3">
                  {editingId === f.claim_id ? (
                    <div className="flex gap-2">
                      <Input
                        value={editText}
                        onChange={(e) => setEditText(e.target.value)}
                        onKeyDown={(e) => {
                          if (e.key === "Enter") void doEditSave(f)
                          if (e.key === "Escape") setEditingId(null)
                        }}
                        autoFocus
                      />
                      <Button size="sm" onClick={() => void doEditSave(f)}>Save</Button>
                      <Button size="sm" variant="ghost" onClick={() => setEditingId(null)}>Cancel</Button>
                    </div>
                  ) : (
                    <div className="flex items-start gap-2">
                      <div className="text-sm leading-snug flex-1">{f.text}</div>
                      {f.claim_id && (
                        <div className="flex gap-1 opacity-0 group-hover:opacity-100 transition shrink-0">
                          <Button
                            size="sm"
                            variant="ghost"
                            className="h-6 px-2 text-xs"
                            onClick={() => {
                              setEditingId(f.claim_id!)
                              setEditText(f.text)
                            }}
                          >
                            edit
                          </Button>
                          <Button
                            size="sm"
                            variant="ghost"
                            className="h-6 px-2 text-xs text-orange-400 hover:text-orange-300"
                            onClick={() => void doForget(f)}
                          >
                            forget
                          </Button>
                        </div>
                      )}
                    </div>
                  )}
                  <div className="mt-1.5 flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
                    <Badge variant="secondary">{kindLabel(f.kind)}</Badge>
                    {f.occurred_at && <span>{f.occurred_at}</span>}
                    {f.project_id && <span>· project</span>}
                    {f.confidence > 0 && <span>· {Math.round(f.confidence * 100)}%</span>}
                    <span className="ml-auto opacity-60">{f.source}</span>
                  </div>
                </CardContent>
              </Card>
            ))}
            {facts && facts.length === 0 && (
              <p className="text-sm text-muted-foreground">No facts match.</p>
            )}
          </div>
        </>
      )}

      {tab === "journal" && (
        <div className="space-y-3">
          {journal?.map((j) => (
            <Card key={j.id}>
              <CardContent className="py-3">
                <div className="flex items-baseline gap-2">
                  <span className="text-xs font-mono text-muted-foreground">
                    {j.happened_on || j.created_at?.slice(0, 10)}
                  </span>
                  {j.title && <span className="text-sm font-medium">{j.title}</span>}
                </div>
                {j.body && <p className="mt-1 text-sm leading-snug text-muted-foreground">{j.body}</p>}
              </CardContent>
            </Card>
          ))}
          {journal && journal.length === 0 && (
            <p className="text-sm text-muted-foreground">No journal entries yet.</p>
          )}
        </div>
      )}
    </div>
  )
}

// MemoryOverviewPanel — Wave 4.1 dashboard: counts, kind/predicate distribution,
// potential conflicts, and recent memory activity (audit trail).
function MemoryOverviewPanel({
  ov,
  onWhy,
}: {
  ov: MemoryOverview | null
  onWhy: (traceId: string) => void
}) {
  if (!ov) return <p className="text-sm text-muted-foreground">Loading overview…</p>
  if (!ov.enabled)
    return (
      <Alert>
        <AlertTitle>Memory Engine v2 not enabled</AlertTitle>
        <AlertDescription>This synth runs the v1 memory layer; the overview is v2-only.</AlertDescription>
      </Alert>
    )
  const stat = (label: string, val: number | undefined, accent?: string) => (
    <Card>
      <CardContent className="py-3">
        <div className={"text-xl font-semibold tabular-nums " + (accent ?? "")}>{val ?? 0}</div>
        <div className="text-[11px] uppercase tracking-wide text-muted-foreground">{label}</div>
      </CardContent>
    </Card>
  )
  return (
    <div className="space-y-4">
      <div className="grid grid-cols-2 sm:grid-cols-4 gap-2">
        {stat("claims (live)", ov.claims?.live)}
        {stat("entities", ov.entities?.live)}
        {stat("pages", ov.pages)}
        {stat("journal", ov.journal)}
        {stat("forgotten claims", ov.claims?.invalidated, ov.claims?.invalidated ? "text-orange-400" : undefined)}
        {stat("superseded", ov.claims?.superseded)}
        {stat("staging", ov.staging_pending)}
        {stat("quarantine", ov.quarantine, ov.quarantine ? "text-red-400" : undefined)}
      </div>

      {(ov.kinds?.length ?? 0) > 0 && (
        <Card>
          <CardContent className="pt-4">
            <div className="text-xs uppercase tracking-wide text-muted-foreground mb-2">claim kinds</div>
            <div className="flex flex-wrap gap-1.5">
              {ov.kinds!.map((k) => (
                <Badge key={k.key} variant="secondary" className="text-[11px]">
                  {kindLabel(k.key)} · {k.n}
                </Badge>
              ))}
            </div>
          </CardContent>
        </Card>
      )}

      {(ov.conflicts?.length ?? 0) > 0 && (
        <Card className="border-orange-500/30 bg-orange-500/5">
          <CardContent className="pt-4">
            <div className="text-xs uppercase tracking-wide text-orange-400 mb-2">
              potential conflicts ({ov.conflicts!.length})
            </div>
            <ul className="space-y-2">
              {ov.conflicts!.map((c, i) => (
                <li key={i} className="text-sm">
                  <span className="font-mono text-[11px] text-muted-foreground">{c.predicate}</span>
                  <ul className="mt-0.5 ml-3 list-disc list-inside text-muted-foreground">
                    {c.claims.map((t, j) => (
                      <li key={j} className="leading-snug">{t}</li>
                    ))}
                  </ul>
                </li>
              ))}
            </ul>
          </CardContent>
        </Card>
      )}

      {(ov.recent?.length ?? 0) > 0 && (
        <Card>
          <CardContent className="pt-4">
            <div className="text-xs uppercase tracking-wide text-muted-foreground mb-2">recent activity</div>
            <ul className="space-y-1 font-mono text-[11px]">
              {ov.recent!.map((t, i) => {
                const clickable = t.event === "recall" && !!t.id
                return (
                  <li
                    key={i}
                    className={"flex gap-2 " + (clickable ? "cursor-pointer hover:text-foreground" : "")}
                    onClick={() => clickable && onWhy(t.id!)}
                    title={clickable ? "why recalled — show injected memory" : ""}
                  >
                    <span className="text-muted-foreground shrink-0">{(t.at || "").slice(0, 16).replace("T", " ")}</span>
                    <span className="text-foreground shrink-0">{t.event}{clickable ? " 🔍" : ""}</span>
                    <span className="text-muted-foreground truncate">
                      {t.target_type}
                      {t.query ? ` "${t.query.slice(0, 40)}"` : ""}
                      {t.reason ? ` — ${t.reason}` : ""}
                    </span>
                  </li>
                )
              })}
            </ul>
          </CardContent>
        </Card>
      )}
    </div>
  )
}

// EntitiesPanel — §7.3 entity hygiene: merge fragmented duplicates (e.g.
// "Sasha"/"Aleksandr"/"Александр" as three nodes) into one survivor, and edit an
// entity's canonical name + aliases. Lists entities from the v2 graph export.
// Split is API-only (it needs a per-claim picker) — surfaced as a note.
function EntitiesPanel() {
  const [nodes, setNodes] = useState<GraphV2Node[] | null>(null)
  const [err, setErr] = useState<string | null>(null)
  const [q, setQ] = useState("")
  const [sel, setSel] = useState<Set<string>>(new Set())
  const [survivor, setSurvivor] = useState<string | null>(null)
  const [editing, setEditing] = useState<string | null>(null)
  const [editName, setEditName] = useState("")
  const [editAliases, setEditAliases] = useState("")
  const [busy, setBusy] = useState(false)

  const load = async () => {
    setErr(null)
    try {
      const ex = await fetchGraphExport()
      setNodes((ex.nodes ?? []).slice().sort((a, b) => b.mention_count - a.mention_count))
    } catch (e) {
      setErr(String((e as Error).message ?? e))
    }
  }
  useEffect(() => {
    void load()
  }, [])

  const shown = useMemo(() => {
    const all = nodes ?? []
    const t = q.trim().toLowerCase()
    const f = t
      ? all.filter(
          (n) => n.label.toLowerCase().includes(t) || (n.aliases ?? []).some((a) => a.toLowerCase().includes(t)),
        )
      : all
    return f.slice(0, 200)
  }, [nodes, q])

  const toggle = (id: string) =>
    setSel((prev) => {
      const next = new Set(prev)
      if (next.has(id)) {
        next.delete(id)
        if (survivor === id) setSurvivor(null)
      } else {
        next.add(id)
      }
      return next
    })

  const markSurvivor = (id: string) => {
    setSel((prev) => new Set(prev).add(id))
    setSurvivor(id)
  }

  const doMerge = async () => {
    if (!survivor) {
      toast.error("Mark one entity as the survivor (★) first")
      return
    }
    const dups = [...sel].filter((id) => id !== survivor)
    if (dups.length === 0) {
      toast.error("Select at least one duplicate to merge in")
      return
    }
    setBusy(true)
    try {
      const r = await mergeEntities(survivor, dups)
      if (!r.ok) throw new Error(`HTTP ${r.status}`)
      const b = r.body
      toast.success(`Merged ${dups.length} → ${b?.claims_rebound ?? 0} claims, ${b?.relations_rebound ?? 0} relations rebound`)
      setSel(new Set())
      setSurvivor(null)
      await load()
    } catch (e) {
      toast.error(String((e as Error).message ?? e))
    } finally {
      setBusy(false)
    }
  }

  const startEdit = (n: GraphV2Node) => {
    setEditing(n.id)
    setEditName(n.label)
    setEditAliases((n.aliases ?? []).join(", "))
  }
  const saveEdit = async () => {
    if (!editing) return
    setBusy(true)
    try {
      const aliases = editAliases.split(",").map((s) => s.trim()).filter(Boolean)
      const r = await editEntity(editing, editName.trim(), aliases)
      if (!r.ok) throw new Error(`HTTP ${r.status}`)
      toast.success("Entity updated")
      setEditing(null)
      await load()
    } catch (e) {
      toast.error(String((e as Error).message ?? e))
    } finally {
      setBusy(false)
    }
  }

  const survivorLabel = nodes?.find((n) => n.id === survivor)?.label ?? ""

  return (
    <div className="space-y-3">
      {err && (
        <Alert variant="destructive">
          <AlertTitle>Couldn't load entities</AlertTitle>
          <AlertDescription>{err}</AlertDescription>
        </Alert>
      )}
      <div className="flex items-center gap-2 flex-wrap">
        <Input
          placeholder="filter entities…"
          value={q}
          onChange={(e) => setQ(e.target.value)}
          className="max-w-xs h-8"
        />
        <span className="text-xs text-muted-foreground">{nodes ? `${nodes.length} entities` : "loading…"}</span>
        {sel.size > 0 && (
          <div className="ml-auto flex items-center gap-2">
            <span className="text-xs text-muted-foreground">
              {sel.size} selected{survivor ? ` · survivor: ${survivorLabel}` : ""}
            </span>
            <Button size="sm" disabled={busy || !survivor || sel.size < 2} onClick={doMerge}>
              Merge into survivor
            </Button>
            <Button
              size="sm"
              variant="ghost"
              onClick={() => {
                setSel(new Set())
                setSurvivor(null)
              }}
            >
              clear
            </Button>
          </div>
        )}
      </div>
      <p className="text-[11px] text-muted-foreground">
        Check duplicates, mark one as the survivor (★), then Merge — claims/relations rebind to the survivor and the
        dups archive (audit-safe, never deleted). Split is API-only for now (needs a per-claim picker).
      </p>
      <ul className="space-y-1.5">
        {shown.map((n) => (
          <li key={n.id} className="rounded-md border border-border/60 px-3 py-2">
            {editing === n.id ? (
              <div className="space-y-2">
                <Input value={editName} onChange={(e) => setEditName(e.target.value)} placeholder="canonical name" className="h-8" />
                <Input
                  value={editAliases}
                  onChange={(e) => setEditAliases(e.target.value)}
                  placeholder="aliases, comma separated"
                  className="h-8"
                />
                <div className="flex gap-2">
                  <Button size="sm" disabled={busy} onClick={saveEdit}>
                    Save
                  </Button>
                  <Button size="sm" variant="ghost" onClick={() => setEditing(null)}>
                    Cancel
                  </Button>
                </div>
              </div>
            ) : (
              <div className="flex items-center gap-2">
                <input
                  type="checkbox"
                  checked={sel.has(n.id)}
                  onChange={() => toggle(n.id)}
                  className="accent-primary"
                  aria-label={`select ${n.label}`}
                />
                <button
                  title="mark as survivor"
                  onClick={() => markSurvivor(n.id)}
                  className={"text-base leading-none " + (survivor === n.id ? "text-amber-400" : "text-muted-foreground/30 hover:text-muted-foreground")}
                >
                  ★
                </button>
                <div className="min-w-0 flex-1">
                  <div className="text-sm font-medium truncate">
                    {n.label}
                    <Badge variant="secondary" className="text-[10px] ml-1.5">
                      {n.type}
                    </Badge>
                    <span className="text-[11px] text-muted-foreground ml-1.5">· {n.mention_count}</span>
                  </div>
                  {(n.aliases?.length ?? 0) > 0 && (
                    <div className="text-[11px] text-muted-foreground truncate">aka {n.aliases!.join(", ")}</div>
                  )}
                </div>
                <Button size="sm" variant="ghost" className="h-7 px-2 text-xs" onClick={() => startEdit(n)}>
                  edit
                </Button>
              </div>
            )}
          </li>
        ))}
      </ul>
    </div>
  )
}
