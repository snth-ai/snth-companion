import { useEffect, useMemo, useState } from "react"
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
  CardDescription,
} from "@/components/ui/card"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Switch } from "@/components/ui/switch"
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import { Badge } from "@/components/ui/badge"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import { ShieldAlert, ShieldCheck, ShieldOff, Trash2 } from "lucide-react"
import {
  fetchTrust,
  fetchTrustAudit,
  setTrustMaster,
  setTrustTool,
  trustPathAdd,
  trustPathRemove,
  trustRevokeAll,
  type ToolMode,
  type TrustAuditEntry,
  type TrustResponse,
  type TrustToolDef,
} from "@/lib/api"

const expiryPresets = [
  { label: "No expiry", value: "" },
  { label: "1 hour", hours: 1 },
  { label: "4 hours", hours: 4 },
  { label: "24 hours", hours: 24 },
  { label: "7 days", hours: 24 * 7 },
] as const

function presetToISO(p: (typeof expiryPresets)[number]): string {
  if (!("hours" in p)) return ""
  return new Date(Date.now() + p.hours * 3600 * 1000).toISOString()
}

export function PrivacyPage() {
  const [data, setData] = useState<TrustResponse | null>(null)
  const [audit, setAudit] = useState<TrustAuditEntry[]>([])
  const [err, setErr] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [newPath, setNewPath] = useState("")
  const [presetIdx, setPresetIdx] = useState(0)

  const reload = async () => {
    setErr(null)
    try {
      const [t, a] = await Promise.all([fetchTrust(), fetchTrustAudit()])
      setData(t)
      setAudit(a)
    } catch (e) {
      setErr(String(e))
    }
  }

  useEffect(() => {
    reload()
    const t = setInterval(() => {
      fetchTrustAudit()
        .then(setAudit)
        .catch(() => {})
    }, 5000)
    return () => clearInterval(t)
  }, [])

  const masterOn = data?.state.master ?? false
  const masterExpires = data?.state.master_expires ?? null
  const tools = data?.tools ?? []
  const writeRoots = data?.state.allowed_write_roots ?? []

  const masterExpiresHuman = useMemo(() => {
    if (!masterExpires) return null
    const t = new Date(masterExpires)
    if (isNaN(t.getTime())) return null
    const now = Date.now()
    const diff = t.getTime() - now
    if (diff <= 0) return "expired"
    const hours = Math.floor(diff / 3600 / 1000)
    if (hours >= 48) return `expires in ${Math.floor(hours / 24)}d`
    if (hours >= 1) return `expires in ${hours}h`
    return `expires in ${Math.floor(diff / 60 / 1000)}m`
  }, [masterExpires])

  const handleMaster = async (on: boolean) => {
    setBusy(true)
    try {
      const expires = on ? presetToISO(expiryPresets[presetIdx]) : ""
      await setTrustMaster(on, expires)
      await reload()
    } catch (e) {
      setErr(String(e))
    } finally {
      setBusy(false)
    }
  }

  const handleTool = async (tool: string, mode: ToolMode) => {
    setBusy(true)
    try {
      await setTrustTool(tool, mode)
      await reload()
    } catch (e) {
      setErr(String(e))
    } finally {
      setBusy(false)
    }
  }

  const handleRevoke = async () => {
    if (!confirm("Reset ALL trust state to prompt-everything? Audit log keeps the kill-switch entry. This is the panic button — only use it if something feels wrong."))
      return
    setBusy(true)
    try {
      await trustRevokeAll()
      await reload()
    } catch (e) {
      setErr(String(e))
    } finally {
      setBusy(false)
    }
  }

  const handleAddPath = async () => {
    const p = newPath.trim()
    if (!p) return
    setBusy(true)
    try {
      await trustPathAdd(p)
      setNewPath("")
      await reload()
    } catch (e) {
      setErr(String(e))
    } finally {
      setBusy(false)
    }
  }

  const handleRemovePath = async (p: string) => {
    setBusy(true)
    try {
      await trustPathRemove(p)
      await reload()
    } catch (e) {
      setErr(String(e))
    } finally {
      setBusy(false)
    }
  }

  return (
      <div className="space-y-6">
        <div>
          <h1 className="text-2xl font-semibold tracking-tight">Privacy</h1>
          <p className="text-sm text-muted-foreground mt-1">
            Per-tool approval policy. Default is{" "}
            <Badge variant="outline" className="font-mono">
              prompt
            </Badge>{" "}
            everywhere — every action pops a dialog. Flip a tool to{" "}
            <Badge variant="outline" className="font-mono text-emerald-400">
              trusted
            </Badge>{" "}
            to skip the dialog, or{" "}
            <Badge variant="outline" className="font-mono text-red-400">
              denied
            </Badge>{" "}
            to never let it run. The synth sees your decision in its audit
            stream.
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
            <CardTitle className="flex items-center gap-2">
              <ShieldCheck className="h-5 w-5" />
              Master toggle
            </CardTitle>
            <CardDescription>
              When ON, every tool not in the always-prompt set and not
              explicitly denied auto-approves without a dialog. Critical
              tools (subagent, iMessage send) still prompt — by design,
              they're high-cost actions and the dialog is the receipt.
            </CardDescription>
          </CardHeader>
          <CardContent className="space-y-4">
            <div className="flex items-center justify-between">
              <div>
                <div className="font-medium">
                  Trust everything (with safe exclusions)
                </div>
                <div className="text-xs text-muted-foreground">
                  {masterOn ? (
                    <>
                      Currently <span className="text-emerald-400">ON</span>
                      {masterExpiresHuman ? ` · ${masterExpiresHuman}` : ""}
                    </>
                  ) : (
                    "Currently OFF — every tool prompts."
                  )}
                </div>
              </div>
              <Switch
                checked={masterOn}
                disabled={busy}
                onCheckedChange={handleMaster}
              />
            </div>

            {!masterOn && (
              <div className="flex items-center gap-2">
                <span className="text-xs text-muted-foreground">
                  Auto-revoke after:
                </span>
                <Select
                  value={String(presetIdx)}
                  onValueChange={(v) => setPresetIdx(Number(v))}
                >
                  <SelectTrigger className="w-40">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {expiryPresets.map((p, i) => (
                      <SelectItem key={i} value={String(i)}>
                        {p.label}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
            )}
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle>Per-tool overrides</CardTitle>
            <CardDescription>
              Tools listed top-to-bottom by danger. The override here wins
              over the master toggle. Always-prompt tools have a 🛡️ icon —
              they keep prompting unless you explicitly flip them.
            </CardDescription>
          </CardHeader>
          <CardContent>
            <ul className="divide-y divide-border">
              {tools.map((t: TrustToolDef) => (
                <li key={t.id} className="py-3 first:pt-0 last:pb-0">
                  <div className="flex items-start justify-between gap-4">
                    <div className="min-w-0">
                      <div className="font-medium flex items-center gap-2">
                        {t.always_prompt && (
                          <ShieldAlert
                            className="h-4 w-4 text-amber-400"
                            aria-label="Always-prompt: master toggle does NOT auto-approve this tool"
                          />
                        )}
                        {t.label}
                        <Badge
                          variant="outline"
                          className="font-mono text-xs"
                        >
                          {t.id}
                        </Badge>
                      </div>
                      <div className="text-xs text-muted-foreground mt-0.5">
                        {t.description}
                      </div>
                    </div>
                    <div className="shrink-0">
                      <Select
                        value={t.current_mode}
                        disabled={busy}
                        onValueChange={(v) =>
                          handleTool(t.id, v as ToolMode)
                        }
                      >
                        <SelectTrigger className="w-32">
                          <SelectValue />
                        </SelectTrigger>
                        <SelectContent>
                          <SelectItem value="prompt">prompt</SelectItem>
                          <SelectItem value="trusted">trusted</SelectItem>
                          <SelectItem value="denied">denied</SelectItem>
                        </SelectContent>
                      </Select>
                    </div>
                  </div>
                </li>
              ))}
            </ul>
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle>Write-root scopes (fs_write)</CardTitle>
            <CardDescription>
              When set, file-write tools are auto-approved ONLY when the
              destination is under one of these roots — even if the tool
              is otherwise trusted. Empty list = trust applies fully (no
              path restriction).
            </CardDescription>
          </CardHeader>
          <CardContent className="space-y-3">
            {writeRoots.length === 0 ? (
              <p className="text-sm text-muted-foreground">
                No path scopes set. Writes are trust-controlled by tool only.
              </p>
            ) : (
              <ul className="divide-y divide-border">
                {writeRoots.map((r) => (
                  <li
                    key={r}
                    className="flex items-center justify-between py-2"
                  >
                    <code className="font-mono text-xs truncate">{r}</code>
                    <Button
                      size="sm"
                      variant="ghost"
                      disabled={busy}
                      onClick={() => handleRemovePath(r)}
                    >
                      <Trash2 className="h-4 w-4" />
                    </Button>
                  </li>
                ))}
              </ul>
            )}
            <div className="flex gap-2">
              <Input
                value={newPath}
                onChange={(e) => setNewPath(e.target.value)}
                placeholder="/Users/sasha/projects"
                disabled={busy}
              />
              <Button
                disabled={busy || !newPath.trim()}
                onClick={handleAddPath}
              >
                Add root
              </Button>
            </div>
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle className="flex items-center gap-2">
              <ShieldOff className="h-5 w-5 text-red-400" />
              Panic switch
            </CardTitle>
            <CardDescription>
              Revoke ALL trust — every tool back to prompt-mode, master
              off, all path scopes cleared. Use this if something feels
              wrong or you handed your laptop to someone else.
            </CardDescription>
          </CardHeader>
          <CardContent>
            <Button
              variant="destructive"
              disabled={busy}
              onClick={handleRevoke}
            >
              Revoke all trust
            </Button>
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle>Recent decisions</CardTitle>
            <CardDescription>
              Last {audit.length} approval evaluations (auto-refresh every
              5s). Source field shows what made the decision —{" "}
              <code className="font-mono text-xs">trusted</code> means the
              dialog was skipped because of your settings;{" "}
              <code className="font-mono text-xs">prompt</code> means a
              real dialog appeared.
            </CardDescription>
          </CardHeader>
          <CardContent>
            {audit.length === 0 ? (
              <p className="text-sm text-muted-foreground">
                No approval decisions yet. The synth hasn't tried any
                tool that needs approval.
              </p>
            ) : (
              <ul className="divide-y divide-border">
                {audit.slice(0, 50).map((e, i) => (
                  <li key={i} className="py-2 text-sm">
                    <div className="flex items-center gap-2">
                      <span
                        className={
                          e.decision === "approved"
                            ? "text-emerald-400 font-mono text-xs"
                            : "text-red-400 font-mono text-xs"
                        }
                      >
                        {e.decision ?? e.outcome}
                      </span>
                      <Badge
                        variant="outline"
                        className="font-mono text-xs"
                      >
                        {e.tool || "(no tool)"}
                      </Badge>
                      <span className="text-xs text-muted-foreground">
                        {e.source || ""}
                      </span>
                      <span className="text-xs text-muted-foreground ml-auto">
                        {new Date(e.started_at).toLocaleTimeString()}
                      </span>
                    </div>
                    <div className="text-xs text-muted-foreground mt-0.5 truncate">
                      {e.args_summary}
                    </div>
                  </li>
                ))}
              </ul>
            )}
          </CardContent>
        </Card>
      </div>
  )
}
