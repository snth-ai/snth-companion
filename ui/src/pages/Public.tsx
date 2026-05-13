import { useEffect, useState } from "react"
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Textarea } from "@/components/ui/textarea"
import { Switch } from "@/components/ui/switch"
import { Label } from "@/components/ui/label"
import { Badge } from "@/components/ui/badge"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs"
import { Separator } from "@/components/ui/separator"
import {
  fetchGroupConfigs,
  upsertGroupConfig,
  deleteGroupConfig,
  fetchPendingOutbound,
  approveOutbound,
  rejectOutbound,
  queueOutboundManual,
  importBriefing,
  parseAttachment,
  synthFileURL,
  type GroupConfig,
  type PendingOutbound,
  type AttachmentPayload,
} from "@/lib/api"
import { toast } from "sonner"
import {
  Users, MessageSquare, Send, Clock, AlertCircle, Trash2,
  Image, Video, FileText, Mic, Music, Smile, Film, BarChart3, LayoutGrid,
} from "lucide-react"

// Public — Mia Public V1 (2026-05-11) management surface. Lists the
// groups Mia is configured in, surfaces pending outbound approvals,
// lets the operator drive per-group settings and post manually. See
// atlas `20-mia-public-surface.md` for the architecture.
export function PublicPage() {
  const [groups, setGroups] = useState<GroupConfig[] | null>(null)
  const [pending, setPending] = useState<PendingOutbound[] | null>(null)
  const [err, setErr] = useState<string | null>(null)
  const [editing, setEditing] = useState<GroupConfig | null>(null)
  const [addingNew, setAddingNew] = useState(false)
  const [briefingOpen, setBriefingOpen] = useState(false)

  const load = async () => {
    try {
      const [gs, ps] = await Promise.all([
        fetchGroupConfigs(),
        fetchPendingOutbound(),
      ])
      setGroups(gs)
      setPending(ps)
      setErr(null)
    } catch (e) {
      setErr(String((e as Error).message ?? e))
    }
  }

  useEffect(() => {
    void load()
    const t = setInterval(load, 8000)
    return () => clearInterval(t)
  }, [])

  const handleApprove = async (id: number, finalText?: string) => {
    try {
      await approveOutbound(id, "operator", finalText)
      toast.success("Approved — will send within ~8s")
      await load()
    } catch (e) {
      toast.error(String((e as Error).message ?? e))
    }
  }

  const handleReject = async (id: number) => {
    const reason = prompt("Rejection reason (optional):") ?? ""
    try {
      await rejectOutbound(id, "operator", reason)
      toast.success("Rejected")
      await load()
    } catch (e) {
      toast.error(String((e as Error).message ?? e))
    }
  }

  const handleDelete = async (gc: GroupConfig) => {
    if (!confirm(`Delete config for ${gc.name || gc.group_chat_id}?`)) return
    try {
      await deleteGroupConfig(gc.group_chat_id)
      toast.success("Deleted")
      await load()
    } catch (e) {
      toast.error(String((e as Error).message ?? e))
    }
  }

  const modeBadgeVariant = (mode: string) => {
    if (mode === "trust") return "default"
    if (mode === "soft") return "secondary"
    return "outline"
  }

  return (
    <div className="container mx-auto py-6 space-y-4">
      <div className="flex items-center justify-between">
        <h1 className="text-2xl font-bold flex items-center gap-2">
          <Users className="w-6 h-6" />
          Public surface
        </h1>
        <div className="flex gap-2">
          <Button variant="outline" onClick={() => setBriefingOpen(true)}>
            Import briefing
          </Button>
          <Button onClick={() => setAddingNew(true)}>+ Add group</Button>
        </div>
      </div>

      {err && (
        <Alert variant="destructive">
          <AlertCircle className="w-4 h-4" />
          <AlertTitle>Couldn't load</AlertTitle>
          <AlertDescription>{err}</AlertDescription>
        </Alert>
      )}

      <Tabs defaultValue="groups">
        <TabsList>
          <TabsTrigger value="groups">
            Groups {groups && `(${groups.length})`}
          </TabsTrigger>
          <TabsTrigger value="pending">
            Pending {pending && pending.length > 0 && (
              <Badge variant="destructive" className="ml-2">
                {pending.length}
              </Badge>
            )}
          </TabsTrigger>
          <TabsTrigger value="manual">Manual post</TabsTrigger>
        </TabsList>

        <TabsContent value="groups">
          {groups === null && <p className="text-sm opacity-70">Loading…</p>}
          {groups && groups.length === 0 && (
            <Card>
              <CardContent className="py-8 text-center text-sm opacity-70">
                No groups configured yet. Click "+ Add group" to set one up.
                <br />
                You'll need the Telegram chat_id (negative integer for groups).
              </CardContent>
            </Card>
          )}
          <div className="space-y-3">
            {groups?.map((g) => (
              <Card key={g.group_chat_id}>
                <CardHeader>
                  <div className="flex items-center justify-between">
                    <div>
                      <CardTitle className="text-lg flex items-center gap-2">
                        {g.name || `(unnamed) ${g.group_chat_id}`}
                        <Badge variant={modeBadgeVariant(g.mode)}>{g.mode}</Badge>
                        <Badge variant="outline">{g.kind}</Badge>
                        {!g.enabled && <Badge variant="destructive">paused</Badge>}
                      </CardTitle>
                      <p className="text-xs opacity-60 mt-1">
                        chat_id <code>{g.group_chat_id}</code> · cooldown {g.cooldown_min}m
                        · daily max {g.daily_max_sent} · {g.trusted_users.length} trusted
                      </p>
                    </div>
                    <div className="flex gap-2">
                      <Button variant="outline" size="sm" onClick={() => setEditing(g)}>
                        Settings
                      </Button>
                      <Button
                        variant="ghost"
                        size="sm"
                        onClick={() => handleDelete(g)}
                      >
                        <Trash2 className="w-4 h-4" />
                      </Button>
                    </div>
                  </div>
                </CardHeader>
              </Card>
            ))}
          </div>
        </TabsContent>

        <TabsContent value="pending">
          {pending === null && <p className="text-sm opacity-70">Loading…</p>}
          {pending && pending.length === 0 && (
            <Card>
              <CardContent className="py-8 text-center text-sm opacity-70">
                No pending outbound. Mia is either silent or all her replies have
                been auto-sent (trust mode).
              </CardContent>
            </Card>
          )}
          <div className="space-y-3">
            {pending?.map((p) => (
              <PendingCard
                key={p.id}
                p={p}
                onApprove={handleApprove}
                onReject={handleReject}
              />
            ))}
          </div>
        </TabsContent>

        <TabsContent value="manual">
          <ManualPost groups={groups ?? []} onSent={load} />
        </TabsContent>
      </Tabs>

      {(editing || addingNew) && (
        <GroupConfigDialog
          initial={editing}
          isNew={addingNew}
          onClose={() => {
            setEditing(null)
            setAddingNew(false)
          }}
          onSaved={() => {
            setEditing(null)
            setAddingNew(false)
            void load()
          }}
        />
      )}

      {briefingOpen && (
        <BriefingImportDialog
          onClose={() => setBriefingOpen(false)}
          defaultGroupChatID={groups?.[0]?.group_chat_id ?? ""}
        />
      )}
    </div>
  )
}

