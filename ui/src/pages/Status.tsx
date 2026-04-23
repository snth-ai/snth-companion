import { useEffect, useState } from "react"
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { Badge } from "@/components/ui/badge"
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import { fetchStatus, type StatusResponse } from "@/lib/api"
import { CircleAlert, CircleCheck, CircleDashed, CircleX } from "lucide-react"

const statusBadge: Record<
  StatusResponse["status"],
  { icon: typeof CircleCheck; variant: "default" | "secondary" | "destructive" | "outline"; label: string }
> = {
  connected: { icon: CircleCheck, variant: "default", label: "Connected" },
  connecting: { icon: CircleDashed, variant: "secondary", label: "Connecting…" },
  paused: { icon: CircleAlert, variant: "outline", label: "Paused" },
  disconnected: { icon: CircleX, variant: "destructive", label: "Disconnected" },
}

export function StatusPage() {
  const [data, setData] = useState<StatusResponse | null>(null)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    let cancelled = false
    const load = async () => {
      try {
        const d = await fetchStatus()
        if (!cancelled) setData(d)
      } catch (e) {
        if (!cancelled) setError(String(e))
      }
    }
    load()
    const interval = setInterval(load, 2500)
    return () => {
      cancelled = true
      clearInterval(interval)
    }
  }, [])

  if (error) {
    return (
      <Alert variant="destructive">
        <AlertTitle>Can't reach companion API</AlertTitle>
        <AlertDescription>{error}</AlertDescription>
      </Alert>
    )
  }

  if (!data) {
    return <div className="text-sm text-muted-foreground">Loading…</div>
  }

  const s = statusBadge[data.status] ?? statusBadge.disconnected
  const StatusIcon = s.icon

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-2xl font-semibold tracking-tight">Status</h1>
          <p className="text-sm text-muted-foreground mt-1">
            Live connection to the paired synth.
          </p>
        </div>
        <div className="text-xs text-muted-foreground font-mono">
          v{data.version}
        </div>
      </div>

      {!data.paired && (
        <Alert>
          <AlertTitle>Not paired yet</AlertTitle>
          <AlertDescription>
            Go to the <strong>Pair</strong> tab to connect this companion to a
            synth.
          </AlertDescription>
        </Alert>
      )}

      <Card>
        <CardHeader>
          <CardTitle>Connection</CardTitle>
        </CardHeader>
        <CardContent className="space-y-3 text-sm">
          <Row label="Status">
            <Badge variant={s.variant} className="gap-1">
              <StatusIcon className="h-3 w-3" />
              {s.label}
            </Badge>
          </Row>
          <Row label="Paired synth">
            <span className="font-mono text-xs">
              {data.synth_id || "—"}
            </span>
          </Row>
          <Row label="Synth URL">
            <span className="font-mono text-xs">
              {data.synth_url || "—"}
            </span>
          </Row>
          <Row label="Last seen">
            <span className="font-mono text-xs">
              {data.last_seen ? formatTs(data.last_seen) : "—"}
            </span>
          </Row>
          {data.last_error ? (
            <Row label="Last error">
              <span className="font-mono text-xs text-destructive">
                {data.last_error}
              </span>
            </Row>
          ) : null}
          <Row label="Tools advertised">
            <span className="font-mono text-xs">{data.tools.length}</span>
          </Row>
        </CardContent>
      </Card>
    </div>
  )
}

function Row({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex items-center justify-between border-b border-border/50 pb-2 last:border-0 last:pb-0">
      <span className="text-muted-foreground">{label}</span>
      <span>{children}</span>
    </div>
  )
}

function formatTs(iso: string): string {
  try {
    const d = new Date(iso)
    if (Number.isNaN(d.getTime())) return iso
    return d.toLocaleString(undefined, {
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
      year: "numeric",
      month: "short",
      day: "2-digit",
    })
  } catch {
    return iso
  }
}
