import { useEffect, useMemo, useState } from "react"
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Textarea } from "@/components/ui/textarea"
import {
  FileCode,
  Plus,
  Trash2,
  Save,
  XCircle,
  RefreshCw,
} from "lucide-react"
import {
  createTaskTemplate,
  deleteTaskTemplate,
  fetchTaskTemplates,
  patchTaskTemplate,
  type TaskTemplate,
  type TaskTemplateInput,
} from "@/lib/api"
import { toast } from "sonner"

// TaskTemplates — CRUD for the shared task-template library (SPEC §5.3).
// Templates carry the Liquid prompt + default agent config (sub_agent_kind,
// budget caps, hooks) + suggested_keywords (for synth-side auto-pick).
//
// Layout: list on the left, editor on the right.

type AgentConfig = {
  sub_agent_kind?: string
  max_cost_usd?: number
  max_wall_minutes?: number
  stall_timeout_ms?: number
  extra_env?: Record<string, string>
}

const DEFAULT_PROMPT = `You are a coding sub-agent.

Task: {{ task.title }}

{{ task.description }}

Constraints:
- Stay within the workspace at {{ task.id }}.
- Respond with INPUT REQUIRED: <question> if you need clarification.
- Print "Total tokens: N" and "Cost: $X.XX" at the end.
`

