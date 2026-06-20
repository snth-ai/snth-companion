import { useEffect, useMemo, useState } from "react"
import { Radio, Loader2, Save, Search, Sparkles, Mic, Cpu, Zap } from "lucide-react"
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
import { Switch } from "@/components/ui/switch"
import { Badge } from "@/components/ui/badge"
import { cn } from "@/lib/utils"
import {
  fetchRealtimeSettings,
  saveRealtimeSettings,
  fetchCallSettings,
  saveCallSettings,
  type RealtimeSettings,
  type CallSettings,
} from "@/lib/api"

const ENGINES: { id: string; label: string; hint: string; icon: typeof Mic }[] = [
  { id: "convai", label: "ElevenLabs", hint: "Her real voice + her brain. Natural turn-taking. Higher latency.", icon: Mic },
  { id: "openai", label: "GPT Realtime", hint: "OpenAI voice + GPT brain. Lowest latency, native barge-in.", icon: Zap },
  { id: "cascade", label: "Cascade", hint: "Our own STT → brain → TTS. Fallback / debug.", icon: Cpu },
]

export function RealtimePage() {
  const [call, setCall] = useState<CallSettings | null>(null)
  const [tools, setTools] = useState<RealtimeSettings | null>(null)
  const [enabled, setEnabled] = useState<Set<string>>(new Set())
  const [q, setQ] = useState("")
  const [busy, setBusy] = useState(false)
  const [err, setErr] = useState<string | null>(null)

  // editable call settings
  const [engine, setEngine] = useState("convai")
  const [speak, setSpeak] = useState(true)
  const [agentId, setAgentId] = useState("")
  const [oaVoice, setOaVoice] = useState("marin")
  const [oaModel, setOaModel] = useState("gpt-realtime-2")
  const [brainModel, setBrainModel] = useState("")
  const [brainEffort, setBrainEffort] = useState("")
  const [brainFast, setBrainFast] = useState(false)

  const load = async () => {
    setErr(null)
    try {
      const [c, t] = await Promise.all([fetchCallSettings(), fetchRealtimeSettings()])
      setCall(c)
      setEngine(c.engine || "convai")
      setSpeak(c.speak)
      setAgentId(c.convai_agent_id || "")
      setOaVoice(c.openai_voice || "marin")
      setOaModel(c.openai_model || "gpt-realtime-2")
      setBrainModel(c.brain_model || "")
      setBrainEffort(c.brain_effort || "")
      setBrainFast(!!c.brain_fast)
      setTools(t)
      setEnabled(new Set(t.tools.filter((x) => x.enabled).map((x) => x.name)))
    } catch (e) {
      setErr(String((e as Error).message ?? e))
    }
  }

  useEffect(() => {
    void load()
  }, [])

  const saveCall = async () => {
    setBusy(true)
    setErr(null)
    try {
      await saveCallSettings({
        engine,
        speak,
        convai_agent_id: agentId.trim(),
        openai_voice: oaVoice.trim(),
        openai_model: oaModel.trim(),
        brain_model: brainModel.trim(),
        brain_effort: brainEffort,
        brain_fast: brainFast,
      })
      toast.success("Call settings saved — applies on the next call")
      await load()
    } catch (e) {
      setErr(String((e as Error).message ?? e))
      toast.error("Save failed")
    } finally {
      setBusy(false)
    }
  }

  const toggle = (name: string) =>
    setEnabled((prev) => {
      const next = new Set(prev)
      next.has(name) ? next.delete(name) : next.add(name)
      return next
    })

  const recommendedOnly = () =>
    setEnabled(new Set((tools?.tools ?? []).filter((t) => t.default_voice).map((t) => t.name)))

  const saveTools = async () => {
    setBusy(true)
    try {
      await saveRealtimeSettings([...enabled])
      toast.success(`Saved — ${enabled.size} tools enabled`)
      await load()
    } catch (e) {
      toast.error("Save failed: " + String((e as Error).message ?? e))
    } finally {
      setBusy(false)
    }
  }

  const filteredTools = useMemo(() => {
    const all = tools?.tools ?? []
    const needle = q.trim().toLowerCase()
    if (!needle) return all
    return all.filter(
      (t) => t.name.toLowerCase().includes(needle) || t.description.toLowerCase().includes(needle),
    )
  }, [tools, q])

  return (
    <div className="space-y-6">
      <div>
        <h1 className="flex items-center gap-2 text-2xl font-semibold tracking-tight">
          <Radio className="size-6" /> Real-Time calls
        </h1>
        <p className="mt-1 max-w-2xl text-muted-foreground">
          How your synth behaves in live voice calls: which engine drives the
          voice, whether she may speak, and which tools she can use. Changes
          apply on the next call — no restart.
        </p>
      </div>

      {/* Engine + speak */}
      <Card>
        <CardHeader className="pb-3">
          <CardTitle className="text-base">Voice engine</CardTitle>
          <CardDescription>Pick what powers the call voice loop.</CardDescription>
        </CardHeader>
        <CardContent className="space-y-4">
          <div className="grid gap-2 sm:grid-cols-3">
            {ENGINES.map((e) => {
              const Icon = e.icon
              const active = engine === e.id
              return (
                <button
                  key={e.id}
                  onClick={() => setEngine(e.id)}
                  className={cn(
                    "rounded-lg border p-3 text-left transition-colors",
                    active ? "border-primary bg-primary/5" : "border-border hover:bg-muted/50",
                  )}
                >
                  <div className="flex items-center gap-2 font-medium">
                    <Icon className="size-4" /> {e.label}
                    {active ? <Badge className="ml-auto h-4 px-1 text-[10px]">active</Badge> : null}
                  </div>
                  <p className="mt-1 text-xs text-muted-foreground">{e.hint}</p>
                </button>
              )
            })}
          </div>

          <div className="flex items-center justify-between rounded-md border px-3 py-2">
            <div>
              <div className="font-medium">Let her speak in calls</div>
              <p className="text-xs text-muted-foreground">
                Off = she joins &amp; listens only (post-call recap), never speaks.
              </p>
            </div>
            <Switch checked={speak} onCheckedChange={setSpeak} />
          </div>

          {/* Call brain model — applies to engines that use her brain (convai/cascade) */}
          {engine !== "openai" ? (
            <div className="grid gap-1.5">
              <Label htmlFor="brain">Call brain model</Label>
              <Input
                id="brain"
                placeholder="main (or provider:model, e.g. openrouter:google/gemini-2.5-flash)"
                value={brainModel}
                onChange={(e) => setBrainModel(e.target.value)}
              />
              <p className="text-xs text-muted-foreground">
                The model that thinks for her in calls. <code>main</code> = your chat
                model. A faster model (e.g. <code>codex:gpt-5.5</code>) cuts per-turn
                latency. GPT Realtime uses its own model (above), not this.
              </p>

              {/* effort + speed (apply to a specific brain model, not "main") */}
              <div className="flex flex-wrap items-center gap-4 pt-1">
                <div className="flex items-center gap-1.5">
                  <span className="text-xs text-muted-foreground">Effort</span>
                  {["", "minimal", "low", "medium", "high"].map((e) => (
                    <button
                      key={e || "auto"}
                      onClick={() => setBrainEffort(e)}
                      className={cn(
                        "rounded px-2 py-0.5 text-xs border transition-colors",
                        brainEffort === e ? "border-primary bg-primary/10" : "border-border hover:bg-muted/50",
                      )}
                    >
                      {e || "auto"}
                    </button>
                  ))}
                </div>
                <label className="flex items-center gap-2 text-xs">
                  <Switch checked={brainFast} onCheckedChange={setBrainFast} />
                  <span>Fast (accelerated tier)</span>
                </label>
              </div>
              <p className="text-xs text-muted-foreground">
                Effort + Fast apply to a specific brain model (e.g. <code>codex:gpt-5.5</code> +
                low + fast), not <code>main</code>. Lower effort = faster.
              </p>
            </div>
          ) : null}

          {/* Engine-specific config */}
          {engine === "convai" ? (
            <div className="grid gap-1.5">
              <Label htmlFor="agent">ElevenLabs agent ID</Label>
              <Input id="agent" placeholder="agent_..." value={agentId} onChange={(e) => setAgentId(e.target.value)} />
              <p className="text-xs text-muted-foreground">
                The ConvAI agent wired to her custom-LLM brain.{" "}
                {call?.elevenlabs_key_set ? "ElevenLabs key: set." : "⚠ ElevenLabs key not set."}{" "}
                Voice: <code>{call?.elevenlabs_voice || "default"}</code>.
              </p>
            </div>
          ) : null}

          {engine === "openai" ? (
            <div className="grid gap-3 sm:grid-cols-2">
              <div className="grid gap-1.5">
                <Label htmlFor="oav">OpenAI voice</Label>
                <Input id="oav" placeholder="marin" value={oaVoice} onChange={(e) => setOaVoice(e.target.value)} />
              </div>
              <div className="grid gap-1.5">
                <Label htmlFor="oam">Model</Label>
                <Input id="oam" placeholder="gpt-realtime-2" value={oaModel} onChange={(e) => setOaModel(e.target.value)} />
              </div>
              <p className="text-xs text-muted-foreground sm:col-span-2">
                {call?.openai_key_set ? "OpenAI key: set." : "⚠ OpenAI key not set."} Note: GPT
                Realtime uses OpenAI's voice, not her ElevenLabs voice.
              </p>
            </div>
          ) : null}

          <div className="flex items-center gap-3">
            <Button onClick={saveCall} disabled={busy}>
              {busy ? <Loader2 className="size-4 animate-spin" /> : <Save className="size-4" />}
              Save engine settings
            </Button>
            {err ? <p className="text-sm text-destructive">{err}</p> : null}
          </div>
        </CardContent>
      </Card>

      {/* Tool picker */}
      <Card>
        <CardHeader className="pb-3">
          <div className="flex flex-wrap items-center justify-between gap-3">
            <CardTitle className="text-base">
              Tools allowed in calls{" "}
              <Badge variant="secondary" className="ml-1">{enabled.size}</Badge>
            </CardTitle>
            <div className="flex items-center gap-2">
              <Button variant="outline" size="sm" onClick={recommendedOnly}>
                <Sparkles className="size-3.5" /> Recommended only
              </Button>
              <Button size="sm" onClick={saveTools} disabled={busy || !tools}>
                {busy ? <Loader2 className="size-4 animate-spin" /> : <Save className="size-4" />}
                Save tools
              </Button>
            </div>
          </div>
          <CardDescription>
            Fewer tools = lighter &amp; faster. The full set still works in text chat.
          </CardDescription>
          <div className="relative pt-2">
            <Search className="absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground" />
            <Input className="pl-8" placeholder="Filter tools…" value={q} onChange={(e) => setQ(e.target.value)} />
          </div>
        </CardHeader>
        <CardContent className="space-y-1">
          {!tools ? (
            <div className="flex items-center gap-2 py-8 text-muted-foreground">
              <Loader2 className="size-4 animate-spin" /> Loading tools…
            </div>
          ) : (
            filteredTools.map((t) => (
              <div key={t.name} className="flex items-start justify-between gap-3 rounded-md px-2 py-2 hover:bg-muted/50">
                <div className="min-w-0">
                  <div className="flex items-center gap-2">
                    <span className="font-mono text-sm">{t.name}</span>
                    {t.default_voice ? (
                      <Badge variant="outline" className="h-4 px-1 text-[10px]">recommended</Badge>
                    ) : null}
                  </div>
                  <p className="line-clamp-2 text-xs text-muted-foreground">{t.description}</p>
                </div>
                <Switch checked={enabled.has(t.name)} onCheckedChange={() => toggle(t.name)} />
              </div>
            ))
          )}
        </CardContent>
      </Card>
    </div>
  )
}
