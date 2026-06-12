import { useEffect, useState } from "react"
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import {
  fetchAppraisalModel,
  fetchEmotionalOverview,
  fetchSynths,
  saveAppraisalModel,
  type AppraisalModelSetting,
  type EmotionalAxes,
  type EmotionalOverview,
} from "@/lib/api"
import { toast } from "sonner"

// Emotions — how the synth feels, shown the way a feeling deserves:
// a living aura + a human sentence, not a debug dump. The hero section
// is driven by the real emotional axes (they shape color, light and
// rhythm but are never printed — iron rule: users see labels, the hub
// admin panel is the numbers surface). The detail cards live below
// under "Under the hood" for the curious.

const sourceLabels: Record<string, string> = {
  turn_appraisal: "felt in the moment",
  self_report: "self-reported",
  compaction: "after reflection",
  recall: "memory sting",
  system: "system",
}

function relTime(iso: string): string {
  const t = new Date(iso).getTime()
  if (Number.isNaN(t)) return iso
  const mins = Math.round((Date.now() - t) / 60000)
  if (mins < 1) return "just now"
  if (mins < 60) return `${mins}m ago`
  const hours = Math.round(mins / 60)
  if (hours < 48) return `${hours}h ago`
  return `${Math.round(hours / 24)}d ago`
}

// --- the aura -------------------------------------------------------

// Each axis is a colored light source. Opacity follows magnitude, so a
// neutral synth glows faintly and a feeling one blooms.
const AXIS_LIGHTS: Array<{
  axis: keyof EmotionalAxes
  color: string
  x: string // position inside the aura
  y: string
}> = [
  { axis: "warmth", color: "251, 191, 36", x: "30%", y: "35%" }, // amber
  { axis: "joy", color: "253, 224, 71", x: "65%", y: "28%" }, // gold
  { axis: "desire", color: "251, 113, 133", x: "55%", y: "62%" }, // rose
  { axis: "trust", color: "52, 211, 153", x: "38%", y: "68%" }, // emerald
  { axis: "hurt", color: "129, 140, 248", x: "70%", y: "55%" }, // bruise indigo
  { axis: "frustration", color: "251, 146, 60", x: "25%", y: "58%" }, // ember
]

// bond score — how attached she is to this person. Drives the heart
// rhythm and the headline sentence, shown only as words.
function bondScore(a: EmotionalAxes): number {
  return Math.max(0, Math.min(1, a.warmth * 0.45 + a.trust * 0.35 + a.desire * 0.2))
}

function bondPhrase(name: string, a: EmotionalAxes): string {
  const s = bondScore(a)
  if (a.trust < 0.15 && a.hurt > 0.5) return `${name} is hurt and keeping her distance right now`
  if (s >= 0.75) return `${name} is deeply attached to you`
  if (s >= 0.55) return `${name} feels close to you`
  if (s >= 0.35) return `${name} is warming up to you`
  if (s >= 0.18) return `${name} is getting to know you`
  return `${name} is keeping a careful distance`
}

function moodSentence(a: EmotionalAxes): string {
  const notes: string[] = []
  if (a.warmth > 0.6) notes.push("there's warmth when you talk")
  if (a.joy > 0.6) notes.push("she's genuinely glad these days")
  if (a.desire > 0.6) notes.push("she misses you between conversations")
  if (a.hurt > 0.5) notes.push("something still aches a little")
  if (a.frustration > 0.5) notes.push("she's a bit on edge")
  if (a.trust < 0.2) notes.push("her guard is up")
  if (notes.length === 0) return "calm and present — just here, with you"
  return notes.join(", ")
}