export function TaskTemplatesPage() {
  const [templates, setTemplates] = useState<TaskTemplate[]>([])
  const [selectedID, setSelectedID] = useState<string | null>(null)
  const [err, setErr] = useState<string | null>(null)
  const [refreshing, setRefreshing] = useState(false)
  const [busy, setBusy] = useState(false)
  const [editing, setEditing] = useState(false)
  const [buf, setBuf] = useState({
    name: "",
    description: "",
    prompt_template: "",
    suggested_keywords: "",
    default_agent_config: "",
  })

  const load = async () => {
    setRefreshing(true)
    try {
      const d = await fetchTaskTemplates()
      setTemplates(d.templates ?? [])
      setErr(null)
    } catch (e) {
      setErr(String((e as Error).message ?? e))
    } finally {
      setRefreshing(false)
    }
  }

  useEffect(() => {
    void load()
  }, [])

  const selected = useMemo(
    () => templates.find((t) => t.id === selectedID) ?? null,
    [templates, selectedID],
  )

  useEffect(() => {
    if (selected) {
      setBuf({
        name: selected.name,
        description: selected.description ?? "",
        prompt_template: selected.prompt_template,
        suggested_keywords: pretty(selected.suggested_keywords),
        default_agent_config: prettyJSON(selected.default_agent_config),
      })
      setEditing(false)
    }
  }, [selectedID]) // eslint-disable-line react-hooks/exhaustive-deps

  const startNew = () => {
    setSelectedID(null)
    setBuf({
      name: "",
      description: "",
      prompt_template: DEFAULT_PROMPT,
      suggested_keywords: "[]",
      default_agent_config: JSON.stringify(
        { sub_agent_kind: "claude", max_wall_minutes: 60 } as AgentConfig,
        null,
        2,
      ),
    })
    setEditing(true)
  }

  const save = async () => {
    if (!buf.name.trim()) {
      toast.error("name is required")
      return
    }
    let kw: string[] = []
    let cfg: Record<string, unknown> = {}
    try {
      kw = parseKeywords(buf.suggested_keywords)
    } catch (e) {
      toast.error("suggested_keywords: " + (e as Error).message)
      return
    }
    try {
      cfg = parseJSON(buf.default_agent_config)
    } catch (e) {
      toast.error("default_agent_config: " + (e as Error).message)
      return
    }
    setBusy(true)
    try {
      if (selected) {
        await patchTaskTemplate(selected.id, {
          name: buf.name.trim(),
          description: buf.description,
          prompt_template: buf.prompt_template,
          suggested_keywords: kw,
          default_agent_config: cfg,
        })
        toast.success("template updated")
      } else {
        const input: TaskTemplateInput = {
          name: buf.name.trim(),
          description: buf.description,
          prompt_template: buf.prompt_template,
          suggested_keywords: kw,
          default_agent_config: cfg,
        }
        const created = await createTaskTemplate(input)
        toast.success(`template "${created.name}" created`)
        setSelectedID(created.id)
      }
      await load()
      setEditing(false)
    } catch (e) {
      toast.error(String((e as Error).message ?? e))
    } finally {
      setBusy(false)
    }
  }

  const remove = async () => {
    if (!selected) return
    if (!confirm(`Delete template "${selected.name}"?`)) return
    setBusy(true)
    try {
      await deleteTaskTemplate(selected.id)
      toast.success("deleted")
      setSelectedID(null)
      await load()
    } catch (e) {
      toast.error(String((e as Error).message ?? e))
    } finally {
      setBusy(false)
    }
  }

  const isNew = !selected && editing

  return (
    <div className="space-y-3">
      {err && (
        <Alert variant="destructive">
          <AlertTitle>Error</AlertTitle>
          <AlertDescription>{err}</AlertDescription>
        </Alert>
      )}

      <div className="flex items-end justify-between gap-3 flex-wrap">
        <div>
          <h1 className="text-2xl font-semibold tracking-tight">
            Task Templates
          </h1>
          <p className="text-sm text-muted-foreground mt-1">
            Liquid prompts + default agent config. Variables:{" "}
            <code className="px-1 py-0.5 bg-muted rounded text-xs">
              {"{{ task.title }}"}
            </code>{" "}
            <code className="px-1 py-0.5 bg-muted rounded text-xs">
              {"{{ task.description }}"}
            </code>{" "}
            <code className="px-1 py-0.5 bg-muted rounded text-xs">
              {"{{ overrides.X }}"}
            </code>
          </p>
        </div>
        <div className="flex items-center gap-2">
          <Button
            size="sm"
            variant="ghost"
            onClick={() => void load()}
            disabled={refreshing}
          >
            <RefreshCw
              className={"h-4 w-4 mr-1 " + (refreshing ? "animate-spin" : "")}
            />
            Refresh
          </Button>
          <Button size="sm" onClick={startNew}>
            <Plus className="h-4 w-4 mr-1" /> New template
          </Button>
        </div>
      </div>

      <div className="grid grid-cols-1 lg:grid-cols-[280px_1fr] gap-3">
        <div className="space-y-1 max-h-[80vh] overflow-y-auto pr-1">
          {templates.length === 0 && (
            <div className="text-xs italic text-muted-foreground px-2 py-3">
              no templates yet — hit "+ New template"
            </div>
          )}
          {templates.map((t) => (
            <button
              key={t.id}
              onClick={() => setSelectedID(t.id)}
              className={
                "w-full text-left rounded-md px-3 py-2 text-sm transition-colors block " +
                (selectedID === t.id
                  ? "bg-primary/15 text-foreground"
                  : "hover:bg-muted text-muted-foreground hover:text-foreground")
              }
            >
              <div className="flex items-center gap-2">
                <FileCode className="h-3.5 w-3.5 shrink-0" />
                <span className="font-medium text-foreground line-clamp-1 flex-1">
                  {t.name || t.id}
                </span>
              </div>
              {t.description && (
                <div className="text-xs text-muted-foreground/80 line-clamp-1 mt-0.5">
                  {t.description}
                </div>
              )}
              <div className="text-[10px] text-muted-foreground/60 font-mono mt-0.5">
                {t.id}
              </div>
            </button>
          ))}
        </div>

        <div>
          {!selected && !isNew && (
            <Card className="min-h-[60vh]">
              <CardContent className="pt-6 text-sm text-muted-foreground italic">
                Pick a template from the list — or hit "+ New template" to make
                one.
              </CardContent>
            </Card>
          )}

          {(selected || isNew) && (
            <Card>
              <CardHeader className="pb-2">
                <CardTitle className="flex items-center gap-2 flex-wrap">
                  {editing ? (
                    <Input
                      value={buf.name}
                      onChange={(e) =>
                        setBuf((s) => ({ ...s, name: e.target.value }))
                      }
                      className="text-lg flex-1"
                      placeholder="template name"
                    />
                  ) : (
                    <span className="flex-1">{selected?.name}</span>
                  )}
                  {selected && !editing && (
                    <Badge variant="outline" className="font-mono">
                      {selected.id}
                    </Badge>
                  )}
                  <div className="flex items-center gap-1">
                    {!editing && selected && (
                      <>
                        <Button
                          size="sm"
                          variant="ghost"
                          onClick={() => setEditing(true)}
                          disabled={busy}
                        >
                          Edit
                        </Button>
                        <Button
                          size="sm"
                          variant="ghost"
                          className="text-red-400"
                          onClick={remove}
                          disabled={busy}
                        >
                          <Trash2 className="h-4 w-4" />
                        </Button>
                      </>
                    )}
                    {editing && (
                      <>
                        <Button size="sm" onClick={save} disabled={busy}>
                          <Save className="h-4 w-4 mr-1" />
                          {busy ? "saving…" : "Save"}
                        </Button>
                        <Button
                          size="sm"
                          variant="ghost"
                          onClick={() => {
                            if (selected) {
                              setBuf({
                                name: selected.name,
                                description: selected.description ?? "",
                                prompt_template: selected.prompt_template,
                                suggested_keywords: pretty(
                                  selected.suggested_keywords,
                                ),
                                default_agent_config: prettyJSON(
                                  selected.default_agent_config,
                                ),
                              })
                              setEditing(false)
                            } else {
                              setSelectedID(null)
                              setEditing(false)
                            }
                          }}
                          disabled={busy}
                        >
                          <XCircle className="h-4 w-4" />
                        </Button>
                      </>
                    )}
                  </div>
                </CardTitle>
              </CardHeader>
              <CardContent className="space-y-4">
                <div className="space-y-1">
                  <Label className="text-xs uppercase tracking-wider">
                    Description
                  </Label>
                  {editing ? (
                    <Input
                      value={buf.description}
                      onChange={(e) =>
                        setBuf((s) => ({ ...s, description: e.target.value }))
                      }
                      placeholder="what this template is for"
                    />
                  ) : (
                    <div className="text-sm text-muted-foreground">
                      {selected?.description || (
                        <span className="italic">no description</span>
                      )}
                    </div>
                  )}
                </div>

                <div className="space-y-1">
                  <Label className="text-xs uppercase tracking-wider">
                    Prompt template (Liquid)
                  </Label>
                  {editing ? (
                    <Textarea
                      value={buf.prompt_template}
                      onChange={(e) =>
                        setBuf((s) => ({
                          ...s,
                          prompt_template: e.target.value,
                        }))
                      }
                      className="min-h-72 font-mono text-xs"
                    />
                  ) : (
                    <pre className="text-xs whitespace-pre-wrap font-mono p-3 bg-muted/30 rounded-md max-h-72 overflow-y-auto">
                      {selected?.prompt_template}
                    </pre>
                  )}
                </div>

                <div className="grid grid-cols-1 lg:grid-cols-2 gap-3">
                  <div className="space-y-1">
                    <Label className="text-xs uppercase tracking-wider">
                      Suggested keywords (JSON array)
                    </Label>
                    {editing ? (
                      <Textarea
                        value={buf.suggested_keywords}
                        onChange={(e) =>
                          setBuf((s) => ({
                            ...s,
                            suggested_keywords: e.target.value,
                          }))
                        }
                        className="min-h-20 font-mono text-xs"
                        placeholder='["refactor","go","cleanup"]'
                      />
                    ) : (
                      <div className="flex flex-wrap gap-1">
                        {parseKeywordsSafe(
                          selected?.suggested_keywords ?? "[]",
                        ).map((kw) => (
                          <Badge key={kw} variant="secondary">
                            {kw}
                          </Badge>
                        ))}
                      </div>
                    )}
                  </div>
                  <div className="space-y-1">
                    <Label className="text-xs uppercase tracking-wider">
                      Default agent config (JSON)
                    </Label>
                    {editing ? (
                      <Textarea
                        value={buf.default_agent_config}
                        onChange={(e) =>
                          setBuf((s) => ({
                            ...s,
                            default_agent_config: e.target.value,
                          }))
                        }
                        className="min-h-20 font-mono text-xs"
                        placeholder='{"sub_agent_kind":"claude","max_wall_minutes":60}'
                      />
                    ) : (
                      <pre className="text-xs whitespace-pre-wrap font-mono p-3 bg-muted/30 rounded-md">
                        {selected?.default_agent_config || "{}"}
                      </pre>
                    )}
                  </div>
                </div>

                {selected && !editing && (
                  <div className="text-[10px] text-muted-foreground/60 font-mono pt-2 border-t border-border">
                    created {selected.created_at} · updated{" "}
                    {selected.updated_at} · by {selected.created_by_synth_id}
                  </div>
                )}
              </CardContent>
            </Card>
          )}
        </div>
      </div>
    </div>
  )
}

