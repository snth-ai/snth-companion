import { useEffect, useState } from "react"
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { Badge } from "@/components/ui/badge"
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import { fetchDream, fetchDreamList, type DreamPage } from "@/lib/api"

// Dreams — read-only feed of the synth's dreaming output (REM theme
// extraction + Diary narratives, both written by the dreaming
// subsystem). Two-list layout: dreams (daily diaries) on the left,
// themes on the right. Click any to view the page content.

export function DreamsPage() {
  const [dreams, setDreams] = useState<DreamPage[] | null>(null)
  const [themes, setThemes] = useState<DreamPage[] | null>(null)
  const [err, setErr] = useState<string | null>(null)
  const [selected, setSelected] = useState<DreamPage | null>(null)
  const [content, setContent] = useState<string>("")
  const [loadingDetail, setLoadingDetail] = useState(false)

  useEffect(() => {
    void (async () => {
      try {
        const d = await fetchDreamList()
        setDreams(d.dreams ?? [])
        setThemes(d.themes ?? [])
      } catch (e) {
        setErr(String((e as Error).message ?? e))
      }
    })()
  }, [])

  useEffect(() => {
    if (!selected) {
      setContent("")
      return
    }
    setLoadingDetail(true)
    void fetchDream(selected.id)
      .then((d) => setContent(d.content_md ?? ""))
      .catch((e) => setErr(String(e.message ?? e)))
      .finally(() => setLoadingDetail(false))
  }, [selected])

  return (
    <div className="space-y-4">
      <div>
        <h1 className="text-2xl font-semibold tracking-tight">Dreams</h1>
        <p className="text-sm text-muted-foreground mt-1">
          What the synth processes overnight — daily diary narratives and
          recurring themes the dreaming subsystem extracts.
        </p>
      </div>

      {err && (
        <Alert variant="destructive">
          <AlertTitle>Error</AlertTitle>
          <AlertDescription>{err}</AlertDescription>
        </Alert>
      )}

      <div className="grid grid-cols-1 lg:grid-cols-[280px_280px_1fr] gap-4">
        <Card className="max-h-[80vh] overflow-y-auto">
          <CardHeader className="pb-2">
            <CardTitle className="text-sm uppercase tracking-wider text-muted-foreground">
              Diaries{" "}
              <span className="text-muted-foreground/60">({dreams?.length ?? 0})</span>
            </CardTitle>
          </CardHeader>
          <CardContent className="space-y-1">
            {dreams === null && <div className="text-sm text-muted-foreground">loading…</div>}
            {dreams?.length === 0 && <div className="text-sm text-muted-foreground italic">no diaries yet</div>}
            {dreams?.map((d) => (
              <button
                key={d.id}
                onClick={() => setSelected(d)}
                className={
                  "w-full text-left rounded-md px-3 py-2 text-sm transition-colors " +
                  (selected?.id === d.id
                    ? "bg-primary/15 text-foreground"
                    : "hover:bg-muted text-muted-foreground hover:text-foreground")
                }
              >
                <div className="font-medium text-foreground line-clamp-1">{d.title || d.id}</div>
                <div className="text-xs text-muted-foreground/80">{d.id}</div>
              </button>
            ))}
          </CardContent>
        </Card>

        <Card className="max-h-[80vh] overflow-y-auto">
          <CardHeader className="pb-2">
            <CardTitle className="text-sm uppercase tracking-wider text-muted-foreground">
              Themes{" "}
              <span className="text-muted-foreground/60">({themes?.length ?? 0})</span>
            </CardTitle>
          </CardHeader>
          <CardContent className="space-y-1">
            {themes === null && <div className="text-sm text-muted-foreground">loading…</div>}
            {themes?.length === 0 && <div className="text-sm text-muted-foreground italic">no themes yet</div>}
            {themes?.map((t) => (
              <button
                key={t.id}
                onClick={() => setSelected(t)}
                className={
                  "w-full text-left rounded-md px-3 py-2 text-sm transition-colors " +
                  (selected?.id === t.id
                    ? "bg-primary/15 text-foreground"
                    : "hover:bg-muted text-muted-foreground hover:text-foreground")
                }
              >
                <div className="font-medium text-foreground line-clamp-1">{t.title || t.id}</div>
                <div className="text-xs text-muted-foreground/80">{t.id}</div>
              </button>
            ))}
          </CardContent>
        </Card>

        <Card className="min-h-[60vh]">
          <CardHeader>
            <CardTitle className="flex items-center gap-2 flex-wrap">
              {selected ? selected.title || selected.id : "Pick a dream or theme"}
              {selected && <Badge variant="secondary">{selected.type}</Badge>}
            </CardTitle>
          </CardHeader>
          <CardContent>
            {loadingDetail && <div className="text-sm text-muted-foreground">loading…</div>}
            {!loadingDetail && !selected && (
              <div className="text-sm text-muted-foreground italic">
                Select an entry from one of the columns to read the full
                text.
              </div>
            )}
            {selected && content && (
              <pre className="whitespace-pre-wrap text-sm font-sans leading-relaxed bg-muted/30 rounded-md p-4 max-h-[70vh] overflow-y-auto">
                {content}
              </pre>
            )}
          </CardContent>
        </Card>
      </div>
    </div>
  )
}