// ---------- attachment preview ----------
//
// post_to_channel v2 (2026-05-12) lets the synth queue rich-media drafts
// (photo, video, album, poll, sticker, etc.). The actual file bytes
// live in the synth container's workspace — we can't render them
// directly from companion without a hub file proxy. For now we render
// metadata only (kind icon + path + caption + structured fields) so
// the operator at least knows WHAT was queued before approving.
// Phase-2 plan: hub proxy /api/instances/file?path=... → actual <img>
// previews for photo/album items.
function kindIcon(kind: AttachmentPayload["kind"]) {
  switch (kind) {
    case "photo":      return <Image className="w-4 h-4" />
    case "video":      return <Video className="w-4 h-4" />
    case "video_note": return <Video className="w-4 h-4" />
    case "voice":      return <Mic className="w-4 h-4" />
    case "audio":      return <Music className="w-4 h-4" />
    case "document":   return <FileText className="w-4 h-4" />
    case "sticker":    return <Smile className="w-4 h-4" />
    case "animation":  return <Film className="w-4 h-4" />
    case "album":      return <LayoutGrid className="w-4 h-4" />
    case "poll":       return <BarChart3 className="w-4 h-4" />
    default:           return <MessageSquare className="w-4 h-4" />
  }
}

function MediaThumb({ path, kind }: { path: string; kind: "photo" | "video" }) {
  // Synth's workspace files stream through hub /api/my/synth-fetch-raw.
  // For videos we'd need the file path of the .mp4 + a <video> tag; for
  // photos we just point an <img>. Both kinds render whatever bytes
  // the synth's /api/file returns.
  if (kind === "photo") {
    return (
      <img
        src={synthFileURL(path)}
        alt={path}
        className="rounded border max-h-40 object-cover"
        loading="lazy"
        onError={(e) => {
          // Fall through to a text-only marker when the file is
          // unreachable (synth offline / file deleted / path moved).
          (e.currentTarget as HTMLImageElement).style.display = "none"
        }}
      />
    )
  }
  return (
    <video
      src={synthFileURL(path)}
      className="rounded border max-h-40"
      controls
      preload="metadata"
    />
  )
}