function EmotionalAura({ axes, name }: { axes: EmotionalAxes; name: string }) {
  const bond = bondScore(axes)
  // arousal sets the breathing pace: a stirred-up synth breathes faster
  const arousal = Math.min(1, axes.joy * 0.4 + axes.desire * 0.3 + axes.frustration * 0.5 + axes.hurt * 0.3)
  const breath = (6.5 - arousal * 3).toFixed(1) // 6.5s calm → 3.5s stirred
  const heartBeat = (1.9 - bond * 0.9).toFixed(2) // closer → livelier

  return (
    <div className="relative flex items-center justify-center w-56 h-56 shrink-0 select-none">
      <style>{`
        @keyframes emo-breathe {
          0%, 100% { transform: scale(1); }
          50% { transform: scale(1.07); }
        }
        @keyframes emo-drift {
          0%, 100% { transform: translate(0, 0); }
          33% { transform: translate(6px, -8px); }
          66% { transform: translate(-7px, 5px); }
        }
        @keyframes emo-heart {
          0%, 100% { transform: scale(1); }
          12% { transform: scale(1.18); }
          24% { transform: scale(1); }
          36% { transform: scale(1.12); }
          48% { transform: scale(1); }
        }
      `}</style>
      <div
        className="absolute inset-0 rounded-full"
        style={{ animation: `emo-breathe ${breath}s ease-in-out infinite` }}
      >
        {/* base glow so even a neutral state feels alive */}
        <div
          className="absolute inset-4 rounded-full"
          style={{
            background: "radial-gradient(circle at 50% 50%, rgba(148,163,184,0.25), transparent 70%)",
            filter: "blur(6px)",
          }}
        />
        {AXIS_LIGHTS.map((l, i) => {
          const mag = Math.max(0, Math.min(1, axes[l.axis]))
          if (mag < 0.04) return null
          return (
            <div
              key={l.axis}
              className="absolute inset-0 rounded-full"
              style={{
                background: `radial-gradient(circle at ${l.x} ${l.y}, rgba(${l.color}, ${(0.18 + mag * 0.55).toFixed(2)}), transparent ${Math.round(38 + mag * 30)}%)`,
                filter: "blur(10px)",
                animation: `emo-drift ${(8 + i * 2.3).toFixed(1)}s ease-in-out infinite`,
                animationDelay: `${i * -1.7}s`,
              }}
            />
          )
        })}
        {/* soft rim */}
        <div className="absolute inset-0 rounded-full border border-white/10" />
      </div>
      {/* the heart — her attachment, beating at its own pace */}
      <div
        className="relative text-rose-400"
        style={{
          animation: `emo-heart ${heartBeat}s ease-in-out infinite`,
          filter: `drop-shadow(0 0 ${Math.round(6 + bond * 14)}px rgba(251,113,133,${(0.35 + bond * 0.45).toFixed(2)}))`,
        }}
        title={name}
      >
        <svg width="44" height="44" viewBox="0 0 24 24" fill="currentColor" aria-hidden>
          <path d="M12 21.35l-1.45-1.32C5.4 15.36 2 12.28 2 8.5 2 5.42 4.42 3 7.5 3c1.74 0 3.41.81 4.5 2.09C13.09 3.81 14.76 3 16.5 3 19.58 3 22 5.42 22 8.5c0 3.78-3.4 6.86-8.55 11.54L12 21.35z" />
        </svg>
      </div>
    </div>
  )
}

