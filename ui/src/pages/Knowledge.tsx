import { useEffect, useMemo, useState } from "react"
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { Input } from "@/components/ui/input"
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Trash2 } from "lucide-react"
import {
  deleteWikiPage,
  fetchWikiList,
  fetchWikiPage,
  type WikiPageDetail,
  type WikiPageLite,
} from "@/lib/api"
import { toast } from "sonner"

// Knowledge — read-only browser for the bound synth's wiki pages.
// Two-pane: list on the left, full content on the right when a page
// is picked. Markdown is rendered as <pre> for the MVP — we can add
// a real renderer later if it turns out we want it.

export function KnowledgePage() {
  const [pages, setPages] = useState<WikiPageLite[] | null>(null)
  const [err, setErr] = useState<string | null>(null)
  const [filter, setFilter] = useState("")
  const [selected, setSelected] = useState<string | null>(null)
  const [detail, setDetail] = useState<WikiPageDetail | null>(null)
  const [loadingDetail, setLoadingDetail] = useState(false)

  useEffect(() => {
    void (async () => {
      try {
        const d = await fetchWikiList(undefined, undefined, 500)
        setPages(d.pages ?? [])
      } catch (e) {
        setErr(String((e as Error).message ?? e))
      }
    })()
  }, [])

  useEffect(() => {
    if (!selected) {
      setDetail(null)
      return
    }
    setLoadingDetail(true)
    void fetchWikiPage(selected)
      .then(setDetail)
      .catch((e) => setErr(String(e.message ?? e)))
      .finally(() => setLoadingDetail(false))
  }, [selected])

  const grouped = useMemo(() => {
    if (!pages) return new Map<string, WikiPageLite[]>()
    const q = filter.trim().toLowerCase()
    const visible = q
      ? pages.filter(
          (p) =>
            p.title.toLowerCase().includes(q) ||
            p.id.toLowerCase().includes(q) ||
            p.snippet?.toLowerCase().includes(q),
        )
      : pages
    const m = new Map<string, WikiPageLite[]>()
    for (const p of visible) {
      const key = p.type || "other"
      const arr = m.get(key) ?? []
      arr.push(p)
      m.set(key, arr)
    }
    return m
  }, [pages, filter])

  return (
    <div className="space-y-4">
      <div className="flex items-end justify-between gap-4 flex-wrap">
        <div>
          <h1 className="text-2xl font-semibold tracking-tight">Knowledge</h1>
          <p className="text-sm text-muted-foreground mt-1">
            Wiki pages your synth has authored — daily journals, themes,
            decisions, references.
          </p>
        </div>
        <Input
          placeholder="Filter by title or content…"
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

      <div className="grid grid-cols-1 lg:grid-cols-[320px_1fr] gap-4">
        <div className="space-y-3 max-h-[80vh] overflow-y-auto pr-2">
          {pages === null && (
            <div className="text-sm text-muted-foreground">loading…</div>
          )}
          {pages !== null && pages.length === 0 && (
            <div className="text-sm text-muted-foreground italic">
              no pages yet
            </div>
          )}
          {[...grouped.entries()].map(([type, list]) => (
            <div key={type}>
              <div className="text-xs uppercase tracking-wider text-muted-foreground mb-1 px-1">
                {type}{" "}
                <span className="text-muted-foreground/60">({list.length})</span>
              </div>
              <div className="space-y-1">
                {list.map((p) => (
                  <button
                    key={p.id}
                    onClick={() => setSelected(p.id)}
                    className={
                      "w-full text-left rounded-md px-3 py-2 text-sm transition-colors " +
                      (selected === p.id
                        ? "bg-primary/15 text-foreground"
                        : "hover:bg-muted text-muted-foreground hover:text-foreground")
                    }
                  >
                    <div className="font-medium text-foreground line-clamp-1">
                      {p.title || p.id}
                    </div>
                    <div className="text-xs text-muted-foreground/80 line-clamp-1">
                      {p.id} · {p.bytes}b
                    </div>
                  </button>
                ))}
              </div>
            </div>
          ))}
        </div>

        <Card className="min-h-[60vh]">
          <CardHeader>
            <CardTitle className="flex items-center gap-2 flex-wrap">
              {detail ? detail.title || detail.id : "Pick a page"}
              {detail && <Badge variant="secondary">{detail.type}</Badge>}
              {detail?.namespace && (
                <Badge variant="outline">{detail.namespace}</Badge>
              )}
              {detail && (
                <Button
                  variant="ghost"
                  size="sm"
                  className="ml-auto text-red-400 hover:text-red-300"
                  onClick={async () => {
                    if (!confirm(`Delete "${detail.title || detail.id}"?`)) return
                    try {
                      await deleteWikiPage(detail.id)
                      toast.success("page deleted")
                      setSelected(null)
                      const d = await fetchWikiList(undefined, undefined, 500)
                      setPages(d.pages ?? [])
                    } catch (e) {
                      toast.error(String((e as Error).message ?? e))
                    }
                  }}
                >
                  <Trash2 className="h-4 w-4 mr-1" /> Delete
                </Button>
              )}
            </CardTitle>
          </CardHeader>
          <CardContent>
            {loadingDetail && (
              <div className="text-sm text-muted-foreground">loading…</div>
            )}
            {!loadingDetail && !detail && (
              <div className="text-sm text-muted-foreground italic">
                Select a page from the list to view its content.
              </div>
            )}
            {detail && (
              <div className="space-y-3">
                <div className="text-xs text-muted-foreground font-mono">
                  id: {detail.id} · updated {detail.updated_at}
                </div>
                {detail.links_out && detail.links_out.length > 0 && (
                  <div className="text-xs text-muted-foreground">
                    links out:{" "}
                    {detail.links_out.map((l) => (
                      <button
                        key={l.page_id}
                        onClick={() => setSelected(l.page_id)}
                        className="underline text-primary hover:text-primary/80 mr-2"
                      >
                        {l.title || l.page_id}
                      </button>
                    ))}
                  </div>
                )}
                {detail.links_in && detail.links_in.length > 0 && (
                  <div className="text-xs text-muted-foreground">
                    referenced by:{" "}
                    {detail.links_in.map((l) => (
                      <button
                        key={l.page_id}
                        onClick={() => setSelected(l.page_id)}
                        className="underline text-primary hover:text-primary/80 mr-2"
                      >
                        {l.title || l.page_id}
                      </button>
                    ))}
                  </div>
                )}
                <pre className="whitespace-pre-wrap text-sm font-sans leading-relaxed bg-muted/30 rounded-md p-4 max-h-[70vh] overflow-y-auto">
                  {detail.content}
                </pre>
              </div>
            )}
          </CardContent>
        </Card>
      </div>
    </div>
  )
}