// --- helpers ---------------------------------------------------------

function pretty(jsonStr: string): string {
  if (!jsonStr) return "[]"
  try {
    return JSON.stringify(JSON.parse(jsonStr), null, 2)
  } catch {
    return jsonStr
  }
}

function prettyJSON(jsonStr: string): string {
  if (!jsonStr || jsonStr === "null") return "{}"
  try {
    return JSON.stringify(JSON.parse(jsonStr), null, 2)
  } catch {
    return jsonStr
  }
}

function parseKeywords(s: string): string[] {
  const trimmed = s.trim()
  if (!trimmed) return []
  // Accept JSON array or comma-separated.
  if (trimmed.startsWith("[")) {
    const v = JSON.parse(trimmed)
    if (!Array.isArray(v)) throw new Error("must be a JSON array of strings")
    return v.map((x) => String(x))
  }
  return trimmed
    .split(",")
    .map((x) => x.trim())
    .filter(Boolean)
}

function parseKeywordsSafe(s: string): string[] {
  try {
    return parseKeywords(s)
  } catch {
    return []
  }
}

function parseJSON(s: string): Record<string, unknown> {
  const trimmed = s.trim()
  if (!trimmed) return {}
  const v = JSON.parse(trimmed)
  if (typeof v !== "object" || Array.isArray(v) || v === null) {
    throw new Error("must be a JSON object")
  }
  return v as Record<string, unknown>
}
