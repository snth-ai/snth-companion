import { useEffect, useMemo, useState } from "react"
import { Card, CardContent } from "@/components/ui/card"
import { Input } from "@/components/ui/input"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import {
  fetchFacts,
  fetchJournal,
  type FactItem,
  type JournalItem,
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
  const [tab, setTab] = useState<"facts" | "journal">("facts")

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
        </div>
      </div>

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
