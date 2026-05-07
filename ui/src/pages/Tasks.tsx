import { useEffect, useMemo, useRef, useState } from "react"
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
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import {
  Plus,
  RefreshCw,
  XCircle,
  Send,
  Bot,
  Cpu,
  Cloud as CloudIcon,
  AlertCircle,
  CheckCircle2,
  Clock,
  Loader2,
  HelpCircle,
  Pause,
  ChevronRight,
} from "lucide-react"
import {
  cancelTask,
  createTask,
  fetchTask,
  fetchTaskEvents,
  fetchTasksList,
  fetchTaskTemplates,
  patchTask,
  provideTaskInput,
  TASK_STATES,
  type CreateTaskInput,
  type TaskEventRow,
  type TaskRow,
  type TaskState,
  type TaskTemplate,
} from "@/lib/api"
import { toast } from "sonner"

// Tasks — Kanban board over the hub-side tasks system (SPEC §11).
// One column per state. Cards show title + owner synth + age + cost.
// Click → slide-in detail panel with transcript-tail / events.
// Drag between columns triggers a state PATCH (only the manual moves
// the SPEC permits: backlog ↔ queued ↔ blocked). Other transitions are
// owned by the orchestrator/companion worker and rejected by the hub.

const COLS: Array<{
  state: TaskState
  label: string
  icon: typeof Clock
  tone: string
}> = [
  { state: "backlog", label: "Backlog", icon: Clock, tone: "text-zinc-400" },
  { state: "queued", label: "Queued", icon: Clock, tone: "text-blue-400" },
  { state: "claimed", label: "Claimed", icon: Cpu, tone: "text-amber-400" },
  { state: "running", label: "Running", icon: Loader2, tone: "text-emerald-400" },
  {
    state: "awaiting_input",
    label: "Awaiting input",
    icon: HelpCircle,
    tone: "text-purple-400",
  },
  { state: "blocked", label: "Blocked", icon: Pause, tone: "text-rose-400" },
  { state: "done", label: "Done", icon: CheckCircle2, tone: "text-emerald-500" },
  { state: "error", label: "Error", icon: AlertCircle, tone: "text-red-500" },
  {
    state: "cancelled",
    label: "Cancelled",
    icon: XCircle,
    tone: "text-zinc-500",
  },
]

const MANUAL_MOVE_OK: Record<string, TaskState[]> = {
  backlog: ["queued", "blocked"],
  queued: ["backlog", "blocked"],
  blocked: ["backlog", "queued"],
}

const KIND_OPTIONS = [
  { value: "claude", label: "Claude (claude -p)" },
  { value: "codex", label: "Codex (codex exec)" },
  { value: "gemini", label: "Gemini" },
  { value: "manual", label: "Manual" },
]

function relAge(iso?: string | null): string {
  if (!iso) return ""
  const t = new Date(iso).getTime()
  if (!isFinite(t)) return ""
  const sec = Math.max(0, Math.floor((Date.now() - t) / 1000))
  if (sec < 60) return `${sec}s`
  if (sec < 3600) return `${Math.floor(sec / 60)}m`
  if (sec < 86400) return `${Math.floor(sec / 3600)}h`
  return `${Math.floor(sec / 86400)}d`
}

function fmtCost(usd: number): string {
  if (!usd) return ""
  if (usd < 0.01) return "<$0.01"
  return `$${usd.toFixed(2)}`
}

function ownerLabel(owner?: string | null): string {
  if (!owner) return ""
  return owner.replace(/^(_)?(snth|snthai)?(_)?(bot)?_?/i, "").replace(/_bot$/i, "") ||
    owner
}

