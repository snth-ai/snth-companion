import { useEffect, useMemo, useState } from "react"
import { Sparkles, Plus, RefreshCw, Trash2, AlertCircle, FileCode2 } from "lucide-react"
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
import { Textarea } from "@/components/ui/textarea"
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
  fetchSkills,
  upsertSkill,
  deleteSkill,
  reloadSkills,
  type SkillView,
  type SkillUpsertPayload,
} from "@/lib/api"
import { toast } from "sonner"

// SkillsPage — author / edit / delete / reload synth-runtime skills.
// Skill = manifest.json (name, description, command/script, optional
// io_schema) + a script file (Python/Bash/JS) + optional SKILL.md.
//
// Baked skills (shipped with the synth image) show up read-only;
// runtime skills (written into the synth's data/ volume) are editable
// + deletable. Reload picks up the new set without a synth restart.

const STARTER_MANIFEST = `{
  "name": "<skill_name>",
  "description": "What the skill does, written for the LLM to read.",
  "command": "main.py",
  "io_schema": {
    "type": "object",
    "properties": {
      "input": { "type": "string", "description": "..." }
    },
    "required": ["input"]
  }
}
`

const STARTER_SCRIPT_PY = `#!/usr/bin/env python3
# main.py — read JSON args from stdin, write JSON result to stdout.
import json, sys

args = json.load(sys.stdin)
# ... do work ...
print(json.dumps({"ok": True, "echo": args.get("input", "")}))
`

const STARTER_SCRIPT_SH = `#!/usr/bin/env bash
# Read JSON args from stdin via jq, write JSON result to stdout.
set -euo pipefail
input=$(jq -r .input)
echo "{\\"ok\\": true, \\"echo\\": \\"$input\\"}"
`

