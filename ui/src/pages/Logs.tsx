import { useEffect, useState } from "react"
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { Button } from "@/components/ui/button"
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import { fetchAudit, fetchRemoteLogs, type AuditEntry } from "@/lib/api"

export function LogsPage() {
  const [remote, setRemote] = useState<string>("")
  const [remoteErr, setRemoteErr] = useState<string | null>(null)
  const [audit, setAudit] = useState<AuditEntry[]>([])
  const [auto, setAuto] = useState(false)
  const [lines, setLines] = useState(200)
  const [loading, setLoading] = useState(false)

  const loadRemote = async () => {
    setLoading(true)
    setRemoteErr(null)
    try {
      const r = await fetchRemoteLogs(lines)
      setRemote(r.log ?? "")
    } catch (e) {
      setRemoteErr(String(e))
    } finally {
      setLoading(false)
    }
  }

  const loadAudit = async () => {
    try {
      setAudit(await fetchAudit())
    } catch {
      // non-fatal
    }
  }

  useEffect(() => {
    loadRemote()
    loadAudit()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  useEffect(() => {
    if (!auto) return
    const t = setInterval(() => {
      loadRemote()
      loadAudit()
    }, 2500)
    return () => clearInterval(t)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [auto, lines])

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-2xl font-semibold tracking-tight">Logs</h1>
        <p className="text-sm text-muted-foreground mt-1">
          Remote synth container log (via hub proxy) + local RPC audit.
        </p>
      </div>

      <Card>
        <CardHeader>
          <CardTitle className="flex items-center justify-between gap-2">
            <span>Synth container log (remote)</span>
            <div className="flex items-center gap-2">
              <Button
                size="sm"
                variant={auto ? "default" : "secondary"}
                onClick={() => setAuto((a) => !a)}
              >
                {auto ? "● Auto 2.5s" : "Auto-refresh"}
              </Button>
              <Button
                size="sm"
                variant="secondary"
                disabled={loading}
                onClick={loadRemote}
              >
                {loading ? "…" : "Refresh"}
              </Button>
              <select
                className="text-xs rounded border border-border bg-input/30 px-2 py-1"
                value={lines}
                onChange={(e) => setLines(parseInt(e.target.value, 10))}
              >
                <option value={100}>100</option>
                <option value={200}>200</option>
                <option value={500}>500</option>
                <option value={2000}>2000</option>
              </select>
            </div>
          </CardTitle>
        </CardHeader>
        <CardContent>
          {remoteErr ? (
            <Alert variant="destructive">
              <AlertTitle>Fetch failed</AlertTitle>
              <AlertDescription className="font-mono text-xs">
                {remoteErr}
              </AlertDescription>
            </Alert>
          ) : (
            <pre className="font-mono text-[11px] leading-relaxed whitespace-pre-wrap break-words max-h-[520px] overflow-auto rounded bg-background/60 border border-border/50 p-3">
              {remote || "(empty)"}
            </pre>
          )}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>Companion RPC log (local)</CardTitle>
        </CardHeader>
        <CardContent>
          {audit.length === 0 ? (
            <p className="text-sm text-muted-foreground">
              No recent RPCs.
            </p>
          ) : (
            <table className="w-full text-xs">
              <thead>
                <tr className="text-left text-muted-foreground border-b border-border">
                  <th className="py-2 font-normal">When</th>
                  <th className="py-2 font-normal">Tool</th>
                  <th className="py-2 font-normal">Outcome</th>
                  <th className="py-2 font-normal">ms</th>
                </tr>
              </thead>
              <tbody>
                {audit.slice(0, 30).map((e, i) => (
                  <tr key={i} className="border-b border-border/30">
                    <td className="py-1 font-mono">
                      {new Date(e.started_at).toLocaleTimeString()}
                    </td>
                    <td className="py-1 font-mono">{e.tool}</td>
                    <td
                      className={
                        "py-1 " +
                        (e.outcome === "error"
                          ? "text-destructive"
                          : e.outcome === "denied"
                            ? "text-warning"
                            : "text-success")
                      }
                    >
                      {e.outcome}
                    </td>
                    <td className="py-1 font-mono">{e.duration_ms}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </CardContent>
      </Card>
    </div>
  )
}
