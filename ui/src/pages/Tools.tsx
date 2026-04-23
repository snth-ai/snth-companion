import { useEffect, useState } from "react"
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { Badge } from "@/components/ui/badge"
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import { fetchTools, type ToolEntry } from "@/lib/api"

const dangerVariant: Record<
  string,
  "default" | "secondary" | "destructive" | "outline"
> = {
  safe: "default",
  prompt: "secondary",
  "always-prompt": "destructive",
}

export function ToolsPage() {
  const [tools, setTools] = useState<ToolEntry[] | null>(null)
  const [err, setErr] = useState<string | null>(null)

  useEffect(() => {
    let cancelled = false
    const load = async () => {
      try {
        const t = await fetchTools()
        if (!cancelled) setTools(t)
      } catch (e) {
        if (!cancelled) setErr(String(e))
      }
    }
    load()
    const interval = setInterval(load, 5000)
    return () => {
      cancelled = true
      clearInterval(interval)
    }
  }, [])

  if (err && !tools) {
    return (
      <Alert variant="destructive">
        <AlertTitle>Error</AlertTitle>
        <AlertDescription>{err}</AlertDescription>
      </Alert>
    )
  }
  if (!tools) {
    return <div className="text-sm text-muted-foreground">Loading…</div>
  }

  const totalCalls = tools.reduce((n, t) => n + t.stat.calls, 0)
  const totalErr = tools.reduce((n, t) => n + t.stat.errors, 0)

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-2xl font-semibold tracking-tight">Tools</h1>
        <p className="text-sm text-muted-foreground mt-1">
          Tool catalog advertised to the paired synth in the{" "}
          <code className="font-mono text-xs">hello</code> frame. To enable
          a new one, restart the companion.
        </p>
      </div>

      <div className="grid grid-cols-3 gap-3">
        <SummaryTile label="Registered" value={tools.length} />
        <SummaryTile label="Calls (session)" value={totalCalls} />
        <SummaryTile
          label="Errors (session)"
          value={totalErr}
          danger={totalErr > 0}
        />
      </div>

      <div className="grid grid-cols-2 gap-4">
        {tools.map((t) => (
          <Card key={t.name}>
            <CardHeader className="pb-3">
              <CardTitle className="flex items-center justify-between gap-2 text-sm font-mono">
                <span className="truncate">{t.name}</span>
                <Badge
                  variant={dangerVariant[t.danger_level] ?? "outline"}
                  className="shrink-0"
                >
                  {t.danger_level}
                </Badge>
              </CardTitle>
            </CardHeader>
            <CardContent className="text-xs text-muted-foreground space-y-3">
              <p className="leading-relaxed">{t.description}</p>
              <div className="border-t border-border/50 pt-2">
                {t.stat.last ? (
                  <>
                    Last:{" "}
                    <span className="font-mono">
                      {new Date(t.stat.last.started_at).toLocaleTimeString()}
                    </span>{" "}
                    ·{" "}
                    <span
                      className={
                        t.stat.last.outcome === "error"
                          ? "text-destructive"
                          : t.stat.last.outcome === "denied"
                            ? "text-warning"
                            : "text-success"
                      }
                    >
                      {t.stat.last.outcome}
                    </span>{" "}
                    · <span className="font-mono">{t.stat.last.duration_ms}ms</span>
                    <div>
                      Calls: {t.stat.calls} · Errors: {t.stat.errors}
                    </div>
                  </>
                ) : (
                  <span className="text-muted-foreground/70">
                    Not yet used this session.
                  </span>
                )}
              </div>
            </CardContent>
          </Card>
        ))}
      </div>
    </div>
  )
}

function SummaryTile({
  label,
  value,
  danger,
}: {
  label: string
  value: number
  danger?: boolean
}) {
  return (
    <Card>
      <CardContent className="pt-6">
        <div
          className={
            "text-3xl font-semibold tracking-tight " +
            (danger ? "text-destructive" : "")
          }
        >
          {value}
        </div>
        <div className="text-xs text-muted-foreground mt-1">{label}</div>
      </CardContent>
    </Card>
  )
}
