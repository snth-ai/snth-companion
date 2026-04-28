import { useEffect, useState } from "react"
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
  CardDescription,
} from "@/components/ui/card"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Badge } from "@/components/ui/badge"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import { CheckCircle2, Trash2, Cpu, User } from "lucide-react"
import {
  fetchSynths,
  setActiveSynth,
  updateSynth,
  removeSynth,
  fetchCompanionConfig,
  updateCompanionConfig,
  type SynthPair,
  type CompanionConfig,
} from "@/lib/api"

// Synths page: lists every paired synth, lets the user switch active,
// edit per-pair role/label/tags, and configure THIS companion's role +
// tags (synth-host vs user-device, freeform tags like "file-storage").

export function SynthsPage() {
  const [synths, setSynths] = useState<SynthPair[]>([])
  const [activeID, setActiveID] = useState<string>("")
  const [companion, setCompanion] = useState<CompanionConfig | null>(null)
  const [err, setErr] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  const reload = async () => {
    setErr(null)
    try {
      const [s, c] = await Promise.all([fetchSynths(), fetchCompanionConfig()])
      setSynths(s.synths ?? [])
      setActiveID(s.active_synth_id ?? "")
      setCompanion(c)
    } catch (e) {
      setErr(String(e))
    }
  }

  useEffect(() => {
    reload()
  }, [])

  const onActivate = async (id: string) => {
    if (id === activeID) return
    setBusy(true)
    try {
      await setActiveSynth(id)
      await reload()
    } catch (e) {
      setErr(String(e))
    } finally {
      setBusy(false)
    }
  }

  const onUpdatePair = async (
    id: string,
    patch: { label?: string; role?: string; tags?: string[] },
  ) => {
    setBusy(true)
    try {
      await updateSynth(id, patch)
      await reload()
    } catch (e) {
      setErr(String(e))
    } finally {
      setBusy(false)
    }
  }

  const onRemovePair = async (id: string) => {
    if (!window.confirm("Remove this synth pairing? This Mac will disconnect from it.")) return
    setBusy(true)
    try {
      await removeSynth(id)
      await reload()
    } catch (e) {
      setErr(String(e))
    } finally {
      setBusy(false)
    }
  }

  const onUpdateCompanion = async (patch: { role?: string; tags?: string[] }) => {
    setBusy(true)
    try {
      await updateCompanionConfig(patch)
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
        <h1 className="text-2xl font-semibold tracking-tight">Synths</h1>
        <p className="text-sm text-muted-foreground mt-1">
          Paired synths and this companion's role. Active synth is the one this
          companion's WS connection currently serves.
        </p>
      </div>

      {err && (
        <Alert variant="destructive">
          <AlertTitle>Error</AlertTitle>
          <AlertDescription>{err}</AlertDescription>
        </Alert>
      )}

      {/* This companion's identity ------------------------------------ */}
      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <Cpu className="h-4 w-4" /> This companion
          </CardTitle>
          <CardDescription>
            Role and tags advertised to every paired synth. Drives where
            synths send remote_* tool calls when they have multiple companions
            online.
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-4">
          <CompanionRoleEditor
            companion={companion}
            busy={busy}
            onUpdate={onUpdateCompanion}
          />
          <CompanionTagsEditor
            companion={companion}
            busy={busy}
            onUpdate={onUpdateCompanion}
          />
        </CardContent>
      </Card>

      {/* Per-pair list ------------------------------------------------- */}
      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <User className="h-4 w-4" /> Paired synths
          </CardTitle>
          <CardDescription>
            One row per pairing. Click a row to make it active — the WS client
            reconnects to that synth's URL.
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-3">
          {synths.length === 0 && (
            <div className="text-sm text-muted-foreground italic">
              No synths paired yet. Use the Pair page to add one.
            </div>
          )}
          {synths.map((s) => (
            <SynthRow
              key={s.id}
              pair={s}
              active={s.id === activeID}
              busy={busy}
              onActivate={() => onActivate(s.id)}
              onUpdate={(patch) => onUpdatePair(s.id, patch)}
              onRemove={() => onRemovePair(s.id)}
            />
          ))}
        </CardContent>
      </Card>
    </div>
  )
}

// --- subcomponents ----------------------------------------------------

function CompanionRoleEditor({
  companion,
  busy,
  onUpdate,
}: {
  companion: CompanionConfig | null
  busy: boolean
  onUpdate: (p: { role: string }) => void
}) {
  const role = companion?.role ?? ""
  return (
    <div className="flex items-center gap-3">
      <span className="text-sm font-medium w-24">Role</span>
      <Select
        value={role}
        onValueChange={(v) => onUpdate({ role: v })}
        disabled={busy}
      >
        <SelectTrigger className="w-[220px]">
          <SelectValue placeholder="Pick a role" />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value="synth-host">synth-host (synth lives here)</SelectItem>
          <SelectItem value="user-device">user-device (your daily box)</SelectItem>
          <SelectItem value="shared">shared (multi-tenant / office)</SelectItem>
        </SelectContent>
      </Select>
    </div>
  )
}

function CompanionTagsEditor({
  companion,
  busy,
  onUpdate,
}: {
  companion: CompanionConfig | null
  busy: boolean
  onUpdate: (p: { tags: string[] }) => void
}) {
  const tags = companion?.tags ?? []
  const [draft, setDraft] = useState("")
  const addTag = () => {
    const t = draft.trim()
    if (!t) return
    if (tags.includes(t)) {
      setDraft("")
      return
    }
    onUpdate({ tags: [...tags, t] })
    setDraft("")
  }
  const removeTag = (t: string) => onUpdate({ tags: tags.filter((x) => x !== t) })
  return (
    <div className="flex items-start gap-3">
      <span className="text-sm font-medium w-24 pt-1.5">Tags</span>
      <div className="flex-1 space-y-2">
        <div className="flex flex-wrap gap-1.5">
          {tags.length === 0 && (
            <span className="text-xs text-muted-foreground italic">
              No tags. Examples: file-storage, home-nas, battery-only.
            </span>
          )}
          {tags.map((t) => (
            <Badge
              key={t}
              variant="secondary"
              className="cursor-pointer"
              onClick={() => removeTag(t)}
              title="Click to remove"
            >
              {t} ×
            </Badge>
          ))}
        </div>
        <div className="flex gap-2">
          <Input
            placeholder="Add a tag…"
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault()
                addTag()
              }
            }}
            disabled={busy}
            className="max-w-xs"
          />
          <Button onClick={addTag} disabled={busy || !draft.trim()} size="sm">
            Add
          </Button>
        </div>
      </div>
    </div>
  )
}

