import { useEffect, useMemo, useState } from "react"
import { Card, CardContent } from "@/components/ui/card"
import { Input } from "@/components/ui/input"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import { Trash2 } from "lucide-react"
import { deleteMemory, fetchMemoryList, type MemoryEntry } from "@/lib/api"
import { toast } from "sonner"

const PAGE_SIZE = 100

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
  const [offset, setOffset] = useState(0)
  const [total, setTotal] = useState(0)
  const [filteredTotal, setFilteredTotal] = useState(0)
  const [counts, setCounts] = useState<Record<string, number>>({})
  const [busy, setBusy] = useState(false)

  const load = async () => {
    setBusy(true)
    try {
      const d = await fetchMemoryList({
        category: category || undefined,
        offset,
        limit: PAGE_SIZE,
      })
      setItems(d.memories ?? [])
      setTotal(d.total)
      setFilteredTotal(d.filtered_total)
      setCounts(d.categories ?? {})
      setErr(null)
    } catch (e) {
      setErr(String((e as Error).message ?? e))
    } finally {
      setBusy(false)
    }
  }

  // Reset offset when category changes; reload on offset/category change.
  useEffect(() => {
    setOffset(0)
  }, [category])

  useEffect(() => {
    void load()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [offset, category])

  const visible = useMemo(() => {
    if (!items) return []
    const q = filter.trim().toLowerCase()
    if (!q) return items
    return items.filter((m) => m.text.toLowerCase().includes(q))
  }, [items, filter])

  const handleDelete = async (id: string) => {
    if (!confirm("Delete this memory?")) return
    try {
      await deleteMemory(id)
      toast.success("memory deleted")
      void load()
    } catch (e) {
      toast.error(String((e as Error).message ?? e))
    }
  }

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
          all ({total})
        </button>
        {Object.entries(counts).map(([cat, n]) => (
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
        <div className="ml-auto text-xs text-muted-foreground">
          showing {Math.min(offset + 1, filteredTotal)}–
          {Math.min(offset + (items?.length ?? 0), filteredTotal)} of{" "}
          {filteredTotal}
          {category && filteredTotal !== total && (
            <span className="text-muted-foreground/60"> (in category)</span>
          )}
        </div>
        <Button
          variant="outline"
          size="sm"
          disabled={busy || offset === 0}
          onClick={() => setOffset(Math.max(0, offset - PAGE_SIZE))}
        >
          ←
        </Button>
        <Button
          variant="outline"
          size="sm"
          disabled={busy || offset + PAGE_SIZE >= filteredTotal}
          onClick={() => setOffset(offset + PAGE_SIZE)}
        >
          →
        </Button>
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
                <Button
                  variant="ghost"
                  size="sm"
                  className="ml-auto h-6 px-2 text-red-400 hover:text-red-300"
                  onClick={() => handleDelete(m.id)}
                >
                  <Trash2 className="h-3.5 w-3.5" />
                </Button>
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