function AttachmentPreview({ a }: { a: AttachmentPayload }) {
  const baseRow = "text-xs flex items-center gap-2 px-2 py-1 bg-muted/40 rounded"
  if (a.kind === "album") {
    return (
      <div className="space-y-2 border border-dashed rounded p-3">
        <div className="flex items-center gap-2 text-sm font-medium">
          {kindIcon("album")} Album · {a.items?.length ?? 0} items
        </div>
        <div className="grid grid-cols-3 gap-2">
          {a.items?.map((it, i) => (
            <div key={i} className="space-y-1">
              <MediaThumb path={it.path} kind={it.kind} />
              <div className="flex items-center gap-1 text-xs opacity-70">
                {kindIcon(it.kind)}
                <code className="truncate flex-1">{it.path.split("/").pop()}</code>
              </div>
              {it.caption && (
                <div className="text-xs opacity-60 truncate">“{it.caption}”</div>
              )}
            </div>
          ))}
        </div>
        {a.caption && (
          <div className="text-xs opacity-70 italic">album caption: {a.caption}</div>
        )}
      </div>
    )
  }
  if (a.kind === "poll") {
    return (
      <div className="space-y-2 border border-dashed rounded p-3">
        <div className="flex items-center gap-2 text-sm font-medium">
          {kindIcon("poll")} Poll
          {a.quiz && <Badge variant="outline">quiz</Badge>}
          {a.anonymous !== false && <Badge variant="outline">anon</Badge>}
          {a.multi && <Badge variant="outline">multi</Badge>}
        </div>
        <div className="text-sm font-medium">{a.question}</div>
        <ol className="text-xs list-decimal ml-5 space-y-0.5">
          {a.options?.map((o, i) => (
            <li key={i} className={a.quiz && a.correct_option === i ? "font-bold" : ""}>
              {o}{a.quiz && a.correct_option === i ? "  ✓" : ""}
            </li>
          ))}
        </ol>
        {a.explanation && (
          <div className="text-xs opacity-70 italic">explanation: {a.explanation}</div>
        )}
      </div>
    )
  }
  if (a.kind === "animation") {
    return (
      <div className="space-y-2 border border-dashed rounded p-3">
        <div className="flex items-center gap-2 text-sm font-medium">
          {kindIcon("animation")} Animation · GIF
        </div>
        {/* Giphy / public CDN URLs render straight from the browser */}
        {a.gif_url && (
          <img
            src={a.gif_url}
            alt="gif preview"
            className="max-h-48 rounded border"
            loading="lazy"
          />
        )}
        <code className="text-xs break-all opacity-70">{a.gif_url}</code>
      </div>
    )
  }
  if (a.kind === "sticker") {
    return (
      <div className={baseRow}>
        {kindIcon("sticker")} Sticker · file_id=<code className="truncate">{a.sticker_id}</code>
      </div>
    )
  }
  // Single-file kinds: photo, video, video_note, voice, audio, document
  // photo/video render an inline preview straight from the synth via
  // /api/my/synth-fetch-raw. Other kinds (voice/audio/document) are
  // metadata-only since browsers can't preview them in a useful way
  // and we'd just be wasting bandwidth fetching them.
  const showThumb = (a.kind === "photo" || a.kind === "video") && !!a.path
  return (
    <div className="space-y-2 border border-dashed rounded p-3">
      <div className="flex items-center gap-2 text-sm font-medium">
        {kindIcon(a.kind)} {a.kind}
        {(a.width || a.height) && (
          <span className="text-xs opacity-60">{a.width}×{a.height}</span>
        )}
      </div>
      {showThumb && (
        <MediaThumb path={a.path!} kind={a.kind as "photo" | "video"} />
      )}
      {a.path && (
        <code className="block text-xs break-all opacity-80">{a.path}</code>
      )}
      {a.caption && (
        <div className="text-xs opacity-70">caption: {a.caption}</div>
      )}
      {a.title && <div className="text-xs">title: {a.title}</div>}
      {a.performer && <div className="text-xs opacity-70">performer: {a.performer}</div>}
    </div>
  )
}