function SynthRow({
  pair,
  active,
  busy,
  onActivate,
  onUpdate,
  onRemove,
}: {
  pair: SynthPair
  active: boolean
  busy: boolean
  onActivate: () => void
  onUpdate: (p: { label?: string; role?: string; tags?: string[] }) => void
  onRemove: () => void
}) {
  const [draftTag, setDraftTag] = useState("")
  const tags = pair.tags ?? []
  return (
    <div
      className={`rounded-md border p-3 ${
        active ? "border-primary bg-primary/5" : "border-border"
      }`}
    >
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2">
            {active && <CheckCircle2 className="h-4 w-4 text-primary" />}
            <code className="text-sm font-mono">{pair.label || pair.id}</code>
            <Badge variant="outline" className="text-xs">
              {pair.role}
            </Badge>
          </div>
          <div className="text-xs text-muted-foreground mt-1 truncate">
            {pair.url}
          </div>
        </div>
        <div className="flex gap-2 shrink-0">
          {!active && (
            <Button
              size="sm"
              variant="outline"
              onClick={onActivate}
              disabled={busy}
            >
              Activate
            </Button>
          )}
          <Button
            size="sm"
            variant="ghost"
            onClick={onRemove}
            disabled={busy}
            title="Remove pairing"
          >
            <Trash2 className="h-4 w-4" />
          </Button>
        </div>
      </div>

      <div className="mt-3 grid grid-cols-1 md:grid-cols-2 gap-3">
        <div className="flex items-center gap-2">
          <span className="text-xs text-muted-foreground w-16">Role</span>
          <Select
            value={pair.role}
            onValueChange={(v) => onUpdate({ role: v })}
            disabled={busy}
          >
            <SelectTrigger className="h-8 text-xs">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="primary">primary</SelectItem>
              <SelectItem value="secondary">secondary</SelectItem>
              <SelectItem value="test">test</SelectItem>
            </SelectContent>
          </Select>
        </div>
        <div className="flex items-center gap-2">
          <span className="text-xs text-muted-foreground w-16">Label</span>
          <Input
            className="h-8 text-xs"
            placeholder={pair.id}
            defaultValue={pair.label ?? ""}
            onBlur={(e) => {
              const v = e.target.value.trim()
              if (v !== (pair.label ?? "")) onUpdate({ label: v })
            }}
            disabled={busy}
          />
        </div>
      </div>

      <div className="mt-3 flex items-start gap-2">
        <span className="text-xs text-muted-foreground w-16 pt-1">Tags</span>
        <div className="flex-1 space-y-1.5">
          <div className="flex flex-wrap gap-1">
            {tags.map((t) => (
              <Badge
                key={t}
                variant="secondary"
                className="text-xs cursor-pointer"
                onClick={() => onUpdate({ tags: tags.filter((x) => x !== t) })}
                title="Click to remove"
              >
                {t} ×
              </Badge>
            ))}
            {tags.length === 0 && (
              <span className="text-xs text-muted-foreground italic">
                no tags
              </span>
            )}
          </div>
          <div className="flex gap-1.5">
            <Input
              placeholder="Add tag…"
              className="h-7 text-xs max-w-[12rem]"
              value={draftTag}
              onChange={(e) => setDraftTag(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  e.preventDefault()
                  const t = draftTag.trim()
                  if (t && !tags.includes(t)) {
                    onUpdate({ tags: [...tags, t] })
                  }
                  setDraftTag("")
                }
              }}
              disabled={busy}
            />
          </div>
        </div>
      </div>
    </div>
  )
}