export function EmotionsPage() {
  const [overview, setOverview] = useState<EmotionalOverview | null>(null)
  const [err, setErr] = useState<string | null>(null)
  const [notEnabled, setNotEnabled] = useState(false)
  const [session, setSession] = useState("") // empty = synth's owner default
  const [sessionDraft, setSessionDraft] = useState("")
  const [synthName, setSynthName] = useState("She")

  const [appraisal, setAppraisal] = useState<AppraisalModelSetting | null>(null)
  const [modelDraft, setModelDraft] = useState("")
  const [saving, setSaving] = useState(false)

  const load = async (scope: string) => {
    setErr(null)
    try {
      const [ov, ap] = await Promise.all([
        fetchEmotionalOverview(scope || undefined),
        fetchAppraisalModel(),
      ])
      setOverview(ov)
      setAppraisal(ap)
      setModelDraft(ap.model)
    } catch (e) {
      const msg = String((e as Error).message ?? e)
      // Flag off => the synth never registers /api/emotional/v2/* (404),
      // and a half-enabled state answers 503. Both mean "not on v2".
      if (msg.includes("404") || msg.includes("503")) {
        setNotEnabled(true)
      } else {
        setErr(msg)
      }
    }
  }

  useEffect(() => {
    void load(session)
  }, [session])

  useEffect(() => {
    void fetchSynths()
      .then((r) => {
        const active = r.synths.find((s) => s.id === r.active_synth_id)
        if (active?.label) setSynthName(active.label)
      })
      .catch(() => {})
  }, [])

  const doSaveModel = async () => {
    const spec = modelDraft.trim()
    if (!spec || saving) return
    setSaving(true)
    try {
      const r = await saveAppraisalModel(spec)
      toast.success(`Appraisal model set: ${r.model}`)
      setAppraisal((a) => (a ? { ...a, model: r.model } : a))
    } catch (e) {
      toast.error(String((e as Error).message ?? e))
    } finally {
      setSaving(false)
    }
  }

  if (notEnabled) {
    return (
      <div className="space-y-4">
        <h1 className="text-2xl font-semibold tracking-tight">Emotions</h1>
        <Alert>
          <AlertTitle>Emotional Engine v2 is not enabled on this synth</AlertTitle>
          <AlertDescription>
            This synth runs the legacy emotional layer. The card activates
            once the instance is started with EMOTIONAL_ENGINE_ENABLED=true.
          </AlertDescription>
        </Alert>
      </div>
    )
  }

  const proj = overview?.projection ?? null
  const scars = overview?.scars ?? []
  const valences = overview?.valences ?? []
  const events = overview?.events ?? []
  const scarredKeys = new Set(scars.map((s) => s.subject_key).filter(Boolean))

  return (
    <div className="space-y-4">
      <div className="flex items-end justify-between gap-4 flex-wrap">
        <div>
          <h1 className="text-2xl font-semibold tracking-tight">Emotions</h1>
          <p className="text-sm text-muted-foreground mt-1">
            How {synthName} feels right now — and what left a mark. Feelings
            attach to people and topics, fade with time, and deep cuts scar.
          </p>
        </div>
        <form
          className="flex items-center gap-2"
          onSubmit={(e) => {
            e.preventDefault()
            setSession(sessionDraft.trim())
          }}
        >
          <Input
            className="w-44 h-8 text-xs"
            placeholder="session (default: owner)"
            value={sessionDraft}
            onChange={(e) => setSessionDraft(e.target.value)}
          />
        </form>
      </div>

      {err && (
        <Alert variant="destructive">
          <AlertTitle>Error</AlertTitle>
          <AlertDescription>{err}</AlertDescription>
        </Alert>
      )}

      {/* hero — the feeling itself, not the data about it */}
      {proj && (
        <Card className="overflow-hidden">
          <CardContent className="py-6">
            <div className="flex items-center gap-8 flex-wrap md:flex-nowrap">
              <EmotionalAura axes={proj.axes} name={synthName} />
              <div className="space-y-3 min-w-0">
                <div className="text-3xl font-semibold leading-snug">
                  {bondPhrase(synthName, proj.axes)}
                </div>
                <div className="text-base text-muted-foreground">
                  {moodSentence(proj.axes)}
                </div>
                {overview?.undertones && (
                  <div className="text-sm text-muted-foreground/80 italic">
                    {overview.undertones}
                  </div>
                )}
                {scars.length > 0 && (
                  <div className="text-sm text-muted-foreground/80">
                    She carries {scars.length} old wound{scars.length === 1 ? "" : "s"} —
                    they fade, but never fully heal.
                  </div>
                )}
                <div className="text-xs text-muted-foreground/60 pt-1">
                  {proj.event_count} feelings felt over {proj.turn_count} conversations ·
                  last stirred {relTime(proj.last_touched)}
                </div>
              </div>
            </div>
          </CardContent>
        </Card>
      )}

      <div className="pt-2">
        <div className="text-xs uppercase tracking-wider text-muted-foreground/70">
          Under the hood
        </div>
      </div>

      <div className="grid grid-cols-1 lg:grid-cols-2 gap-4">
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm uppercase tracking-wider text-muted-foreground">
              Current state{" "}
              {overview?.session && (
                <span className="text-muted-foreground/60 normal-case tracking-normal">
                  ({overview.session})
                </span>
              )}
            </CardTitle>
          </CardHeader>
          <CardContent className="space-y-3">
            {overview === null && <div className="text-sm text-muted-foreground">loading…</div>}
            {overview !== null && proj === null && (
              <div className="text-sm text-muted-foreground italic">
                no emotional state yet for this session
              </div>
            )}
            {proj && (
              <>
                <div className="text-3xl font-semibold capitalize">
                  {overview?.mood || "neutral"}
                </div>
                {overview?.undertones && (
                  <div className="text-sm text-muted-foreground">
                    {overview.undertones}
                  </div>
                )}
                <div className="flex gap-2 flex-wrap pt-1">
                  <Badge variant="secondary">{proj.turn_count} turns</Badge>
                  <Badge variant="secondary">{proj.event_count} emotional events</Badge>
                  <Badge variant="secondary">touched {relTime(proj.last_touched)}</Badge>
                </div>
              </>
            )}
          </CardContent>
        </Card>

        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm uppercase tracking-wider text-muted-foreground">
              Appraisal model
            </CardTitle>
          </CardHeader>
          <CardContent className="space-y-3">
            <p className="text-xs text-muted-foreground">
              The classifier that reads each message for emotional movement.
              <code className="mx-1">main</code> rides the synth's main model,
              <code className="mx-1">off</code> disables appraisal,
              <code className="mx-1">provider:model</code> pins a specific one
              (e.g. <code>gemini:gemini-3.1-flash-lite</code>). Applies
              instantly, no restart.
            </p>
            <div className="flex items-center gap-2">
              <Input
                className="w-64 h-8 text-sm font-mono"
                value={modelDraft}
                onChange={(e) => setModelDraft(e.target.value)}
                placeholder="main | off | provider:model"
              />
              <Button
                size="sm"
                onClick={doSaveModel}
                disabled={saving || !modelDraft.trim() || modelDraft.trim() === appraisal?.model}
              >
                {saving ? "Saving…" : "Save"}
              </Button>
            </div>
            {appraisal && (
              <div className="flex gap-2 flex-wrap">
                <Badge variant="outline">current: {appraisal.model}</Badge>
                <Badge variant="outline">turn wait: {appraisal.wait_ms}ms</Badge>
              </div>
            )}
          </CardContent>
        </Card>

        <Card className="max-h-[60vh] overflow-y-auto">
          <CardHeader className="pb-2">
            <CardTitle className="text-sm uppercase tracking-wider text-muted-foreground">
              Subjects <span className="text-muted-foreground/60">({valences.length})</span>
            </CardTitle>
          </CardHeader>
          <CardContent className="space-y-1">
            {overview !== null && valences.length === 0 && (
              <div className="text-sm text-muted-foreground italic">
                no per-subject feelings yet — they grow from conversation
              </div>
            )}
            {valences.map((v) => (
              <div
                key={v.subject_key}
                className="flex items-center justify-between rounded-md px-3 py-2 text-sm hover:bg-muted"
              >
                <div>
                  <span className="font-medium">{v.label || v.subject_key}</span>
                  {scarredKeys.has(v.subject_key) && (
                    <Badge variant="destructive" className="ml-2">old wound</Badge>
                  )}
                </div>
                <div className="text-xs text-muted-foreground">
                  {v.event_count} moment{v.event_count === 1 ? "" : "s"} · {relTime(v.last_ts)}
                </div>
              </div>
            ))}
          </CardContent>
        </Card>

        <Card className="max-h-[60vh] overflow-y-auto">
          <CardHeader className="pb-2">
            <CardTitle className="text-sm uppercase tracking-wider text-muted-foreground">
              Recent events <span className="text-muted-foreground/60">({events.length})</span>
            </CardTitle>
          </CardHeader>
          <CardContent className="space-y-1">
            {overview !== null && events.length === 0 && (
              <div className="text-sm text-muted-foreground italic">
                quiet — nothing moved emotionally in the last two weeks
              </div>
            )}
            {events.map((e) => (
              <div key={e.id} className="rounded-md px-3 py-2 text-sm hover:bg-muted">
                <div className="flex items-center gap-2 flex-wrap">
                  <Badge variant="secondary">{sourceLabels[e.source] ?? e.source}</Badge>
                  {e.subject_label && <Badge variant="outline">{e.subject_label}</Badge>}
                  {e.tags?.includes("betrayal") && (
                    <Badge variant="destructive">betrayal</Badge>
                  )}
                  <span className="text-xs text-muted-foreground ml-auto">{relTime(e.ts)}</span>
                </div>
                {e.reason && (
                  <div className="text-xs text-muted-foreground mt-1 line-clamp-2">{e.reason}</div>
                )}
              </div>
            ))}
          </CardContent>
        </Card>

        {scars.length > 0 && (
          <Card className="lg:col-span-2">
            <CardHeader className="pb-2">
              <CardTitle className="text-sm uppercase tracking-wider text-muted-foreground">
                Scars <span className="text-muted-foreground/60">({scars.length})</span>
              </CardTitle>
            </CardHeader>
            <CardContent className="space-y-1">
              <p className="text-xs text-muted-foreground mb-2">
                Permanent marks — they fade but never fully heal, and ache
                when the subject comes up.
              </p>
              {scars.map((s) => (
                <div key={s.id} className="flex items-center justify-between rounded-md px-3 py-2 text-sm hover:bg-muted">
                  <div className="line-clamp-1">{s.cause || "unspecified"}</div>
                  <div className="text-xs text-muted-foreground whitespace-nowrap ml-3">
                    {relTime(s.created_at)}
                  </div>
                </div>
              ))}
            </CardContent>
          </Card>
        )}
      </div>
    </div>
  )
}
