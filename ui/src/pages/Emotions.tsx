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
  saveAppraisalModel,
  type AppraisalModelSetting,
  type EmotionalOverview,
} from "@/lib/api"
import { toast } from "sonner"

// Emotions — read-only window into the synth's Emotional Engine v2
// (event-sourced state, object-level valence, scars) plus the one
// runtime setting that lives here: the appraisal model.
//
// Deliberately label-only: the synth renders mood + undertones in its
// own language server-side; numeric axes are never shown anywhere a
// non-operator can see them (iron rule). The hub admin panel is the
// numbers/debug surface, not this card.

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

export function EmotionsPage() {
  const [overview, setOverview] = useState<EmotionalOverview | null>(null)
  const [err, setErr] = useState<string | null>(null)
  const [notEnabled, setNotEnabled] = useState(false)
  const [session, setSession] = useState("") // empty = synth's owner default
  const [sessionDraft, setSessionDraft] = useState("")

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
            How the synth feels right now — and what left a mark. Feelings
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
