import { useEffect, useMemo, useState } from "react"
import { Card, CardContent } from "@/components/ui/card"
import { Input } from "@/components/ui/input"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import {
  fetchFacts,
  fetchJournal,
  fetchMemoryOverview,
  type FactItem,
  type JournalItem,
  type MemoryOverview,
} from "@/lib/api"

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
  const [tab, setTab] = useState<"facts" | "journal" | "overview">("facts")

  // overview state (Wave 4.1)
  const [overview, setOverview] = useState<MemoryOverview | null>(null)

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
        </div>
      </div>

      {tab === "overview" && (
        <MemoryOverviewPanel ov={overview} />
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
              <Card key={f.id}>
                <CardContent className="py-3">
                  <div className="text-sm leading-snug">{f.text}</div>
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
function MemoryOverviewPanel({ ov }: { ov: MemoryOverview | null }) {
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
              {ov.recent!.map((t, i) => (
                <li key={i} className="flex gap-2">
                  <span className="text-muted-foreground shrink-0">{(t.at || "").slice(0, 16).replace("T", " ")}</span>
                  <span className="text-foreground shrink-0">{t.event}</span>
                  <span className="text-muted-foreground truncate">
                    {t.target_type}
                    {t.query ? ` "${t.query.slice(0, 40)}"` : ""}
                    {t.reason ? ` — ${t.reason}` : ""}
                  </span>
                </li>
              ))}
            </ul>
          </CardContent>
        </Card>
      )}
    </div>
  )
}
