import { useEffect, useState } from "react"
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import { addSandbox, fetchSandbox, removeSandbox } from "@/lib/api"
import { Trash2 } from "lucide-react"

export function SandboxPage() {
  const [roots, setRoots] = useState<string[]>([])
  const [path, setPath] = useState("")
  const [err, setErr] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  const reload = async () => {
    setErr(null)
    try {
      setRoots(await fetchSandbox())
    } catch (e) {
      setErr(String(e))
    }
  }

  useEffect(() => {
    reload()
  }, [])

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-2xl font-semibold tracking-tight">Sandbox</h1>
        <p className="text-sm text-muted-foreground mt-1">
          Paths the companion lets tools touch without an approval
          dialog for safe commands. Everything outside always prompts.
        </p>
      </div>

      {err && (
        <Alert variant="destructive">
          <AlertTitle>Error</AlertTitle>
          <AlertDescription>{err}</AlertDescription>
        </Alert>
      )}

      <Card>
        <CardHeader>
          <CardTitle>Sandbox roots</CardTitle>
        </CardHeader>
        <CardContent className="space-y-3">
          {roots.length === 0 ? (
            <p className="text-sm text-muted-foreground">
              No roots. Every file operation will require explicit
              approval.
            </p>
          ) : (
            <ul className="divide-y divide-border">
              {roots.map((r) => (
                <li
                  key={r}
                  className="flex items-center justify-between py-2"
                >
                  <code className="font-mono text-xs truncate">{r}</code>
                  <Button
                    size="sm"
                    variant="ghost"
                    disabled={busy}
                    onClick={async () => {
                      setBusy(true)
                      setErr(null)
                      try {
                        await removeSandbox(r)
                        await reload()
                      } catch (e) {
                        setErr(String(e))
                      } finally {
                        setBusy(false)
                      }
                    }}
                  >
                    <Trash2 className="h-4 w-4" />
                  </Button>
                </li>
              ))}
            </ul>
          )}

          <div className="flex gap-2 pt-2">
            <Input
              value={path}
              onChange={(e) => setPath(e.target.value)}
              placeholder="/Users/sabry/Projects/…"
              className="font-mono text-xs"
            />
            <Button
              disabled={busy || !path.trim()}
              onClick={async () => {
                setBusy(true)
                setErr(null)
                try {
                  await addSandbox(path.trim())
                  setPath("")
                  await reload()
                } catch (e) {
                  setErr(String(e))
                } finally {
                  setBusy(false)
                }
              }}
            >
              {busy ? "Adding…" : "Add"}
            </Button>
          </div>
        </CardContent>
      </Card>
    </div>
  )
}