// ---------- pending card ----------
function PendingCard({
  p,
  onApprove,
  onReject,
}: {
  p: PendingOutbound
  onApprove: (id: number, finalText?: string) => void
  onReject: (id: number) => void
}) {
  const [editing, setEditing] = useState(false)
  const [text, setText] = useState(p.draft_text)
  const attachment = parseAttachment(p.attachment_json)
  return (
    <Card>
      <CardHeader>
        <CardTitle className="text-sm flex items-center justify-between">
          <span className="flex items-center gap-2">
            <MessageSquare className="w-4 h-4" />
            <code className="text-xs">{p.group_chat_id}</code>
            <Badge variant="outline">{p.trigger_kind}</Badge>
            {attachment && (
              <Badge variant="secondary" className="flex items-center gap-1">
                {kindIcon(attachment.kind)}
                {attachment.kind}
              </Badge>
            )}
          </span>
          <span className="text-xs opacity-60 flex items-center gap-1">
            <Clock className="w-3 h-3" />
            {new Date(p.created_at).toLocaleString()}
          </span>
        </CardTitle>
      </CardHeader>
      <CardContent className="space-y-3">
        {attachment && <AttachmentPreview a={attachment} />}
        {editing ? (
          <Textarea
            value={text}
            onChange={(e) => setText(e.target.value)}
            rows={6}
          />
        ) : (
          p.draft_text && (
            <div className="whitespace-pre-wrap text-sm bg-muted p-3 rounded">
              {p.draft_text}
            </div>
          )
        )}
        {p.reason && (
          <p className="text-xs opacity-60 italic">Reason: {p.reason}</p>
        )}
        <div className="flex gap-2 flex-wrap">
          {!editing && (
            <>
              <Button size="sm" onClick={() => onApprove(p.id)}>
                <Send className="w-4 h-4 mr-1" /> Send as-is
              </Button>
              <Button size="sm" variant="outline" onClick={() => setEditing(true)}>
                Edit
              </Button>
              <Button
                size="sm"
                variant="ghost"
                onClick={() => onReject(p.id)}
              >
                Reject
              </Button>
            </>
          )}
          {editing && (
            <>
              <Button size="sm" onClick={() => onApprove(p.id, text)}>
                Send edited
              </Button>
              <Button
                size="sm"
                variant="ghost"
                onClick={() => {
                  setText(p.draft_text)
                  setEditing(false)
                }}
              >
                Cancel
              </Button>
            </>
          )}
        </div>
      </CardContent>
    </Card>
  )
}

