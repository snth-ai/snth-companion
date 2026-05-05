import { useEffect, useState } from "react"
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { Button } from "@/components/ui/button"
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import { ChevronLeft, FolderIcon, FileIcon } from "lucide-react"
import { fetchMediaList, mediaFileURL, type MediaItem } from "@/lib/api"

// Library — files browser for workspace/media on the bound synth.
// Folder navigation + click-through previews for images and audio /
// video. Anything else falls through to a download link.

function isImage(mime?: string) {
  return mime?.startsWith("image/") ?? false
}
function isAudio(mime?: string) {
  return mime?.startsWith("audio/") ?? false
}
function isVideo(mime?: string) {
  return mime?.startsWith("video/") ?? false
}

function formatBytes(n: number) {
  if (n < 1024) return `${n}B`
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)}KB`
  if (n < 1024 * 1024 * 1024) return `${(n / 1024 / 1024).toFixed(1)}MB`
  return `${(n / 1024 / 1024 / 1024).toFixed(2)}GB`
}

export function LibraryPage() {
  const [dir, setDir] = useState(".")
  const [items, setItems] = useState<MediaItem[] | null>(null)
  const [err, setErr] = useState<string | null>(null)
  const [selected, setSelected] = useState<MediaItem | null>(null)

  useEffect(() => {
    setItems(null)
    setSelected(null)
    void (async () => {
      try {
        const d = await fetchMediaList(dir === "." ? undefined : dir)
        setItems(d.items ?? [])
      } catch (e) {
        setErr(String((e as Error).message ?? e))
      }
    })()
  }, [dir])

  const goUp = () => {
    if (dir === "." || !dir) return
    const parts = dir.split("/").filter(Boolean)
    parts.pop()
    setDir(parts.length ? parts.join("/") : ".")
  }

  return (
    <div className="space-y-4">
      <div>
        <h1 className="text-2xl font-semibold tracking-tight">Library</h1>
        <p className="text-sm text-muted-foreground mt-1">
          Files in your synth's <code>workspace/media/</code> — images,
          audio, video, anything she's saved or generated.
        </p>
      </div>

      {err && (
        <Alert variant="destructive">
          <AlertTitle>Error</AlertTitle>
          <AlertDescription>{err}</AlertDescription>
        </Alert>
      )}

      <div className="flex items-center gap-2 text-sm">
        <Button
          variant="ghost"
          size="sm"
          onClick={goUp}
          disabled={dir === "."}
        >
          <ChevronLeft className="h-4 w-4 mr-1" /> up
        </Button>
        <code className="font-mono text-muted-foreground">
          media/{dir === "." ? "" : dir}
        </code>
      </div>

      <div className="grid grid-cols-1 lg:grid-cols-[320px_1fr] gap-4">
        <div className="space-y-1 max-h-[75vh] overflow-y-auto pr-2">
          {items === null && (
            <div className="text-sm text-muted-foreground">loading…</div>
          )}
          {items?.length === 0 && (
            <div className="text-sm text-muted-foreground italic">empty</div>
          )}
          {items?.map((it) => (
            <button
              key={it.path}
              onClick={() => {
                if (it.is_dir) setDir(it.path)
                else setSelected(it)
              }}
              className={
                "w-full text-left rounded-md px-3 py-2 flex items-center gap-2 text-sm transition-colors " +
                (selected?.path === it.path
                  ? "bg-primary/15 text-foreground"
                  : "hover:bg-muted text-muted-foreground hover:text-foreground")
              }
            >
              {it.is_dir ? (
                <FolderIcon className="h-4 w-4 shrink-0" />
              ) : (
                <FileIcon className="h-4 w-4 shrink-0" />
              )}
              <span className="flex-1 truncate text-foreground">{it.name}</span>
              {!it.is_dir && (
                <span className="text-xs text-muted-foreground shrink-0">
                  {formatBytes(it.size)}
                </span>
              )}
            </button>
          ))}
        </div>

        <Card className="min-h-[60vh]">
          <CardHeader>
            <CardTitle className="flex items-center gap-2 flex-wrap text-sm">
              {selected ? selected.name : "Pick a file"}
              {selected && (
                <span className="text-xs text-muted-foreground font-mono">
                  {selected.mime ?? "?"} · {formatBytes(selected.size)}
                </span>
              )}
            </CardTitle>
          </CardHeader>
          <CardContent>
            {!selected && (
              <div className="text-sm text-muted-foreground italic">
                Select a file from the list to preview.
              </div>
            )}
            {selected && (
              <div className="space-y-3">
                <div className="text-xs text-muted-foreground font-mono">
                  {selected.path} · {selected.mod_time}
                </div>
                {isImage(selected.mime) && (
                  <img
                    src={mediaFileURL(selected.path)}
                    alt={selected.name}
                    className="max-w-full max-h-[70vh] rounded-md border border-border"
                  />
                )}
                {isAudio(selected.mime) && (
                  <audio
                    controls
                    src={mediaFileURL(selected.path)}
                    className="w-full"
                  />
                )}
                {isVideo(selected.mime) && (
                  <video
                    controls
                    src={mediaFileURL(selected.path)}
                    className="max-w-full max-h-[70vh] rounded-md"
                  />
                )}
                {!isImage(selected.mime) &&
                  !isAudio(selected.mime) &&
                  !isVideo(selected.mime) && (
                    <a
                      href={mediaFileURL(selected.path)}
                      target="_blank"
                      rel="noreferrer"
                      className="text-sm text-primary underline"
                    >
                      open / download
                    </a>
                  )}
              </div>
            )}
          </CardContent>
        </Card>
      </div>
    </div>
  )
}
