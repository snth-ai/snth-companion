import { useEffect, useMemo, useState } from "react"
import { Radio, Loader2, Save, Search, Sparkles } from "lucide-react"
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
import { Switch } from "@/components/ui/switch"
import { Badge } from "@/components/ui/badge"
import {
  fetchRealtimeSettings,
  saveRealtimeSettings,
  type RealtimeSettings,
} from "@/lib/api"

export function RealtimePage() {
  const [data, setData] = useState<RealtimeSettings | null>(null)
  const [enabled, setEnabled] = useState<Set<string>>(new Set())
  const [q, setQ] = useState("")
  const [busy, setBusy] = useState(false)
  const [err, setErr] = useState<string | null>(null)

  const load = async () => {
    setErr(null)
    try {
      const d = await fetchRealtimeSettings()
      setData(d)
      setEnabled(new Set(d.tools.filter((t) => t.enabled).map((t) => t.name)))
    } catch (e) {
      setErr(String((e as Error).message ?? e))
    }
  }

  useEffect(() => {
    void load()
  }, [])

  const toggle = (name: string) =>
    setEnabled((prev) => {
      const next = new Set(prev)
      next.has(name) ? next.delete(name) : next.add(name)
      return next
    })

  const recommendedOnly = () =>
    setEnabled(new Set((data?.tools ?? []).filter((t) => t.default_voice).map((t) => t.name)))

  const save = async () => {
    setBusy(true)
    setErr(null)
    try {
      await saveRealtimeSettings([...enabled])
      toast.success(`Saved — ${enabled.size} tools enabled for realtime`)
      await load()
    } catch (e) {
      setErr(String((e as Error).message ?? e))
      toast.error("Save failed")
    } finally {
      setBusy(false)
    }
  }

  const tools = useMemo(() => {
    const all = data?.tools ?? []
    const needle = q.trim().toLowerCase()
    if (!needle) return all
    return all.filter(
      (t) =>
        t.name.toLowerCase().includes(needle) ||
        t.description.toLowerCase().includes(needle),
    )
  }, [data, q])

  return (
    <div className="space-y-6">
      <div>
        <h1 className="flex items-center gap-2 text-2xl font-semibold tracking-tight">
          <Radio className="size-6" /> Real-Time tools
        </h1>
        <p className="mt-1 max-w-2xl text-muted-foreground">
          Pick which tools your synth may use during live voice calls. Fewer
          tools = lighter, faster, less likely to wander off mid-conversation.
          The full tool set still works in text chat.
        </p>
      </div>

      <Card>
        <CardHeader className="pb-3">
          <div className="flex flex-wrap items-center justify-between gap-3">
            <CardTitle className="text-base">
              Allowed in realtime{" "}
              <Badge variant="secondary" className="ml-1">
                {enabled.size}
              </Badge>
            </CardTitle>
            <div className="flex items-center gap-2">
              <Button variant="outline" size="sm" onClick={recommendedOnly}>
                <Sparkles className="size-3.5" /> Recommended only
              </Button>
              <Button size="sm" onClick={save} disabled={busy || !data}>
                {busy ? <Loader2 className="size-4 animate-spin" /> : <Save className="size-4" />}
                Save
              </Button>
            </div>
          </div>
          <CardDescription>
            {data?.customized
              ? "Custom selection active."
              : "Using the recommended default set (not yet customized)."}
          </CardDescription>
          <div className="relative pt-2">
            <Search className="absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground" />
            <Input
              className="pl-8"
              placeholder="Filter tools…"
              value={q}
              onChange={(e) => setQ(e.target.value)}
            />
          </div>
        </CardHeader>
        <CardContent className="space-y-1">
          {err ? <p className="text-sm text-destructive">{err}</p> : null}
          {!data ? (
            <div className="flex items-center gap-2 py-8 text-muted-foreground">
              <Loader2 className="size-4 animate-spin" /> Loading tools…
            </div>
          ) : (
            tools.map((t) => (
              <div
                key={t.name}
                className="flex items-start justify-between gap-3 rounded-md px-2 py-2 hover:bg-muted/50"
              >
                <div className="min-w-0">
                  <div className="flex items-center gap-2">
                    <span className="font-mono text-sm">{t.name}</span>
                    {t.default_voice ? (
                      <Badge variant="outline" className="h-4 px-1 text-[10px]">
                        recommended
                      </Badge>
                    ) : null}
                  </div>
                  <p className="line-clamp-2 text-xs text-muted-foreground">
                    {t.description}
                  </p>
                </div>
                <Switch
                  checked={enabled.has(t.name)}
                  onCheckedChange={() => toggle(t.name)}
                />
              </div>
            ))
          )}
        </CardContent>
      </Card>
    </div>
  )
}