// ---------- manual post ----------
function ManualPost({
  groups,
  onSent,
}: {
  groups: GroupConfig[]
  onSent: () => void
}) {
  const [groupID, setGroupID] = useState("")
  const [text, setText] = useState("")
  const [sendNow, setSendNow] = useState(false)
  const [sending, setSending] = useState(false)

  const submit = async () => {
    if (!groupID || !text.trim()) return
    setSending(true)
    try {
      await queueOutboundManual(groupID, text, { sendNow })
      toast.success(sendNow ? "Sent" : "Queued for approval")
      setText("")
      onSent()
    } catch (e) {
      toast.error(String((e as Error).message ?? e))
    } finally {
      setSending(false)
    }
  }

  return (
    <Card>
      <CardHeader>
        <CardTitle>Manual post as Mia</CardTitle>
      </CardHeader>
      <CardContent className="space-y-3">
        <div>
          <Label>Group</Label>
          <Select value={groupID} onValueChange={setGroupID}>
            <SelectTrigger>
              <SelectValue placeholder="Pick group" />
            </SelectTrigger>
            <SelectContent>
              {groups.map((g) => (
                <SelectItem key={g.group_chat_id} value={g.group_chat_id}>
                  {g.name || g.group_chat_id} ({g.mode})
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
        <div>
          <Label>Text</Label>
          <Textarea
            value={text}
            onChange={(e) => setText(e.target.value)}
            rows={6}
            placeholder="What should she say?"
          />
        </div>
        <div className="flex items-center gap-2">
          <Switch checked={sendNow} onCheckedChange={setSendNow} />
          <Label className="text-sm">Send immediately (skip approval queue)</Label>
        </div>
        <Button onClick={submit} disabled={sending || !groupID || !text.trim()}>
          {sending ? "Sending…" : sendNow ? "Send" : "Queue for approval"}
        </Button>
      </CardContent>
    </Card>
  )
}

// ---------- settings dialog ----------
function GroupConfigDialog({
  initial,
  isNew,
  onClose,
  onSaved,
}: {
  initial: GroupConfig | null
  isNew: boolean
  onClose: () => void
  onSaved: () => void
}) {
  const blank: GroupConfig = {
    group_chat_id: "",
    name: "",
    kind: "group",
    mode: "strict",
    bot_privacy: "on",
    triggers: { mention: true, reply: true, scheduled: false, topic: false },
    cooldown_min: 30,
    daily_max_sent: 20,
    trusted_users: [],
    banned_topics: [],
    tone_overlay: "",
    private_memory_blocked: true,
    enabled: true,
    model_override: "",
    memory_addendum: "",
    soul_full_override: "",
    soul_addendum: "",
    allowed_tools: "",
    trigger_mode: "selective",
    created_at: "",
    updated_at: "",
  }
  const [cfg, setCfg] = useState<GroupConfig>(initial ?? blank)
  const [saving, setSaving] = useState(false)
  const [trustedRaw, setTrustedRaw] = useState(
    (initial?.trusted_users ?? []).join(", "),
  )
  const [bannedRaw, setBannedRaw] = useState(
    (initial?.banned_topics ?? []).join("\n"),
  )

  const save = async () => {
    setSaving(true)
    try {
      const payload: Partial<GroupConfig> = {
        ...cfg,
        trusted_users: trustedRaw
          .split(",")
          .map((s) => s.trim())
          .filter(Boolean),
        banned_topics: bannedRaw
          .split("\n")
          .map((s) => s.trim())
          .filter(Boolean),
      }
      await upsertGroupConfig(payload)
      toast.success("Saved")
      onSaved()
    } catch (e) {
      toast.error(String((e as Error).message ?? e))
    } finally {
      setSaving(false)
    }
  }

  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="max-w-2xl max-h-[90vh] overflow-y-auto">
        <DialogHeader>
          <DialogTitle>
            {isNew ? "Add new group" : `Settings: ${cfg.name || cfg.group_chat_id}`}
          </DialogTitle>
          <DialogDescription>
            Per-group controls for Mia's behavior in this Telegram chat.
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-4">
          <div className="grid grid-cols-2 gap-3">
            <div>
              <Label>Chat ID (negative for groups)</Label>
              <Input
                value={cfg.group_chat_id}
                onChange={(e) =>
                  setCfg({ ...cfg, group_chat_id: e.target.value })
                }
                placeholder="-12345 or -1001234567890"
                disabled={!isNew}
              />
            </div>
            <div>
              <Label>Display name</Label>
              <Input
                value={cfg.name}
                onChange={(e) => setCfg({ ...cfg, name: e.target.value })}
                placeholder="My friends chat"
              />
            </div>
            <div>
              <Label>Kind</Label>
              <Select
                value={cfg.kind}
                onValueChange={(v) =>
                  setCfg({ ...cfg, kind: v as GroupConfig["kind"] })
                }
              >
                <SelectTrigger>
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="group">group</SelectItem>
                  <SelectItem value="channel">channel</SelectItem>
                  <SelectItem value="discussion">discussion</SelectItem>
                </SelectContent>
              </Select>
            </div>
            <div>
              <Label>Mode</Label>
              <Select
                value={cfg.mode}
                onValueChange={(v) =>
                  setCfg({ ...cfg, mode: v as GroupConfig["mode"] })
                }
              >
                <SelectTrigger>
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="strict">strict (approve all)</SelectItem>
                  <SelectItem value="soft">soft (queue proactive)</SelectItem>
                  <SelectItem value="trust">trust (autonomous)</SelectItem>
                </SelectContent>
              </Select>
            </div>
          </div>

          <Separator />

          <div className="grid grid-cols-2 gap-3">
            <div>
              <Label>Cooldown (min between sends)</Label>
              <Input
                type="number"
                value={cfg.cooldown_min}
                onChange={(e) =>
                  setCfg({ ...cfg, cooldown_min: parseInt(e.target.value) || 0 })
                }
              />
            </div>
            <div>
              <Label>Daily max sent</Label>
              <Input
                type="number"
                value={cfg.daily_max_sent}
                onChange={(e) =>
                  setCfg({ ...cfg, daily_max_sent: parseInt(e.target.value) || 0 })
                }
              />
            </div>
          </div>

          <Separator />

          <div className="space-y-3">
            <Label className="text-sm font-medium">Triggers (when she replies)</Label>
            {(["mention", "reply", "scheduled", "topic"] as const).map((k) => (
              <div key={k} className="flex items-center gap-2">
                <Switch
                  checked={cfg.triggers[k]}
                  onCheckedChange={(v) =>
                    setCfg({
                      ...cfg,
                      triggers: { ...cfg.triggers, [k]: v },
                    })
                  }
                />
                <Label className="text-sm">{k}</Label>
              </div>
            ))}
          </div>

          <Separator />

          <div>
            <Label>
              Trusted users (comma-separated tg user_ids — no `tg_user_` prefix)
            </Label>
            <Textarea
              value={trustedRaw}
              onChange={(e) => setTrustedRaw(e.target.value)}
              placeholder="7392742, 12345"
              rows={2}
            />
          </div>

          <div>
            <Label>Banned topics (one per line — NEVER discuss in this group)</Label>
            <Textarea
              value={bannedRaw}
              onChange={(e) => setBannedRaw(e.target.value)}
              placeholder={"Finances\nHealth\nExact addresses\n..."}
              rows={4}
            />
          </div>

          <div>
            <Label>Tone overlay (free text — extra prompt instructions for this group)</Label>
            <Textarea
              value={cfg.tone_overlay}
              onChange={(e) => setCfg({ ...cfg, tone_overlay: e.target.value })}
              placeholder="e.g. 'Casual friends chat — keep it light, no business talk'"
              rows={3}
            />
          </div>

          <div className="flex items-center gap-2">
            <Switch
              checked={cfg.private_memory_blocked}
              onCheckedChange={(v) =>
                setCfg({ ...cfg, private_memory_blocked: v })
              }
            />
            <Label>
              Block private-memory leakage (refuse to quote 1:1 conversation content here)
            </Label>
          </div>

          <div className="flex items-center gap-2">
            <Switch
              checked={cfg.enabled}
              onCheckedChange={(v) => setCfg({ ...cfg, enabled: v })}
            />
            <Label>Enabled (off = drop all outbound + audit)</Label>
          </div>

          <Separator />

          <div className="space-y-1">
            <div className="text-sm font-medium">
              Per-channel overrides (advanced)
            </div>
            <div className="text-xs text-muted-foreground">
              Leave fields empty to inherit the synth's instance default.
              These take effect for messages in THIS channel only.
            </div>
          </div>

          <div>
            <Label>Trigger mode</Label>
            <Select
              value={cfg.trigger_mode || "selective"}
              onValueChange={(v) =>
                setCfg({
                  ...cfg,
                  trigger_mode: v as GroupConfig["trigger_mode"],
                })
              }
            >
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="selective">
                  selective — agent decides (default)
                </SelectItem>
                <SelectItem value="mention_or_reply">
                  mention_or_reply — hard gate
                </SelectItem>
                <SelectItem value="mention_only">
                  mention_only — hard gate
                </SelectItem>
                <SelectItem value="always">
                  always — answer every message
                </SelectItem>
              </SelectContent>
            </Select>
            <div className="text-xs text-muted-foreground mt-1">
              <strong>selective</strong>: must respond to mention/reply,
              MAY respond to others if she has something genuine to add.
              Bias toward silence.
              <br />
              <strong>mention_only</strong> / <strong>mention_or_reply</strong>:
              hard delivery-layer gate — non-matching messages are dropped
              before the LLM runs (no cost, no audit).
            </div>
          </div>

          <div>
            <Label>Model override (provider:model)</Label>
            <Input
              value={cfg.model_override}
              onChange={(e) =>
                setCfg({ ...cfg, model_override: e.target.value })
              }
              placeholder="openrouter:anthropic/claude-sonnet-4-6"
            />
            <div className="text-xs text-muted-foreground mt-1">
              Empty = inherit instance default. Format:{" "}
              <code>provider:model_id</code> (same as <code>/provider</code>{" "}
              command).
            </div>
          </div>

          <div>
            <Label>Allowed tools (comma-separated; empty = all)</Label>
            <Textarea
              value={cfg.allowed_tools}
              onChange={(e) =>
                setCfg({ ...cfg, allowed_tools: e.target.value })
              }
              placeholder="memory_recall, wiki_search, send_message"
              rows={2}
            />
            <div className="text-xs text-muted-foreground mt-1">
              When non-empty, ONLY these tools are visible to the LLM in this
              channel. Useful for read-only public channels (e.g. block{" "}
              <code>send_message</code>, <code>post_to_channel</code>).
            </div>
          </div>

          <div>
            <Label>Per-channel memory addendum</Label>
            <Textarea
              value={cfg.memory_addendum}
              onChange={(e) =>
                setCfg({ ...cfg, memory_addendum: e.target.value })
              }
              placeholder={
                "Free-form context for THIS channel:\n- who's in here, history with them\n- recurring topics\n- inside jokes / shared references\n- anything not in the graph but matters here"
              }
              rows={5}
            />
            <div className="text-xs text-muted-foreground mt-1">
              Injected into the turn's dynamic context. The synth treats this
              as operator-curated knowledge about this channel.
            </div>
          </div>

          <div>
            <Label>Per-channel SOUL addendum (additive)</Label>
            <Textarea
              value={cfg.soul_addendum}
              onChange={(e) =>
                setCfg({ ...cfg, soul_addendum: e.target.value })
              }
              placeholder="Light tweaks layered on top of the default persona — e.g. 'in this channel, lean technical and skip flirty asides.'"
              rows={3}
            />
            <div className="text-xs text-muted-foreground mt-1">
              Use this for SMALL persona tweaks. For a full persona
              replacement use the field below.
            </div>
          </div>

          <div>
            <Label>Per-channel SOUL FULL override (replaces persona)</Label>
            <Textarea
              value={cfg.soul_full_override}
              onChange={(e) =>
                setCfg({ ...cfg, soul_full_override: e.target.value })
              }
              placeholder="A complete different persona for this channel (e.g. formal corporate voice). Takes precedence over the addendum."
              rows={6}
            />
            <div className="text-xs text-muted-foreground mt-1">
              When non-empty, this REPLACES the default SOUL persona for
              messages in this channel. Name/identity stays; voice/style/
              topic boundaries get the override. Wins over addendum.
            </div>
          </div>
        </div>

        <DialogFooter>
          <Button variant="outline" onClick={onClose}>
            Cancel
          </Button>
          <Button onClick={save} disabled={saving || !cfg.group_chat_id}>
            {saving ? "Saving…" : "Save"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

// ---------- briefing import dialog ----------
function BriefingImportDialog({
  onClose,
  defaultGroupChatID,
}: {
  onClose: () => void
  defaultGroupChatID: string
}) {
  const [markdown, setMarkdown] = useState("")
  const [groupID, setGroupID] = useState(defaultGroupChatID)
  const [importing, setImporting] = useState(false)

  const submit = async () => {
    if (!markdown.trim()) return
    setImporting(true)
    try {
      const r = await importBriefing(undefined, markdown, groupID)
      toast.success(
        `Imported ${r.imported_count} members${r.failed_count > 0 ? `, ${r.failed_count} failed` : ""}`,
      )
      onClose()
    } catch (e) {
      toast.error(String((e as Error).message ?? e))
    } finally {
      setImporting(false)
    }
  }

  const example = `## @username (tg_id: 12345) — Имя Фамилия
**Relationship:** друг с 2018
**Trusted in this group:** yes
**Topics they care about:** ML, фотография
**Special notes:** легко обижается на сарказм
**Authority level:** trusted

## @another (tg_id: 67890) — Другое Имя
**Relationship:** коллега
**Topics they care about:** finance`

  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="max-w-2xl max-h-[90vh] overflow-y-auto">
        <DialogHeader>
          <DialogTitle>Import members briefing</DialogTitle>
          <DialogDescription>
            Paste markdown describing group members. Each section becomes a
            wiki entity page. Members marked as "trusted" are auto-added to
            the group's trusted_users list.
          </DialogDescription>
        </DialogHeader>
        <div className="space-y-3">
          <div>
            <Label>Default group_chat_id (for trusted_users updates)</Label>
            <Input
              value={groupID}
              onChange={(e) => setGroupID(e.target.value)}
              placeholder="-12345"
            />
          </div>
          <div>
            <Label>Markdown briefing</Label>
            <Textarea
              value={markdown}
              onChange={(e) => setMarkdown(e.target.value)}
              placeholder={example}
              rows={20}
              className="font-mono text-xs"
            />
            <p className="text-xs opacity-60 mt-1">
              Section header format: <code>## @user (tg_id: N) — Name</code>.
              Recognised fields: <code>**Relationship:**</code>,
              <code>**Trusted in this group:**</code>,
              <code>**Topics they care about:**</code>,
              <code>**Special notes:**</code>,
              <code>**Authority level:**</code>.
            </p>
          </div>
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={onClose}>
            Cancel
          </Button>
          <Button onClick={submit} disabled={importing || !markdown.trim()}>
            {importing ? "Importing…" : "Import"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
