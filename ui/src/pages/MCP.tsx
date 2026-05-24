import { useEffect, useMemo, useState } from "react"
import { Plug, Plus, Trash2, ExternalLink, RefreshCw } from "lucide-react"
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Switch } from "@/components/ui/switch"
import { Textarea } from "@/components/ui/textarea"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet"
import {
  Alert,
  AlertDescription,
  AlertTitle,
} from "@/components/ui/alert"
import {
  fetchMCPServers,
  createMCPServer,
  updateMCPServer,
  deleteMCPServer,
  toggleMCPServer,
  fetchMCPOAuthURL,
  type MCPServerView,
  type MCPServerPayload,
} from "@/lib/api"
import { toast } from "sonner"

// MCPPage — manage MCP (Model Context Protocol) servers for the paired
// synth. MCP servers expose tools the synth's LLM can call: Notion,
// Linear, Gmail, filesystem, custom, etc. Mirrors hub admin /admin/mcp
// but scoped to the user's own synth via /api/my/mcp/* (companion bearer
// auth on the hub side).
//
// Transports:
//   - stdio       — local subprocess (npm packages, your own scripts)
//   - http        — JSON-RPC POST + optional static Bearer
//   - sse         — Server-Sent Events streaming HTTP
//   - http_oauth  — HTTP with OAuth code flow + auto-refresh (Notion etc)

type TransportKind = "stdio" | "http" | "sse" | "http_oauth"

const transportLabels: Record<TransportKind, string> = {
  stdio: "Local subprocess (stdio)",
  http: "HTTP (static token)",
  sse: "SSE (streaming)",
  http_oauth: "HTTP + OAuth",
}

const emptyForm: MCPServerPayload = {
  name: "",
  transport: "stdio",
  command: "",
  args: [],
  env: {},
  url: "",
  static_token: "",
  oauth_auth_url: "",
  oauth_token_url: "",
  oauth_client_id: "",
  oauth_client_secret: "",
  oauth_scopes: "",
  enabled: true,
}

