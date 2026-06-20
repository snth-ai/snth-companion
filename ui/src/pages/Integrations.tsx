import { useEffect, useState } from "react"
import { Plug, Loader2, CheckCircle2, Save, Video } from "lucide-react"
import { toast } from "sonner"
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Badge } from "@/components/ui/badge"
import {
  fetchRecallConfig,
  saveRecallConfig,
  type RecallConfig,
} from "@/lib/api"

const DEFAULT_REGION = "https://ap-northeast-1.recall.ai"

export function IntegrationsPage() {
  const [recall, setRecall] = useState<RecallConfig | null>(null)
  const [key, setKey] = useState("")
  const [region, setRegion] = useState(DEFAULT_REGION)
  const [lang, setLang] = useState("auto")
  const [busy, setBusy] = useState(false)
  const [err, setErr] = useState<string | null>(null)

  const load = async () => {
    setErr(null)
    try {
      const c = await fetchRecallConfig()
      setRecall(c)
      if (c.region_host) setRegion(c.region_host)
      if (c.language_code) setLang(c.language_code)
    } catch (e) {
      setErr(String((e as Error).message ?? e))
    }
  }

  useEffect(() => {
    void load()
  }, [])

  const saveRecall = async () => {
    setBusy(true)
    setErr(null)
    try {
      await saveRecallConfig({
        api_key: key.trim(),
        region_host: region.trim(),
        language_code: lang.trim(),
      })
      toast.success("Recall integration saved")
      setKey("")
      await load()
    } catch (e) {
      setErr(String((e as Error).message ?? e))
      toast.error("Save failed")
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="space-y-6">
      <div>
        <h1 className="flex items-center gap-2 text-2xl font-semibold tracking-tight">
          <Plug className="size-6" /> Integrations
        </h1>
        <p className="mt-1 max-w-2xl text-muted-foreground">
          External services your synth uses. Keys are stored on the synth and
          never shown back here.
        </p>
      </div>

      <Card>
        <CardHeader className="pb-3">
          <div className="flex items-center justify-between gap-3">
            <CardTitle className="flex items-center gap-2 text-base">
              <Video className="size-4" /> Recall.ai (meeting bot)
            </CardTitle>
            {recall?.configured ? (
              <Badge variant="secondary" className="gap-1">
                <CheckCircle2 className="size-3.5" /> Configured
              </Badge>
            ) : (
              <Badge variant="outline">Not set</Badge>
            )}
          </div>
          <CardDescription>
            Lets your synth join Google Meet / Zoom calls by link, listen, and
            speak. Get a key from recall.ai and pick the region your key belongs
            to.
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-4">
          <div className="grid gap-1.5">
            <Label htmlFor="recall-key">API key</Label>
            <Input
              id="recall-key"
              type="password"
              placeholder={recall?.configured ? "•••••••• (leave blank to keep)" : "Paste Recall API key"}
              value={key}
              onChange={(e) => setKey(e.target.value)}
              autoComplete="off"
            />
          </div>
          <div className="grid gap-1.5">
            <Label htmlFor="recall-region">Region host</Label>
            <Input
              id="recall-region"
              placeholder={DEFAULT_REGION}
              value={region}
              onChange={(e) => setRegion(e.target.value)}
            />
            <p className="text-xs text-muted-foreground">
              The key is region-bound. Wrong region returns 401. Default:{" "}
              <code>ap-northeast-1</code>.
            </p>
          </div>
          <div className="grid gap-1.5">
            <Label htmlFor="recall-lang">Transcription language</Label>
            <Input
              id="recall-lang"
              placeholder="auto"
              value={lang}
              onChange={(e) => setLang(e.target.value)}
            />
            <p className="text-xs text-muted-foreground">
              <code>auto</code> = multilingual (recommended), or a code like{" "}
              <code>ru</code>. Used by the post-call transcript &amp; cascade STT.
            </p>
          </div>
          <div className="flex items-center gap-3">
            <Button onClick={saveRecall} disabled={busy || (!key.trim() && !region.trim())}>
              {busy ? <Loader2 className="size-4 animate-spin" /> : <Save className="size-4" />}
              Save
            </Button>
            {err ? <p className="text-sm text-destructive">{err}</p> : null}
          </div>
        </CardContent>
      </Card>
    </div>
  )
}