export function TasksPage() {
  const [tasks, setTasks] = useState<TaskRow[]>([])
  const [err, setErr] = useState<string | null>(null)
  const [selectedID, setSelectedID] = useState<string | null>(null)
  const [showCreate, setShowCreate] = useState(false)
  const [templates, setTemplates] = useState<TaskTemplate[]>([])
  const [refreshing, setRefreshing] = useState(false)
  const [draggingID, setDraggingID] = useState<string | null>(null)

  const load = async () => {
    setRefreshing(true)
    try {
      const d = await fetchTasksList({ limit: 500 })
      setTasks(d.tasks ?? [])
      setErr(null)
    } catch (e) {
      setErr(String((e as Error).message ?? e))
    } finally {
      setRefreshing(false)
    }
  }

  useEffect(() => {
    void load()
    void fetchTaskTemplates()
      .then((d) => setTemplates(d.templates ?? []))
      .catch(() => setTemplates([]))
    const h = setInterval(() => void load(), 5000)
    return () => clearInterval(h)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  const grouped = useMemo(() => {
    const m: Record<string, TaskRow[]> = {}
    for (const c of COLS) m[c.state] = []
    for (const t of tasks) {
      const k = m[t.state] ? t.state : "backlog"
      m[k].push(t)
    }
    for (const k of Object.keys(m)) {
      m[k].sort((a, b) => (b.updated_at < a.updated_at ? -1 : 1))
    }
    return m
  }, [tasks])

  const moveTask = async (task: TaskRow, target: TaskState) => {
    const ok = MANUAL_MOVE_OK[task.state]?.includes(target)
    if (!ok) {
      toast.error(
        `cannot move ${task.state} → ${target} manually (orchestrator owns that)`,
      )
      return
    }
    try {
      await patchTask(task.id, { state: target })
      toast.success(`${task.title || task.id} → ${target}`)
      void load()
    } catch (e) {
      toast.error(String((e as Error).message ?? e))
    }
  }

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
          <h1 className="text-2xl font-semibold tracking-tight">Tasks</h1>
          <p className="text-sm text-muted-foreground mt-1">
            Shared board across all paired synths. Synth orchestrator
            claims queued tasks, sub-agent runs in an isolated workspace.
          </p>
        </div>
        <div className="flex items-center gap-2">
          <Button size="sm" variant="ghost" onClick={() => void load()} disabled={refreshing}>
            <RefreshCw className={"h-4 w-4 mr-1 " + (refreshing ? "animate-spin" : "")} />
            Refresh
          </Button>
          <Button size="sm" onClick={() => setShowCreate(true)}>
            <Plus className="h-4 w-4 mr-1" /> New task
          </Button>
        </div>
      </div>

      <div className="overflow-x-auto -mx-2 px-2 pb-2">
        <div className="flex gap-3 min-w-max">
          {COLS.map((col) => (
            <Column
              key={col.state}
              col={col}
              items={grouped[col.state] ?? []}
              draggingID={draggingID}
              onDragStart={(id) => setDraggingID(id)}
              onDragEnd={() => setDraggingID(null)}
              onDrop={(targetState) => {
                if (!draggingID) return
                const t = tasks.find((x) => x.id === draggingID)
                if (!t) return
                if (t.state === targetState) return
                void moveTask(t, targetState)
                setDraggingID(null)
              }}
              onSelect={(id) => setSelectedID(id)}
              selectedID={selectedID}
            />
          ))}
        </div>
      </div>

      {selectedID && (
        <DetailPanel
          taskID={selectedID}
          onClose={() => setSelectedID(null)}
          onChanged={() => void load()}
        />
      )}

      <CreateDialog
        open={showCreate}
        templates={templates}
        onClose={() => setShowCreate(false)}
        onCreated={() => {
          setShowCreate(false)
          void load()
        }}
      />
    </div>
  )
}

function Column({
  col,
  items,
  draggingID,
  onDragStart,
  onDragEnd,
  onDrop,
  onSelect,
  selectedID,
}: {
  col: { state: TaskState; label: string; icon: typeof Clock; tone: string }
  items: TaskRow[]
  draggingID: string | null
  onDragStart: (id: string) => void
  onDragEnd: () => void
  onDrop: (state: TaskState) => void
  onSelect: (id: string) => void
  selectedID: string | null
}) {
  const Icon = col.icon
  const dropOK =
    draggingID && (MANUAL_MOVE_OK[
      items.find((t) => t.id === draggingID)?.state ?? ""
    ] ?? []).includes(col.state)
  return (
    <div
      onDragOver={(e) => {
        e.preventDefault()
      }}
      onDrop={() => onDrop(col.state)}
      className={
        "w-72 shrink-0 rounded-lg border border-border bg-card/30 p-2 " +
        (dropOK ? "ring-1 ring-primary/40" : "")
      }
    >
      <div className="flex items-center gap-2 px-2 py-2 sticky top-0 bg-card/60 backdrop-blur-sm rounded-md mb-1">
        <Icon
          className={
            "h-4 w-4 " +
            col.tone +
            (col.state === "running" ? " animate-spin" : "")
          }
        />
        <div className="text-sm font-medium tracking-tight">{col.label}</div>
        <div className="ml-auto text-xs text-muted-foreground">
          {items.length}
        </div>
      </div>
      <div className="space-y-2 max-h-[72vh] overflow-y-auto pr-1">
        {items.length === 0 && (
          <div className="text-xs italic text-muted-foreground px-2 py-3">
            empty
          </div>
        )}
        {items.map((t) => (
          <TaskCard
            key={t.id}
            task={t}
            selected={selectedID === t.id}
            onSelect={() => onSelect(t.id)}
            onDragStart={() => onDragStart(t.id)}
            onDragEnd={onDragEnd}
          />
        ))}
      </div>
    </div>
  )
}

function TaskCard({
  task,
  selected,
  onSelect,
  onDragStart,
  onDragEnd,
}: {
  task: TaskRow
  selected: boolean
  onSelect: () => void
  onDragStart: () => void
  onDragEnd: () => void
}) {
  return (
    <button
      draggable
      onDragStart={onDragStart}
      onDragEnd={onDragEnd}
      onClick={onSelect}
      className={
        "w-full text-left rounded-md border border-border bg-background/60 p-2 " +
        "hover:border-primary/40 transition-colors block " +
        (selected ? "ring-1 ring-primary border-primary/60" : "")
      }
    >
      <div className="text-sm font-medium line-clamp-2">
        {task.title || task.id}
      </div>
      {task.last_progress_text && (
        <div className="text-xs text-muted-foreground mt-1 line-clamp-2">
          {task.last_progress_text}
        </div>
      )}
      <div className="flex items-center gap-1 mt-2 flex-wrap">
        {task.owner_synth_id && (
          <Badge variant="outline" className="text-[10px] py-0 px-1.5 gap-1">
            <Bot className="h-2.5 w-2.5" />
            {ownerLabel(task.owner_synth_id)}
          </Badge>
        )}
        {task.sub_agent_kind && (
          <Badge variant="secondary" className="text-[10px] py-0 px-1.5">
            {task.sub_agent_kind}
          </Badge>
        )}
        {task.cost_usd > 0 && (
          <Badge variant="outline" className="text-[10px] py-0 px-1.5">
            {fmtCost(task.cost_usd)}
          </Badge>
        )}
        <span className="ml-auto text-[10px] text-muted-foreground">
          {relAge(task.updated_at)} ago
        </span>
      </div>
    </button>
  )
}

function DetailPanel({
  taskID,
  onClose,
  onChanged,
}: {
  taskID: string
  onClose: () => void
  onChanged: () => void
}) {
  const [task, setTask] = useState<TaskRow | null>(null)
  const [events, setEvents] = useState<TaskEventRow[]>([])
  const [reason, setReason] = useState("")
  const [answer, setAnswer] = useState("")
  const [busy, setBusy] = useState(false)
  const wrapRef = useRef<HTMLDivElement | null>(null)

  const load = async () => {
    try {
      const t = await fetchTask(taskID)
      setTask(t)
      const e = await fetchTaskEvents(taskID, 100)
      setEvents(e.events ?? [])
    } catch (e) {
      toast.error(String((e as Error).message ?? e))
    }
  }

  useEffect(() => {
    void load()
    const h = setInterval(() => void load(), 4000)
    return () => clearInterval(h)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [taskID])

  const doCancel = async () => {
    if (!task) return
    if (!confirm(`Cancel "${task.title || task.id}"?`)) return
    setBusy(true)
    try {
      await cancelTask(task.id, reason || "cancelled from companion UI")
      toast.success("cancel requested")
      onChanged()
      await load()
    } catch (e) {
      toast.error(String((e as Error).message ?? e))
    } finally {
      setBusy(false)
    }
  }

  const doProvide = async () => {
    if (!task) return
    if (!answer.trim()) return
    setBusy(true)
    try {
      await provideTaskInput(task.id, answer.trim(), "user")
      toast.success("answer sent")
      setAnswer("")
      onChanged()
      await load()
    } catch (e) {
      toast.error(String((e as Error).message ?? e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div
      className="fixed inset-0 z-40 bg-black/30 backdrop-blur-sm"
      onClick={(e) => {
        if (e.target === wrapRef.current) onClose()
      }}
    >
      <div
        ref={wrapRef}
        onClick={onClose}
        className="absolute inset-0"
      />
      <div
        className="absolute right-0 top-0 h-full w-full max-w-xl bg-card border-l border-border shadow-2xl overflow-y-auto"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center gap-2 px-4 py-3 border-b border-border sticky top-0 bg-card z-10">
          <ChevronRight
            className="h-4 w-4 cursor-pointer text-muted-foreground hover:text-foreground"
            onClick={onClose}
          />
          <div className="text-sm font-mono text-muted-foreground truncate">
            {taskID}
          </div>
        </div>

        {!task && (
          <div className="p-6 text-sm text-muted-foreground italic">
            loading…
          </div>
        )}

        {task && (
          <div className="p-4 space-y-4">
            <div>
              <h2 className="text-lg font-semibold">{task.title || task.id}</h2>
              <div className="flex flex-wrap items-center gap-2 mt-2">
                <Badge>{task.state}</Badge>
                {task.owner_synth_id && (
                  <Badge variant="outline" className="gap-1">
                    <Bot className="h-3 w-3" /> {ownerLabel(task.owner_synth_id)}
                  </Badge>
                )}
                {task.sub_agent_kind && (
                  <Badge variant="secondary">{task.sub_agent_kind}</Badge>
                )}
                {task.priority !== 0 && (
                  <Badge variant="outline">priority {task.priority}</Badge>
                )}
              </div>
            </div>

            {task.description && (
              <div>
                <Label className="text-xs uppercase tracking-wider text-muted-foreground">
                  Description
                </Label>
                <div className="text-sm whitespace-pre-wrap mt-1 p-3 bg-muted/30 rounded-md">
                  {task.description}
                </div>
              </div>
            )}

            <div className="grid grid-cols-2 gap-3 text-xs">
              <Field label="Created" value={task.created_at} />
              <Field label="Updated" value={task.updated_at} />
              {task.claimed_at && <Field label="Claimed" value={task.claimed_at} />}
              {task.started_at && <Field label="Started" value={task.started_at} />}
              {task.finished_at && <Field label="Finished" value={task.finished_at} />}
              <Field label="Created by" value={task.created_by} />
              {task.cost_usd > 0 && (
                <Field label="Cost" value={fmtCost(task.cost_usd)} />
              )}
              {task.total_tokens > 0 && (
                <Field label="Tokens" value={task.total_tokens.toLocaleString()} />
              )}
              {task.retry_attempt > 0 && (
                <Field label="Retry" value={String(task.retry_attempt)} />
              )}
              {task.workspace_path && (
                <Field label="Workspace" value={task.workspace_path} mono />
              )}
              {task.transcript_path && (
                <Field label="Transcript" value={task.transcript_path} mono />
              )}
            </div>

            {task.last_progress_text && (
              <div>
                <Label className="text-xs uppercase tracking-wider text-muted-foreground">
                  Last progress
                </Label>
                <div className="text-sm mt-1 p-3 bg-muted/30 rounded-md whitespace-pre-wrap">
                  {task.last_progress_text}
                </div>
              </div>
            )}

            {task.error_text && (
              <Alert variant="destructive">
                <AlertTitle>Error</AlertTitle>
                <AlertDescription className="font-mono text-xs whitespace-pre-wrap">
                  {task.error_text}
                </AlertDescription>
              </Alert>
            )}

            {task.cancellation_reason && (
              <div>
                <Label className="text-xs uppercase tracking-wider text-muted-foreground">
                  Cancellation reason
                </Label>
                <div className="text-sm mt-1 p-3 bg-muted/30 rounded-md">
                  {task.cancellation_reason}
                </div>
              </div>
            )}

            {task.state === "awaiting_input" && (
              <Card>
                <CardHeader className="pb-2">
                  <CardTitle className="text-sm flex items-center gap-2">
                    <HelpCircle className="h-4 w-4 text-purple-400" />
                    Sub-agent is asking
                  </CardTitle>
                </CardHeader>
                <CardContent className="space-y-2">
                  <Textarea
                    placeholder="your answer (sent to sub-agent)…"
                    value={answer}
                    onChange={(e) => setAnswer(e.target.value)}
                    className="min-h-20 text-sm"
                  />
                  <Button
                    size="sm"
                    onClick={doProvide}
                    disabled={busy || !answer.trim()}
                  >
                    <Send className="h-4 w-4 mr-1" /> Send answer
                  </Button>
                </CardContent>
              </Card>
            )}

            {task.state !== "done" &&
              task.state !== "cancelled" &&
              task.state !== "error" && (
                <Card>
                  <CardHeader className="pb-2">
                    <CardTitle className="text-sm">Cancel</CardTitle>
                  </CardHeader>
                  <CardContent className="space-y-2">
                    <Input
                      placeholder="reason (optional)"
                      value={reason}
                      onChange={(e) => setReason(e.target.value)}
                    />
                    <Button
                      size="sm"
                      variant="destructive"
                      onClick={doCancel}
                      disabled={busy}
                    >
                      <XCircle className="h-4 w-4 mr-1" /> Cancel task
                    </Button>
                  </CardContent>
                </Card>
              )}

            <div>
              <Label className="text-xs uppercase tracking-wider text-muted-foreground">
                Events ({events.length})
              </Label>
              <div className="mt-2 space-y-1 max-h-72 overflow-y-auto">
                {events.length === 0 && (
                  <div className="text-xs italic text-muted-foreground">
                    none yet
                  </div>
                )}
                {events.map((e) => (
                  <EventRow key={e.id} ev={e} />
                ))}
              </div>
            </div>
          </div>
        )}
      </div>
    </div>
  )
}

function Field({
  label,
  value,
  mono,
}: {
  label: string
  value: string
  mono?: boolean
}) {
  return (
    <div>
      <div className="text-[10px] uppercase tracking-wider text-muted-foreground">
        {label}
      </div>
      <div
        className={
          "text-xs " +
          (mono ? "font-mono break-all" : "") +
          " text-foreground/90"
        }
      >
        {value}
      </div>
    </div>
  )
}

function EventRow({ ev }: { ev: TaskEventRow }) {
  let payload: unknown
  try {
    payload = ev.payload ? JSON.parse(ev.payload) : null
  } catch {
    payload = ev.payload
  }
  return (
    <div className="text-xs border border-border/60 rounded p-2 bg-muted/10">
      <div className="flex items-center gap-2">
        <Badge variant="outline" className="text-[10px] py-0 px-1.5">
          {ev.kind}
        </Badge>
        <span className="text-muted-foreground/70 font-mono text-[10px]">
          {ev.actor}
        </span>
        <span className="ml-auto text-muted-foreground/60 text-[10px]">
          {ev.ts}
        </span>
      </div>
      {payload !== null && payload !== "" && (
        <pre className="mt-1 text-[10px] text-muted-foreground/80 whitespace-pre-wrap break-all max-h-40 overflow-y-auto">
          {typeof payload === "string"
            ? payload
            : JSON.stringify(payload, null, 2)}
        </pre>
      )}
    </div>
  )
}

function CreateDialog({
  open,
  templates,
  onClose,
  onCreated,
}: {
  open: boolean
  templates: TaskTemplate[]
  onClose: () => void
  onCreated: () => void
}) {
  const [form, setForm] = useState<{
    title: string
    description: string
    sub_agent_kind: string
    template_id: string
    state: "backlog" | "queued"
    priority: number
  }>({
    title: "",
    description: "",
    sub_agent_kind: "claude",
    template_id: "",
    state: "queued",
    priority: 0,
  })
  const [busy, setBusy] = useState(false)

  useEffect(() => {
    if (open) {
      setForm({
        title: "",
        description: "",
        sub_agent_kind: "claude",
        template_id: "",
        state: "queued",
        priority: 0,
      })
    }
  }, [open])

  const submit = async () => {
    if (!form.title.trim()) {
      toast.error("title is required")
      return
    }
    setBusy(true)
    try {
      const input: CreateTaskInput = {
        title: form.title.trim(),
        description: form.description,
        sub_agent_kind: form.sub_agent_kind,
        state: form.state,
        priority: form.priority,
      }
      if (form.template_id) input.template_id = form.template_id
      await createTask(input)
      toast.success("task created")
      onCreated()
    } catch (e) {
      toast.error(String((e as Error).message ?? e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <Dialog open={open} onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="max-w-lg">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <CloudIcon className="h-4 w-4" /> New task
          </DialogTitle>
        </DialogHeader>
        <div className="space-y-3">
          <div className="space-y-1">
            <Label>Title</Label>
            <Input
              autoFocus
              value={form.title}
              onChange={(e) => setForm((f) => ({ ...f, title: e.target.value }))}
              placeholder="Refactor auth middleware to support OAuth2"
            />
          </div>
          <div className="space-y-1">
            <Label>Description</Label>
            <Textarea
              value={form.description}
              onChange={(e) =>
                setForm((f) => ({ ...f, description: e.target.value }))
              }
              className="min-h-32"
              placeholder="What should the sub-agent do? Include constraints, files to touch, success criteria."
            />
          </div>
          <div className="grid grid-cols-3 gap-3">
            <div className="space-y-1">
              <Label>Sub-agent</Label>
              <select
                value={form.sub_agent_kind}
                onChange={(e) =>
                  setForm((f) => ({ ...f, sub_agent_kind: e.target.value }))
                }
                className="w-full text-sm bg-muted/40 border border-border rounded px-2 py-1.5"
              >
                {KIND_OPTIONS.map((o) => (
                  <option key={o.value} value={o.value}>
                    {o.label}
                  </option>
                ))}
              </select>
            </div>
            <div className="space-y-1">
              <Label>Initial state</Label>
              <select
                value={form.state}
                onChange={(e) =>
                  setForm((f) => ({
                    ...f,
                    state: e.target.value as "backlog" | "queued",
                  }))
                }
                className="w-full text-sm bg-muted/40 border border-border rounded px-2 py-1.5"
              >
                <option value="queued">queued (pick up now)</option>
                <option value="backlog">backlog (later)</option>
              </select>
            </div>
            <div className="space-y-1">
              <Label>Priority</Label>
              <Input
                type="number"
                value={form.priority}
                onChange={(e) =>
                  setForm((f) => ({
                    ...f,
                    priority: parseInt(e.target.value) || 0,
                  }))
                }
              />
            </div>
          </div>
          {templates.length > 0 && (
            <div className="space-y-1">
              <Label>Template (optional)</Label>
              <select
                value={form.template_id}
                onChange={(e) =>
                  setForm((f) => ({ ...f, template_id: e.target.value }))
                }
                className="w-full text-sm bg-muted/40 border border-border rounded px-2 py-1.5"
              >
                <option value="">— none —</option>
                {templates.map((t) => (
                  <option key={t.id} value={t.id}>
                    {t.name}
                    {t.description ? ` — ${t.description.slice(0, 60)}` : ""}
                  </option>
                ))}
              </select>
            </div>
          )}
        </div>
        <DialogFooter>
          <Button variant="ghost" onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button onClick={submit} disabled={busy}>
            {busy ? "creating…" : "Create"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

// Suppress unused warnings for export-only constants.
void TASK_STATES