export function MCPPage() {
  const [servers, setServers] = useState<MCPServerView[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [editing, setEditing] = useState<MCPServerView | null>(null)
  const [creating, setCreating] = useState(false)
  const [busy, setBusy] = useState(false)

  const load = async () => {
    setLoading(true)
    setError(null)
    try {
      const list = await fetchMCPServers()
      setServers(list)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }

  useEffect(() => {
    load()
  }, [])

  const { instanceServers, globalServers } = useMemo(() => {
    const inst: MCPServerView[] = []
    const glob: MCPServerView[] = []
    for (const s of servers) {
      ;(s.scope === "instance" ? inst : glob).push(s)
    }
    return { instanceServers: inst, globalServers: glob }
  }, [servers])

  const handleToggle = async (s: MCPServerView) => {
    setBusy(true)
    try {
      await toggleMCPServer(s.id)
      toast.success(`${s.name} ${s.enabled ? "disabled" : "enabled"}`)
      await load()
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  const handleDelete = async (s: MCPServerView) => {
    if (!confirm(`Delete MCP server "${s.name}"? Tools will disappear from the synth within 2 minutes.`))
      return
    setBusy(true)
    try {
      await deleteMCPServer(s.id)
      toast.success(`${s.name} deleted`)
      await load()
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  const handleConnectOAuth = async (s: MCPServerView) => {
    try {
      const { url } = await fetchMCPOAuthURL(s.id)
      // Open in default browser via window.open — companion intercepts
      // window.open in Wave-2 and forwards to OS browser.
      window.open(url, "_blank", "noopener,noreferrer")
      toast.info(
        "OAuth flow opened in your browser. After approving, return here and refresh — token expiry will update.",
      )
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    }
  }

  return (
    <div className="max-w-5xl mx-auto space-y-6 py-6 px-4">
      <div className="flex items-start justify-between">
        <div>
          <h1 className="text-2xl font-semibold flex items-center gap-2">
            <Plug className="w-6 h-6" />
            MCP Servers
          </h1>
          <p className="text-sm text-muted-foreground mt-1 max-w-xl">
            Add external tool servers your synth can use. Each server exposes
            tools via the Model Context Protocol; they appear in your synth's
            tool list as <code>{"<server>__<tool>"}</code> within ~2 minutes.
          </p>
        </div>
        <div className="flex gap-2">
          <Button variant="outline" size="sm" onClick={load} disabled={loading}>
            <RefreshCw className={`w-4 h-4 mr-1 ${loading ? "animate-spin" : ""}`} />
            Refresh
          </Button>
          <Button size="sm" onClick={() => setCreating(true)}>
            <Plus className="w-4 h-4 mr-1" />
            Add server
          </Button>
        </div>
      </div>

      {error && (
        <Alert variant="destructive">
          <AlertTitle>Failed to load MCP servers</AlertTitle>
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      )}

      <Card>
        <CardHeader>
          <CardTitle className="text-base">Your servers</CardTitle>
        </CardHeader>
        <CardContent>
          {instanceServers.length === 0 && !loading && (
            <p className="text-sm text-muted-foreground text-center py-8">
              No MCP servers yet. Click <strong>Add server</strong> above to
              connect Notion, Linear, Gmail, a local filesystem, or any custom
              MCP server.
            </p>
          )}
          <div className="space-y-2">
            {instanceServers.map((s) => (
              <ServerRow
                key={s.id}
                server={s}
                onEdit={() => setEditing(s)}
                onToggle={() => handleToggle(s)}
                onDelete={() => handleDelete(s)}
                onConnectOAuth={() => handleConnectOAuth(s)}
                busy={busy}
              />
            ))}
          </div>
        </CardContent>
      </Card>

      {globalServers.length > 0 && (
        <Card>
          <CardHeader>
            <CardTitle className="text-base text-muted-foreground">
              Inherited (fleet-wide, read-only)
            </CardTitle>
          </CardHeader>
          <CardContent>
            <div className="space-y-2">
              {globalServers.map((s) => (
                <ServerRow
                  key={s.id}
                  server={s}
                  onEdit={() => {}}
                  onToggle={() => {}}
                  onDelete={() => {}}
                  onConnectOAuth={() => {}}
                  busy={busy}
                  readOnly
                />
              ))}
            </div>
          </CardContent>
        </Card>
      )}

      <ServerSheet
        open={creating || editing !== null}
        existing={editing}
        onClose={() => {
          setCreating(false)
          setEditing(null)
        }}
        onSaved={async () => {
          setCreating(false)
          setEditing(null)
          await load()
        }}
      />
    </div>
  )
}

type ServerRowProps = {
  server: MCPServerView
  onEdit: () => void
  onToggle: () => void
  onDelete: () => void
  onConnectOAuth: () => void
  busy: boolean
  readOnly?: boolean
}

function ServerRow({
  server,
  onEdit,
  onToggle,
  onDelete,
  onConnectOAuth,
  busy,
  readOnly = false,
}: ServerRowProps) {
  const connection =
    server.transport === "stdio"
      ? [server.command, ...(server.args ?? [])].filter(Boolean).join(" ")
      : (server.url ?? "")
  return (
    <div className="flex items-start gap-3 p-3 rounded-md border border-border">
      <div className="flex-1 min-w-0">
        <div className="flex items-center gap-2 flex-wrap">
          <div className="font-medium">{server.name}</div>
          <Badge variant="outline" className="text-xs">
            {server.transport}
          </Badge>
          {readOnly && (
            <Badge variant="secondary" className="text-xs">
              global
            </Badge>
          )}
          {!server.enabled && (
            <Badge variant="destructive" className="text-xs">
              disabled
            </Badge>
          )}
          {server.transport === "http_oauth" && server.has_oauth_token && (
            <Badge className="text-xs">
              token ✓ {server.oauth_expires_at && `(exp ${formatExpiry(server.oauth_expires_at)})`}
            </Badge>
          )}
          {server.transport === "http_oauth" && !server.has_oauth_token && (
            <Badge variant="destructive" className="text-xs">
              not connected
            </Badge>
          )}
        </div>
        {connection && (
          <div className="text-xs text-muted-foreground font-mono mt-1 truncate">
            {connection}
          </div>
        )}
        {server.last_status && (
          <div className="text-xs text-muted-foreground mt-1">
            last probe: {server.last_status}{" "}
            {server.last_status_at && `· ${formatExpiry(server.last_status_at)}`}
          </div>
        )}
      </div>
      {!readOnly && (
        <div className="flex items-center gap-2 shrink-0">
          {server.transport === "http_oauth" && (
            <Button
              variant="outline"
              size="sm"
              onClick={onConnectOAuth}
              disabled={busy}
              title={server.has_oauth_token ? "Re-authenticate" : "Connect via OAuth"}
            >
              <ExternalLink className="w-3 h-3 mr-1" />
              {server.has_oauth_token ? "Re-auth" : "Connect"}
            </Button>
          )}
          <Switch
            checked={server.enabled}
            onCheckedChange={onToggle}
            disabled={busy}
          />
          <Button variant="outline" size="sm" onClick={onEdit} disabled={busy}>
            Edit
          </Button>
          <Button
            variant="ghost"
            size="icon"
            onClick={onDelete}
            disabled={busy}
            title="Delete"
          >
            <Trash2 className="w-4 h-4 text-destructive" />
          </Button>
        </div>
      )}
    </div>
  )
}

function formatExpiry(iso: string): string {
  try {
    const d = new Date(iso)
    return d.toLocaleString(undefined, {
      year: "numeric",
      month: "short",
      day: "numeric",
      hour: "2-digit",
      minute: "2-digit",
    })
  } catch {
    return iso
  }
}

type ServerSheetProps = {
  open: boolean
  existing: MCPServerView | null
  onClose: () => void
  onSaved: () => void
}

function ServerSheet({ open, existing, onClose, onSaved }: ServerSheetProps) {
  const [form, setForm] = useState<MCPServerPayload>(emptyForm)
  const [argsLine, setArgsLine] = useState("")
  const [envLines, setEnvLines] = useState("")
  const [saving, setSaving] = useState(false)
  const [err, setErr] = useState<string | null>(null)

  useEffect(() => {
    if (!open) return
    if (existing) {
      setForm({
        name: existing.name,
        transport: existing.transport,
        command: existing.command ?? "",
        args: existing.args ?? [],
        env: existing.env ?? {},
        url: existing.url ?? "",
        static_token: "",
        oauth_auth_url: existing.oauth_auth_url ?? "",
        oauth_token_url: existing.oauth_token_url ?? "",
        oauth_client_id: existing.oauth_client_id ?? "",
        oauth_client_secret: "",
        oauth_scopes: existing.oauth_scopes ?? "",
        enabled: existing.enabled,
      })
      setArgsLine((existing.args ?? []).join(" "))
      setEnvLines(
        Object.entries(existing.env ?? {})
          .map(([k, v]) => `${k}=${v}`)
          .join("\n"),
      )
    } else {
      setForm({ ...emptyForm })
      setArgsLine("")
      setEnvLines("")
    }
    setErr(null)
  }, [open, existing])

  const transport = form.transport ?? "stdio"

  const handleSave = async () => {
    setSaving(true)
    setErr(null)
    try {
      // Re-parse args + env so the textarea drives the source of truth.
      const args = argsLine.trim() ? argsLine.split(/\s+/).filter(Boolean) : []
      const env: Record<string, string> = {}
      for (const raw of envLines.split("\n")) {
        const line = raw.trim()
        if (!line) continue
        const eq = line.indexOf("=")
        if (eq < 0) continue
        env[line.slice(0, eq).trim()] = line.slice(eq + 1).trim()
      }
      const payload: MCPServerPayload = {
        ...form,
        args,
        env,
      }
      // Don't ship empty token strings as overrides — server treats nil
      // as "leave existing", "" as "clear".
      if (!payload.static_token) delete payload.static_token
      if (!payload.oauth_client_secret) delete payload.oauth_client_secret
      if (existing) {
        await updateMCPServer(existing.id, payload)
        toast.success(`${payload.name} updated`)
      } else {
        await createMCPServer(payload)
        toast.success(`${payload.name} added — synth will pick it up within 2 min`)
      }
      onSaved()
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e))
    } finally {
      setSaving(false)
    }
  }

  return (
    <Sheet open={open} onOpenChange={(v) => !v && onClose()}>
      <SheetContent className="sm:max-w-lg overflow-y-auto">
        <SheetHeader>
          <SheetTitle>{existing ? `Edit ${existing.name}` : "Add MCP server"}</SheetTitle>
          <SheetDescription>
            {existing
              ? "Update connection details. Token fields stay unchanged when left blank."
              : "Pick a transport, fill in the connection details, save. The synth re-syncs within 2 minutes."}
          </SheetDescription>
        </SheetHeader>

        <div className="space-y-4 py-4 px-4">
          <div className="space-y-1">
            <Label>Name</Label>
            <Input
              placeholder="notion"
              value={form.name ?? ""}
              onChange={(e) => setForm({ ...form, name: e.target.value })}
              disabled={!!existing}
            />
            <p className="text-xs text-muted-foreground">
              Used as the tool-name prefix. Tools appear as{" "}
              <code>{form.name || "<name>"}__<em>tool</em></code>.
            </p>
          </div>

          <div className="space-y-1">
            <Label>Transport</Label>
            <Select
              value={transport}
              onValueChange={(v) =>
                setForm({ ...form, transport: v as TransportKind })
              }
            >
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {(Object.keys(transportLabels) as TransportKind[]).map((k) => (
                  <SelectItem key={k} value={k}>
                    {transportLabels[k]}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>

          {transport === "stdio" && (
            <>
              <div className="space-y-1">
                <Label>Command</Label>
                <Input
                  placeholder="npx"
                  value={form.command ?? ""}
                  onChange={(e) => setForm({ ...form, command: e.target.value })}
                />
              </div>
              <div className="space-y-1">
                <Label>Args (space-separated)</Label>
                <Input
                  placeholder="-y @modelcontextprotocol/server-filesystem /tmp"
                  value={argsLine}
                  onChange={(e) => setArgsLine(e.target.value)}
                />
              </div>
              <div className="space-y-1">
                <Label>Env (KEY=value per line)</Label>
                <Textarea
                  rows={3}
                  placeholder="API_KEY=..."
                  value={envLines}
                  onChange={(e) => setEnvLines(e.target.value)}
                  className="font-mono text-xs"
                />
              </div>
            </>
          )}

          {(transport === "http" || transport === "sse" || transport === "http_oauth") && (
            <div className="space-y-1">
              <Label>URL</Label>
              <Input
                placeholder="https://mcp.example.com/v1"
                value={form.url ?? ""}
                onChange={(e) => setForm({ ...form, url: e.target.value })}
              />
            </div>
          )}

          {(transport === "http" || transport === "sse") && (
            <div className="space-y-1">
              <Label>Static Bearer token (optional)</Label>
              <Input
                type="password"
                placeholder={existing?.has_static_token ? "(unchanged)" : "sk_..."}
                value={form.static_token ?? ""}
                onChange={(e) => setForm({ ...form, static_token: e.target.value })}
              />
            </div>
          )}

          {transport === "http_oauth" && (
            <>
              <div className="space-y-1">
                <Label>OAuth authorize URL</Label>
                <Input
                  placeholder="https://api.notion.com/v1/oauth/authorize"
                  value={form.oauth_auth_url ?? ""}
                  onChange={(e) =>
                    setForm({ ...form, oauth_auth_url: e.target.value })
                  }
                />
              </div>
              <div className="space-y-1">
                <Label>OAuth token URL</Label>
                <Input
                  placeholder="https://api.notion.com/v1/oauth/token"
                  value={form.oauth_token_url ?? ""}
                  onChange={(e) =>
                    setForm({ ...form, oauth_token_url: e.target.value })
                  }
                />
              </div>
              <div className="space-y-1">
                <Label>OAuth client_id</Label>
                <Input
                  value={form.oauth_client_id ?? ""}
                  onChange={(e) =>
                    setForm({ ...form, oauth_client_id: e.target.value })
                  }
                />
              </div>
              <div className="space-y-1">
                <Label>OAuth client_secret (optional — PKCE-only providers skip)</Label>
                <Input
                  type="password"
                  placeholder={existing ? "(unchanged)" : ""}
                  value={form.oauth_client_secret ?? ""}
                  onChange={(e) =>
                    setForm({ ...form, oauth_client_secret: e.target.value })
                  }
                />
              </div>
              <div className="space-y-1">
                <Label>Scopes (space-separated)</Label>
                <Input
                  placeholder="read_content update_content"
                  value={form.oauth_scopes ?? ""}
                  onChange={(e) =>
                    setForm({ ...form, oauth_scopes: e.target.value })
                  }
                />
              </div>
              <Alert>
                <AlertTitle>OAuth flow</AlertTitle>
                <AlertDescription>
                  After saving, click <strong>Connect</strong> on the row. Your
                  browser will open the provider's login page; once you approve,
                  the token comes back to the hub, gets encrypted at rest, and
                  refresh happens automatically before expiry.
                </AlertDescription>
              </Alert>
            </>
          )}

          <div className="flex items-center gap-2 pt-2">
            <Switch
              checked={form.enabled ?? true}
              onCheckedChange={(v) => setForm({ ...form, enabled: v })}
            />
            <Label className="text-sm">Enabled</Label>
          </div>

          {err && (
            <Alert variant="destructive">
              <AlertDescription>{err}</AlertDescription>
            </Alert>
          )}
        </div>

        <div className="flex gap-2 justify-end px-4 pb-4 mt-auto">
          <Button variant="outline" onClick={onClose} disabled={saving}>
            Cancel
          </Button>
          <Button onClick={handleSave} disabled={saving}>
            {saving ? "Saving…" : existing ? "Save changes" : "Add server"}
          </Button>
        </div>
      </SheetContent>
    </Sheet>
  )
}