export function SkillsPage() {
  const [skills, setSkills] = useState<SkillView[]>([])
  const [runtimeDir, setRuntimeDir] = useState("")
  const [bakedDir, setBakedDir] = useState("")
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [editing, setEditing] = useState<SkillView | null>(null)
  const [creating, setCreating] = useState(false)
  const [busy, setBusy] = useState(false)

  const load = async () => {
    setLoading(true)
    setError(null)
    try {
      const r = await fetchSkills()
      setSkills(r.skills ?? [])
      setRuntimeDir(r.runtime_dir ?? "")
      setBakedDir(r.baked_dir ?? "")
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }

  useEffect(() => {
    load()
  }, [])

  const { runtimeSkills, bakedSkills } = useMemo(() => {
    const rt: SkillView[] = []
    const bk: SkillView[] = []
    for (const s of skills) {
      ;(s.source === "runtime" ? rt : bk).push(s)
    }
    return { runtimeSkills: rt, bakedSkills: bk }
  }, [skills])

  const handleDelete = async (s: SkillView) => {
    if (!confirm(`Delete skill "${s.name}"? The synth will lose access on next reload.`))
      return
    setBusy(true)
    try {
      await deleteSkill(s.name)
      toast.success(`${s.name} deleted`)
      await load()
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  const handleReload = async () => {
    setBusy(true)
    try {
      const r = await reloadSkills()
      toast.success(
        r.count !== undefined
          ? `Reloaded — ${r.count} skill(s) now active`
          : "Skills reloaded",
      )
      await load()
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="max-w-5xl mx-auto space-y-6 py-6 px-4">
      <div className="flex items-start justify-between">
        <div>
          <h1 className="text-2xl font-semibold flex items-center gap-2">
            <Sparkles className="w-6 h-6" />
            Skills
          </h1>
          <p className="text-sm text-muted-foreground mt-1 max-w-xl">
            Write custom skills your synth can call. Each skill = a{" "}
            <code>manifest.json</code> + a script file (Python / Bash / Node /
            whatever the container has). Skill name = tool name in the LLM
            schema.
          </p>
        </div>
        <div className="flex gap-2">
          <Button variant="outline" size="sm" onClick={handleReload} disabled={busy || loading}>
            <RefreshCw className={`w-4 h-4 mr-1 ${busy ? "animate-spin" : ""}`} />
            Reload
          </Button>
          <Button variant="outline" size="sm" onClick={load} disabled={loading}>
            Refresh list
          </Button>
          <Button size="sm" onClick={() => setCreating(true)}>
            <Plus className="w-4 h-4 mr-1" />
            New skill
          </Button>
        </div>
      </div>

      {error && (
        <Alert variant="destructive">
          <AlertTitle>Failed to load skills</AlertTitle>
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      )}

      <Card>
        <CardHeader>
          <CardTitle className="text-base">
            Runtime skills
            {runtimeDir && (
              <span className="text-xs text-muted-foreground font-normal ml-2">
                {runtimeDir}
              </span>
            )}
          </CardTitle>
        </CardHeader>
        <CardContent>
          {runtimeSkills.length === 0 && !loading && (
            <p className="text-sm text-muted-foreground text-center py-8">
              No runtime skills yet. Click <strong>New skill</strong> to write
              one. Templates auto-populate to give you a starting shape.
            </p>
          )}
          <div className="space-y-2">
            {runtimeSkills.map((s) => (
              <SkillRow
                key={s.name}
                skill={s}
                onEdit={() => setEditing(s)}
                onDelete={() => handleDelete(s)}
                busy={busy}
              />
            ))}
          </div>
        </CardContent>
      </Card>

      {bakedSkills.length > 0 && (
        <Card>
          <CardHeader>
            <CardTitle className="text-base text-muted-foreground">
              Baked skills (read-only)
              {bakedDir && (
                <span className="text-xs font-normal ml-2">{bakedDir}</span>
              )}
            </CardTitle>
          </CardHeader>
          <CardContent>
            <div className="space-y-2">
              {bakedSkills.map((s) => (
                <SkillRow
                  key={s.name}
                  skill={s}
                  onEdit={() => setEditing(s)}
                  onDelete={() => {}}
                  busy={busy}
                  readOnly
                />
              ))}
            </div>
          </CardContent>
        </Card>
      )}

      <SkillSheet
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

type SkillRowProps = {
  skill: SkillView
  onEdit: () => void
  onDelete: () => void
  busy: boolean
  readOnly?: boolean
}

function SkillRow({ skill, onEdit, onDelete, busy, readOnly = false }: SkillRowProps) {
  const description = useMemo(() => {
    if (!skill.manifest_json) return ""
    try {
      const m = JSON.parse(skill.manifest_json) as { description?: string }
      return m.description ?? ""
    } catch {
      return ""
    }
  }, [skill.manifest_json])
  return (
    <div className="flex items-start gap-3 p-3 rounded-md border border-border">
      <FileCode2 className="w-5 h-5 mt-0.5 shrink-0 text-muted-foreground" />
      <div className="flex-1 min-w-0">
        <div className="flex items-center gap-2 flex-wrap">
          <div className="font-medium font-mono text-sm">{skill.name}</div>
          {skill.script_name && (
            <Badge variant="outline" className="text-xs">
              {skill.script_name}
            </Badge>
          )}
          {readOnly && (
            <Badge variant="secondary" className="text-xs">
              baked
            </Badge>
          )}
          {skill.manifest_error && (
            <Badge variant="destructive" className="text-xs">
              <AlertCircle className="w-3 h-3 mr-1" /> manifest error
            </Badge>
          )}
        </div>
        {description && (
          <div className="text-xs text-muted-foreground mt-1 line-clamp-2">
            {description}
          </div>
        )}
        {skill.manifest_error && (
          <div className="text-xs text-destructive mt-1 font-mono">
            {skill.manifest_error}
          </div>
        )}
      </div>
      <div className="flex items-center gap-2 shrink-0">
        <Button variant="outline" size="sm" onClick={onEdit} disabled={busy}>
          {readOnly ? "View" : "Edit"}
        </Button>
        {!readOnly && (
          <Button
            variant="ghost"
            size="icon"
            onClick={onDelete}
            disabled={busy}
            title="Delete"
          >
            <Trash2 className="w-4 h-4 text-destructive" />
          </Button>
        )}
      </div>
    </div>
  )
}

type SkillSheetProps = {
  open: boolean
  existing: SkillView | null
  onClose: () => void
  onSaved: () => void
}

function SkillSheet({ open, existing, onClose, onSaved }: SkillSheetProps) {
  const [name, setName] = useState("")
  const [manifest, setManifest] = useState("")
  const [scriptName, setScriptName] = useState("")
  const [scriptContent, setScriptContent] = useState("")
  const [skillMd, setSkillMd] = useState("")
  const [saving, setSaving] = useState(false)
  const [err, setErr] = useState<string | null>(null)

  const readOnly = existing?.source === "baked"

  useEffect(() => {
    if (!open) return
    if (existing) {
      setName(existing.name)
      setManifest(existing.manifest_json ?? "")
      setScriptName(existing.script_name ?? "")
      setScriptContent(existing.script_content ?? "")
      setSkillMd(existing.skill_md ?? "")
    } else {
      setName("")
      setManifest(STARTER_MANIFEST)
      setScriptName("main.py")
      setScriptContent(STARTER_SCRIPT_PY)
      setSkillMd("")
    }
    setErr(null)
  }, [open, existing])

  const insertShTemplate = () => {
    setScriptName("main.sh")
    setScriptContent(STARTER_SCRIPT_SH)
  }
  const insertPyTemplate = () => {
    setScriptName("main.py")
    setScriptContent(STARTER_SCRIPT_PY)
  }

  const handleSave = async () => {
    if (!name.trim()) {
      setErr("name required")
      return
    }
    // Quick sanity-check that manifest is valid JSON before sending.
    try {
      JSON.parse(manifest)
    } catch (e) {
      setErr(`manifest.json is not valid JSON: ${e instanceof Error ? e.message : e}`)
      return
    }
    setSaving(true)
    setErr(null)
    try {
      const payload: SkillUpsertPayload = {
        name: name.trim(),
        manifest_json: manifest,
        script_name: scriptName.trim(),
        script_content: scriptContent,
        skill_md: skillMd,
      }
      await upsertSkill(payload)
      toast.success(
        `${payload.name} saved — click Reload at the top to make the synth pick it up.`,
      )
      onSaved()
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e))
    } finally {
      setSaving(false)
    }
  }

  return (
    <Sheet open={open} onOpenChange={(v) => !v && onClose()}>
      <SheetContent className="sm:max-w-2xl overflow-y-auto">
        <SheetHeader>
          <SheetTitle>
            {existing
              ? `${readOnly ? "View" : "Edit"}: ${existing.name}`
              : "New skill"}
          </SheetTitle>
          <SheetDescription>
            {readOnly
              ? "Baked skills are read-only — they ship with the synth image. To customize, copy the contents into a new runtime skill with a different name."
              : "Manifest is JSON. Script gets written into the skill's folder under the name you pick. After saving, click Reload at the top to pick it up."}
          </SheetDescription>
        </SheetHeader>

        <div className="space-y-4 py-4 px-4">
          <div className="space-y-1">
            <Label>Name</Label>
            <Input
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="my_skill"
              disabled={!!existing}
            />
            <p className="text-xs text-muted-foreground">
              Used as the tool name — must be valid identifier (letters,
              digits, underscores).
            </p>
          </div>

          <div className="space-y-1">
            <div className="flex items-center justify-between">
              <Label>manifest.json</Label>
              <span className="text-xs text-muted-foreground">
                LLM-facing description lives here
              </span>
            </div>
            <Textarea
              rows={10}
              value={manifest}
              onChange={(e) => setManifest(e.target.value)}
              className="font-mono text-xs"
              spellCheck={false}
              disabled={readOnly}
            />
          </div>

          <div className="space-y-1">
            <div className="flex items-center justify-between">
              <Label>Script file</Label>
              {!readOnly && (
                <div className="flex gap-2">
                  <button
                    type="button"
                    onClick={insertPyTemplate}
                    className="text-xs text-muted-foreground hover:text-foreground underline"
                  >
                    Python template
                  </button>
                  <button
                    type="button"
                    onClick={insertShTemplate}
                    className="text-xs text-muted-foreground hover:text-foreground underline"
                  >
                    Bash template
                  </button>
                </div>
              )}
            </div>
            <Input
              value={scriptName}
              onChange={(e) => setScriptName(e.target.value)}
              placeholder="main.py"
              className="font-mono text-xs"
              disabled={readOnly}
            />
            <Textarea
              rows={16}
              value={scriptContent}
              onChange={(e) => setScriptContent(e.target.value)}
              className="font-mono text-xs"
              spellCheck={false}
              disabled={readOnly}
            />
            <p className="text-xs text-muted-foreground">
              Synth container has <code>python3</code>, <code>node/npm</code>,{" "}
              <code>deno</code>, <code>jq</code>, <code>sqlite3</code>,{" "}
              <code>ffmpeg</code>, <code>curl</code>. Use any of them. Script
              reads JSON args from stdin and writes JSON result to stdout.
            </p>
          </div>

          <div className="space-y-1">
            <Label>SKILL.md (optional)</Label>
            <Textarea
              rows={4}
              value={skillMd}
              onChange={(e) => setSkillMd(e.target.value)}
              className="font-mono text-xs"
              spellCheck={false}
              placeholder="# Skill docs — extra context for the LLM beyond the manifest description (e.g. examples, edge cases, what NOT to do)."
              disabled={readOnly}
            />
          </div>

          {err && (
            <Alert variant="destructive">
              <AlertDescription>{err}</AlertDescription>
            </Alert>
          )}
        </div>

        <div className="flex gap-2 justify-end px-4 pb-4 mt-auto">
          <Button variant="outline" onClick={onClose} disabled={saving}>
            {readOnly ? "Close" : "Cancel"}
          </Button>
          {!readOnly && (
            <Button onClick={handleSave} disabled={saving}>
              {saving ? "Saving…" : existing ? "Save changes" : "Save skill"}
            </Button>
          )}
        </div>
      </SheetContent>
    </Sheet>
  )
}
