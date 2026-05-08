import { useEffect, useState } from "react"
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import { Switch } from "@/components/ui/switch"
import { Label } from "@/components/ui/label"
import {
  fetchSynthOwnerSettings,
  patchSynthOwnerSettings,
  type SynthOwnerSettings,
} from "@/lib/api"
import { toast } from "sonner"
import { Sparkles, Heart } from "lucide-react"

// SynthSettings — per-session feature toggles for the bound synth's
// owner. Same backend as the Telegram mini-app's Settings tab; the
// companion just calls /api/settings/owner (which auto-resolves
// session_id from the synth's OWNER_TG_ID env).
export function SynthSettingsPage() {
  const [data, setData] = useState<SynthOwnerSettings | null>(null)
  const [err, setErr] = useState<string | null>(null)
  const [saving, setSaving] = useState<string | null>(null)

  const load = async () => {
    try {
      const s = await fetchSynthOwnerSettings()
      setData(s)
      setErr(null)
    } catch (e) {
      setErr(String((e as Error).message ?? e))
    }
  }

  useEffect(() => {
    void load()
  }, [])

  const toggle = async (
    key: "heartbeat" | "emotional_state_enabled",
    next: boolean,
  ) => {
    if (!data) return
    setSaving(key)
    setData({ ...data, [key]: next })
    try {
      await patchSynthOwnerSettings({ [key]: next })
      toast.success(`${humanLabel(key)} ${next ? "on" : "off"}`)
    } catch (e) {
      // Revert on failure.
      setData({ ...data, [key]: !next })
      toast.error(String((e as Error).message ?? e))
    } finally {
      setSaving(null)
    }
  }

  return (
    <div className="space-y-4">
      <div>
        <h1 className="text-2xl font-semibold tracking-tight">Synth Settings</h1>
        <p className="text-sm text-muted-foreground mt-1">
          Per-owner feature toggles for the bound synth. Mirrors the
          Telegram mini-app Settings tab.
        </p>
      </div>

      {err && (
        <Alert variant="destructive">
          <AlertTitle>Can't load settings</AlertTitle>
          <AlertDescription>{err}</AlertDescription>
        </Alert>
      )}

      {!data && !err && (
        <div className="text-sm text-muted-foreground">Loading…</div>
      )}

      {data && (
        <>
          <Card>
            <CardHeader className="pb-3">
              <CardTitle className="flex items-center gap-2 text-base">
                <Heart className="h-4 w-4 text-pink-400" />
                Emotional state machine
              </CardTitle>
            </CardHeader>
            <CardContent className="flex items-start justify-between gap-6">
              <div className="text-sm text-muted-foreground max-w-md">
                Track desire / warmth / hurt / frustration / joy / trust over
                time. Off = flat-tone synth: no per-turn mood injection, no
                <code className="px-1 mx-1 bg-muted rounded text-xs">
                  update_emotions
                </code>
                tool, no decay tick.
              </div>
              <div className="flex items-center gap-2 shrink-0">
                <Switch
                  checked={data.emotional_state_enabled}
                  disabled={saving === "emotional_state_enabled"}
                  onCheckedChange={(v) => void toggle("emotional_state_enabled", v)}
                  id="emo-toggle"
                />
                <Label htmlFor="emo-toggle" className="text-xs">
                  {data.emotional_state_enabled ? "on" : "off"}
                </Label>
              </div>
            </CardContent>
          </Card>

          <Card>
            <CardHeader className="pb-3">
              <CardTitle className="flex items-center gap-2 text-base">
                <Sparkles className="h-4 w-4 text-amber-400" />
                Proactive messages (heartbeat)
              </CardTitle>
            </CardHeader>
            <CardContent className="flex items-start justify-between gap-6">
              <div className="text-sm text-muted-foreground max-w-md">
                Allow the synth to message you first via the heartbeat
                daemon (every 30 min, gated by DND / quiet hours / unanswered-pings).
              </div>
              <div className="flex items-center gap-2 shrink-0">
                <Switch
                  checked={data.heartbeat}
                  disabled={saving === "heartbeat"}
                  onCheckedChange={(v) => void toggle("heartbeat", v)}
                  id="hb-toggle"
                />
                <Label htmlFor="hb-toggle" className="text-xs">
                  {data.heartbeat ? "on" : "off"}
                </Label>
              </div>
            </CardContent>
          </Card>

          <div className="text-[11px] text-muted-foreground/70 font-mono pt-2">
            session_id: {data.session_id} · updated {data.updated_at}
          </div>
        </>
      )}
    </div>
  )
}

function humanLabel(key: string): string {
  switch (key) {
    case "emotional_state_enabled":
      return "Emotional state"
    case "heartbeat":
      return "Heartbeat"
    default:
      return key
  }
}
