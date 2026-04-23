import { useEffect, useState } from "react"
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import {
  claimPair,
  savePair,
  unpair,
  fetchStatus,
  type StatusResponse,
} from "@/lib/api"

export function PairPage() {
  const [status, setStatus] = useState<StatusResponse | null>(null)
  const [err, setErr] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  const reload = async () => {
    try {
      setStatus(await fetchStatus())
    } catch (e) {
      setErr(String(e))
    }
  }

  useEffect(() => {
    reload()
  }, [])

  const paired = status?.paired ?? false

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-2xl font-semibold tracking-tight">Pair</h1>
        <p className="text-sm text-muted-foreground mt-1">
          Connect this companion to a synth. Use the 6-digit code from{" "}
          <code className="font-mono text-xs">/pair_companion</code> in
          Telegram, or paste credentials manually for debug.
        </p>
      </div>

      {err && (
        <Alert variant="destructive">
          <AlertTitle>Error</AlertTitle>
          <AlertDescription>{err}</AlertDescription>
        </Alert>
      )}

      {paired && status && (
        <Card>
          <CardHeader>
            <CardTitle>Currently paired</CardTitle>
          </CardHeader>
          <CardContent className="space-y-3 text-sm">
            <Row label="Synth ID" value={status.synth_id} mono />
            <Row label="Synth URL" value={status.synth_url} mono />
            <Button
              variant="destructive"
              size="sm"
              disabled={busy}
              onClick={async () => {
                setBusy(true)
                setErr(null)
                try {
                  await unpair()
                  await reload()
                } catch (e) {
                  setErr(String(e))
                } finally {
                  setBusy(false)
                }
              }}
            >
              {busy ? "Unpairing…" : "Unpair"}
            </Button>
          </CardContent>
        </Card>
      )}

      <PairClaimCard onDone={reload} setErr={setErr} />
      <PairManualCard onDone={reload} setErr={setErr} />
    </div>
  )
}

function PairClaimCard({
  onDone,
  setErr,
}: {
  onDone: () => void
  setErr: (s: string | null) => void
}) {
  const [code, setCode] = useState("")
  const [hubUrl, setHubUrl] = useState("https://hub.snth.ai")
  const [busy, setBusy] = useState(false)
  return (
    <Card>
      <CardHeader>
        <CardTitle>Pair with a 6-digit code</CardTitle>
      </CardHeader>
      <CardContent className="space-y-4">
        <p className="text-sm text-muted-foreground">
          In Telegram, send <code className="font-mono">/pair_companion</code>{" "}
          to your synth's bot. It replies with a 6-digit code valid for 5
          minutes.
        </p>
        <div className="grid grid-cols-2 gap-3">
          <div>
            <Label htmlFor="pair-code">Code</Label>
            <Input
              id="pair-code"
              value={code}
              onChange={(e) => setCode(e.target.value)}
              placeholder="123456"
              pattern="[0-9 ]+"
              maxLength={10}
              className="font-mono text-lg tracking-widest text-center"
            />
          </div>
          <div>
            <Label htmlFor="pair-hub">Hub URL</Label>
            <Input
              id="pair-hub"
              value={hubUrl}
              onChange={(e) => setHubUrl(e.target.value)}
            />
          </div>
        </div>
        <Button
          disabled={busy || code.replace(/\D/g, "").length !== 6}
          onClick={async () => {
            setBusy(true)
            setErr(null)
            try {
              await claimPair(code, hubUrl)
              setCode("")
              onDone()
            } catch (e) {
              setErr(String(e))
            } finally {
              setBusy(false)
            }
          }}
        >
          {busy ? "Claiming…" : "Claim & pair"}
        </Button>
      </CardContent>
    </Card>
  )
}

function PairManualCard({
  onDone,
  setErr,
}: {
  onDone: () => void
  setErr: (s: string | null) => void
}) {
  const [synthUrl, setSynthUrl] = useState("")
  const [token, setToken] = useState("")
  const [synthId, setSynthId] = useState("")
  const [busy, setBusy] = useState(false)
  return (
    <Card>
      <CardHeader>
        <CardTitle>Manual pair (debug)</CardTitle>
      </CardHeader>
      <CardContent className="space-y-4">
        <p className="text-sm text-muted-foreground">
          Pre-pair-code flow: paste the synth's public URL, bearer token, and
          synth id.
        </p>
        <div className="space-y-3">
          <div>
            <Label>Synth URL</Label>
            <Input
              value={synthUrl}
              onChange={(e) => setSynthUrl(e.target.value)}
              placeholder="https://mia-snthai-bot.synth.snth.ai"
              className="font-mono text-xs"
            />
          </div>
          <div>
            <Label>Companion token (64 hex chars)</Label>
            <Input
              type="password"
              value={token}
              onChange={(e) => setToken(e.target.value)}
              className="font-mono text-xs"
            />
          </div>
          <div>
            <Label>Synth ID</Label>
            <Input
              value={synthId}
              onChange={(e) => setSynthId(e.target.value)}
              placeholder="mia_snthai_bot"
              className="font-mono text-xs"
            />
          </div>
        </div>
        <Button
          variant="secondary"
          disabled={busy || !synthUrl || !token || !synthId}
          onClick={async () => {
            setBusy(true)
            setErr(null)
            try {
              await savePair(synthUrl, token, synthId)
              setSynthUrl("")
              setToken("")
              setSynthId("")
              onDone()
            } catch (e) {
              setErr(String(e))
            } finally {
              setBusy(false)
            }
          }}
        >
          {busy ? "Saving…" : "Pair"}
        </Button>
      </CardContent>
    </Card>
  )
}

function Row({
  label,
  value,
  mono,
}: {
  label: string
  value: string
  mono?: boolean
}) {
  return (
    <div className="flex items-center justify-between border-b border-border/50 pb-2 last:border-0 last:pb-0">
      <span className="text-muted-foreground text-sm">{label}</span>
      <span className={mono ? "font-mono text-xs" : "text-sm"}>{value}</span>
    </div>
  )
}
