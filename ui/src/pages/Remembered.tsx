import { useEffect, useMemo, useState } from "react"
import { Card, CardContent } from "@/components/ui/card"
import { Input } from "@/components/ui/input"
import { Badge } from "@/components/ui/badge"
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import { fetchMemoryList, type MemoryEntry } from "@/lib/api"

// Remembered — flat browser of the synth's memory log (LanceDB or
// SQLite fallback). One row per entry, newest first, filterable by
// category and free-text search. Importance is rendered as a small
// 0-10 chip so high-signal entries stand out.

const categoryLabels: Record<string, string> = {
  fact: "fact",
  reflection: "reflection",
  decision: "decision",
  preference: "pref",
  observation: "obs",
  scar: "scar",
}

export function RememberedPage() {
  const [items, setItems] = useState<MemoryEntry[] | null>(null)
  const [err, setErr] = useState<string | null>(null)
  const [filter, setFilter] = useState("")
  const [category, setCategory] = useState<string>("")

  useEffect(() => {
    void (async () => {
      try {
        const d = await fetchMemoryList(undefined, undefined, 1000)
        setItems(d.memories ?? [])
      } catch (e) {
        setErr(String((e as Error).message ?? e))
      }
    })()
  }, [])

  const visible = useMemo(() => {
    if (!items) return []
    let out = items
    if (category) out = out.filter((m) => m.category === category)
    const q = filter.trim().toLowerCase()
    if (q) out = out.filter((m) => m.text.toLowerCase().includes(q))
    return out
  }, [items, filter, category])

  const counts = useMemo(() => {
    const m = new Map<string, number>()
    for (const it of items ?? []) {
      m.set(it.category, (m.get(it.category) ?? 0) + 1)
    }
    return m
  }, [items])

  return (
    <div className="space-y-4">
      <div className="flex items-end justify-between gap-4 flex-wrap">
        <div>
          <h1 className="text-2xl font-semibold tracking-tight">Remembered</h1>
          <p className="text-sm text-muted-foreground mt-1">
            The synth's memory log — facts, reflections, decisions she chose
            to keep around. Newest first.
          </p>
        </div>
        <Input
          placeholder="Search memories…"
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          className="max-w-sm"
        />
      </div>

      {err && (
        <Alert variant="destructive">
          <AlertTitle>Error</AlertTitle>
          <AlertDescription>{err}</AlertDescription>
        </Alert>
      )}

      <div className="flex items-center gap-2 flex-wrap">
        <button
          onClick={() => setCategory("")}
          className={
            "text-xs px-2 py-1 rounded " +
            (category === "" ? "bg-primary/15 text-foreground" : "text-muted-foreground hover:bg-muted")
          }
        >
          all ({items?.length ?? 0})
        </button>
        {[...counts.entries()].map(([cat, n]) => (
          <button
            key={cat}
            onClick={() => setCategory(cat)}
            className={
              "text-xs px-2 py-1 rounded " +
              (category === cat
                ? "bg-primary/15 text-foreground"
                : "text-muted-foreground hover:bg-muted")
            }
          >
            {categoryLabels[cat] ?? cat} ({n})
          </button>
        ))}
      </div>

      <div className="space-y-2 max-h-[70vh] overflow-y-auto">
        {items === null && (
          <div className="text-sm text-muted-foreground">loading…</div>
        )}
        {visible.length === 0 && items !== null && (
          <div className="text-sm text-muted-foreground italic">
            nothing matches
          </div>
        )}
        {visible.map((m) => (
          <Card key={m.id}>
            <CardContent className="py-3">
              <div className="flex items-center gap-2 mb-1.5 text-xs text-muted-foreground">
                <Badge variant="secondary" className="text-xs">
                  {categoryLabels[m.category] ?? m.category}
                </Badge>
                {m.scope && m.scope !== "default" && (
                  <Badge variant="outline" className="text-xs font-mono">
                    {m.scope}
                  </Badge>
                )}
                <span className="font-mono">{m.created_at}</span>
                <span>·</span>
                <span>imp {Math.round(m.importance * 10) / 10}</span>
              </div>
              <div className="text-sm text-foreground whitespace-pre-wrap">
                {m.text}
              </div>
            </CardContent>
          </Card>
        ))}
      </div>
    </div>
  )
}
