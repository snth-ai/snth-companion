import { useEffect, useState } from "react"
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Textarea } from "@/components/ui/textarea"
import { Switch } from "@/components/ui/switch"
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import {
  fetchChannelSettings,
  saveChannelSettings,
  type ChannelSettings,
} from "@/lib/api"

export function ChannelsPage() {
  const [cs, setCs] = useState<ChannelSettings | null>(null)
  const [ownerMapText, setOwnerMapText] = useState("")
  const [err, setErr] = useState<string | null>(null)
  const [saved, setSaved] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  const reload = async () => {
    setErr(null)
    try {
      const d = await fetchChannelSettings()
      setCs(d)
      setOwnerMapText(formatOwnerMap(d.instagram_owner_map ?? {}))
    } catch (e) {
      setErr(String(e))
    }
  }

  useEffect(() => {
    reload()
  }, [])

  if (err && !cs) {
    return (
      <Alert variant="destructive">
        <AlertTitle>Can't load channel settings</AlertTitle>
        <AlertDescription>{err}</AlertDescription>
      </Alert>
    )
  }
  if (!cs) {
    return <div className="text-sm text-muted-foreground">Loading…</div>
  }

  const patch = (p: Partial<ChannelSettings>) =>
    setCs((prev) => (prev ? { ...prev, ...p } : prev))

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-2xl font-semibold tracking-tight">Channels</h1>
        <p className="text-sm text-muted-foreground mt-1">
          Instagram + WhatsApp per-synth. Saved to hub DB; env pushed to
          container via node-agent. Owner-map changes don't restart —
          hub reads fresh on every webhook.
        </p>
      </div>

      {err && (
        <Alert variant="destructive">
          <AlertTitle>Error</AlertTitle>
          <AlertDescription>{err}</AlertDescription>
        </Alert>
      )}
      {saved && (
        <Alert>
          <AlertTitle>Saved</AlertTitle>
          <AlertDescription>{saved}</AlertDescription>
        </Alert>
      )}

      <Card>
        <CardHeader>
          <CardTitle>Instagram</CardTitle>
        </CardHeader>
        <CardContent className="space-y-4">
          <Toggle
            label="Enabled"
            value={cs.instagram_enabled}
            onChange={(v) => patch({ instagram_enabled: v })}
          />
          <Toggle
            label="Read-only (archive inbound, don't send)"
            value={cs.instagram_read_only}
            onChange={(v) => patch({ instagram_read_only: v })}
          />
          <div>
            <Label>
              Owner map{" "}
              <span className="text-muted-foreground font-normal">
                — one per line, <code className="font-mono">IGSID = session_id</code>
              </span>
            </Label>
            <Textarea
              value={ownerMapText}
              onChange={(e) => setOwnerMapText(e.target.value)}
              placeholder="1477095863960189 = tg_7392742"
              rows={4}
              className="font-mono text-xs"
            />
          </div>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>WhatsApp</CardTitle>
        </CardHeader>
        <CardContent className="space-y-4">
          <Toggle
            label="Enabled"
            value={cs.whatsapp_enabled}
            onChange={(v) => patch({ whatsapp_enabled: v })}
          />
          <Toggle
            label="Read-only"
            value={cs.whatsapp_read_only}
            onChange={(v) => patch({ whatsapp_read_only: v })}
          />
          <div>
            <Label>
              Proxy{" "}
              <span className="text-muted-foreground font-normal">
                — empty = hub residential pool,{" "}
                <code className="font-mono">off</code> = direct (Hetzner IP)
              </span>
            </Label>
            <Input
              value={cs.whatsapp_proxy}
              onChange={(e) => patch({ whatsapp_proxy: e.target.value })}
              placeholder="off, http://…, socks5://…"
              className="font-mono text-xs"
            />
          </div>
        </CardContent>
      </Card>

      <div className="flex items-center gap-3">
        <Button
          disabled={busy}
          onClick={async () => {
            setBusy(true)
            setErr(null)
            setSaved(null)
            try {
              const payload: Partial<ChannelSettings> = {
                ...cs,
                instagram_owner_map: parseOwnerMap(ownerMapText),
              }
              const r = (await saveChannelSettings(payload)) as {
                ok?: boolean
                env_push_error?: string
              }
              if (r.env_push_error) {
                setErr("Saved to DB, but env push to node failed: " + r.env_push_error)
              } else {
                setSaved("Channel settings saved + pushed to container.")
              }
              await reload()
            } catch (e) {
              setErr(String(e))
            } finally {
              setBusy(false)
            }
          }}
        >
          {busy ? "Saving…" : "Save & push to synth"}
        </Button>
      </div>
    </div>
  )
}

function Toggle({
  label,
  value,
  onChange,
}: {
  label: string
  value: boolean
  onChange: (v: boolean) => void
}) {
  return (
    <div className="flex items-center gap-3">
      <Switch checked={value} onCheckedChange={onChange} />
      <span className="text-sm">{label}</span>
    </div>
  )
}

function formatOwnerMap(m: Record<string, string>): string {
  const keys = Object.keys(m).sort()
  return keys.map((k) => `${k} = ${m[k]}`).join("\n")
}

function parseOwnerMap(raw: string): Record<string, string> {
  const out: Record<string, string> = {}
  for (const line of raw.split("\n")) {
    const trimmed = line.trim()
    if (!trimmed || trimmed.startsWith("#")) continue
    let idx = trimmed.indexOf("=")
    if (idx < 0) idx = trimmed.indexOf(":")
    if (idx <= 0) continue
    const igsid = trimmed.slice(0, idx).trim()
    const sess = trimmed.slice(idx + 1).trim()
    if (igsid && sess) out[igsid] = sess
  }
  return out
}
