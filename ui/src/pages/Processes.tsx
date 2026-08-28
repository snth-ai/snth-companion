import { useCallback, useEffect, useRef, useState } from "react"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Textarea } from "@/components/ui/textarea"
import { Badge } from "@/components/ui/badge"
import { fetchSynths } from "@/lib/api"
import { processRequest, type ProcessDraft, type ProcessSnapshot, type WorkProcess } from "@/lib/processes"

const fieldClass = "w-full rounded-md border border-input bg-background px-3 py-2 text-sm"
const newDraft = (): ProcessDraft => ({ title: "", instruction: "", executor: "agent", lifecycle: "once", trigger: { kind: "manual" }, policy: { notify: "never", read_only: true, max_attempts: 3, max_steps: 100, max_run_seconds: 600 } })
const when = (value?: string) => value && !value.startsWith("0001-") ? new Date(value).toLocaleString() : "—"
type Editor = { id: string; revision: number; draft: ProcessDraft }

export function ProcessesPage() {
  const [snapshot, setSnapshot] = useState<ProcessSnapshot | null>(null)
  const [synthID, setSynthID] = useState("")
  const [selectedID, setSelectedID] = useState("")
  const [editor, setEditor] = useState<Editor | null>(null)
  const [error, setError] = useState("")
  const [busy, setBusy] = useState(false)
  const [fresh, setFresh] = useState(false)
  const [notice, setNotice] = useState("")
  const [review, setReview] = useState({ delivery_id: "", verdict: "accepted", receipt: "", evidence: "" })
  const generation = useRef({ value: 0 })
  const binding = useRef("")
  const load = useCallback(async () => {
    const request = ++generation.current.value
    setBusy(true); setFresh(false); setError("")
    try {
      const pair = await fetchSynths()
      if (!pair.active_synth_id) throw new Error("Pair a synth first.")
      if (request !== generation.current.value) return
      if (binding.current && binding.current !== pair.active_synth_id) {
        setSnapshot(null); setEditor(null); setSelectedID("")
        setReview({ delivery_id: "", verdict: "accepted", receipt: "", evidence: "" })
      }
      binding.current = pair.active_synth_id
      const data = await processRequest<ProcessSnapshot>(pair.active_synth_id)
      if (!data.authoritative) throw new Error("The synth did not return authoritative process state.")
      if (request !== generation.current.value) return
      setSynthID(pair.active_synth_id)
      setSnapshot({ ...data, processes: data.processes || [], runs: data.runs || [], deliveries: data.deliveries || [], events: data.events || [] })
      setFresh(true)
      setEditor(editor => editor && !editor.revision && data.processes?.some(p => p.id === editor.id) ? null : editor)
    } catch (e) {
      if (request === generation.current.value) setError(String(e instanceof Error ? e.message : e))
    } finally { if (request === generation.current.value) setBusy(false) }
  }, [])
  useEffect(() => {
    const lifetime = generation.current
    let active = true
    // Defer the initial request so a StrictMode setup/cleanup pass cannot
    // start a second fetch. Late responses are fenced by the same generation.
    void Promise.resolve().then(() => { if (active) return load() })
    return () => { active = false; ++lifetime.value }
  }, [load])

  const selected = snapshot?.processes.find(p => p.id === selectedID)
  const mutate = async (command: object, savedID?: string) => {
    if (!fresh || busy) return
    const request = generation.current.value
    setBusy(true); setError(""); setNotice("")
    try {
      await processRequest(synthID, command)
      if (request !== generation.current.value) return
      setEditor(null)
      setReview({ delivery_id: "", verdict: "accepted", receipt: "", evidence: "" })
      if (savedID) setSelectedID(savedID)
      setNotice("Saved on the synth. A wake queues work; it does not mean the work is finished.")
      await load()
    } catch (e) {
      if (request !== generation.current.value) return
      setError(`${e instanceof Error ? e.message : String(e)}. Refresh to check the actual state before trying again; no command was automatically replayed.`)
      setFresh(false)
    } finally { if (request === generation.current.value) setBusy(false) }
  }
  const control = (p: WorkProcess, action: string) => {
    if (action === "cancel" && !window.confirm(`Cancel “${p.title}”? Already accepted deliveries cannot be recalled.`)) return
    void mutate({ action, id: p.id, revision: p.revision, ...(action === "wake" ? { command_id: crypto.randomUUID() } : {}) })
  }
  const edit = (p: WorkProcess) => setEditor({ id: p.id, revision: p.revision, draft: { title: p.title, instruction: p.instruction || "", executor: p.executor, lifecycle: p.lifecycle, completion: p.completion, trigger: p.trigger, policy: p.policy, data: p.data } })
  const patch = (delta: Partial<ProcessDraft>) => setEditor(e => e ? { ...e, draft: { ...e.draft, ...delta } } : e)
  const runs = snapshot?.runs.filter(r => r.process.id === selectedID) || []
  const deliveries = snapshot?.deliveries.filter(d => d.process_id === selectedID) || []

  return <div className="space-y-6">
    <div className="flex flex-wrap items-start justify-between gap-3">
      <div><h1 className="text-2xl font-semibold">Processes</h1><p className="mt-2 text-sm text-muted-foreground">Heartbeat, reminders, personal todos and ongoing synth work. External Companion jobs stay in Tasks.</p></div>
      <div className="flex gap-2"><Button variant="outline" disabled={busy} onClick={() => { setNotice(""); void load() }}>Refresh</Button><Button disabled={busy || !fresh} onClick={() => setEditor({ id: crypto.randomUUID(), revision: 0, draft: newDraft() })}>New process</Button></div>
    </div>
    {error && <div role="alert" className="rounded-lg border border-destructive p-4 text-sm">{error}<p className="mt-2 text-muted-foreground">Updates are disabled until a successful refresh. An unavailable or disabled manager is not an empty task list.</p></div>}
    {notice && <p role="status" className="text-sm text-muted-foreground">{notice}</p>}
    {busy && <p role="status" className="text-sm text-muted-foreground">Waiting for the synth…</p>}
    {snapshot && <p className="text-xs text-muted-foreground">Source: {synthID} · {fresh ? "last successful refresh" : "stale snapshot — read only"}</p>}
    {editor ? <form className="space-y-4 rounded-lg border p-5" onSubmit={e => { e.preventDefault(); void mutate({ action: editor.revision ? "update" : "create", id: editor.id, revision: editor.revision, draft: editor.draft }, editor.id) }}>
      <h2 className="text-lg font-medium">{editor.revision ? "Edit process" : "New process"}</h2>
      <label className="block text-sm">Title<Input required maxLength={500} value={editor.draft.title} onChange={e => patch({ title: e.target.value })} /></label>
      {editor.id === "system:heartbeat" ? <p className="text-sm text-muted-foreground">Instructions stay in HEARTBEAT.md. This form changes only the schedule and policy.</p> : <label className="block text-sm">Instructions<Textarea required maxLength={16000} rows={5} value={editor.draft.instruction} onChange={e => patch({ instruction: e.target.value })} /></label>}
      <div className="grid gap-4 sm:grid-cols-2">
        <label className="text-sm">Executor<select disabled={editor.id.startsWith("system:")} className={fieldClass} value={editor.draft.executor} onChange={e => { const executor = e.target.value; patch({ executor, ...(executor === "user" ? { trigger: { kind: "manual" } } : {}), policy: { ...editor.draft.policy, notify: executor === "deliver" ? "always" : "never", notification_ttl_seconds: executor === "deliver" ? 0 : editor.draft.policy.notification_ttl_seconds } }) }}><option value="agent">Synth</option><option value="deliver">Literal reminder (no model)</option><option value="user">Personal todo (you do it)</option></select></label>
        <label className="text-sm">Lifecycle<select disabled={editor.id.startsWith("system:")} className={fieldClass} value={editor.draft.lifecycle} onChange={e => patch({ lifecycle: e.target.value })}><option value="once">Once</option><option value="until">Until the condition is met</option><option value="continuous">Continuous</option></select></label>
      </div>
      {editor.draft.lifecycle === "until" && <label className="block text-sm">Completion condition<Input required value={editor.draft.completion || ""} onChange={e => patch({ completion: e.target.value })} /></label>}
      <label className="block text-sm">Trigger<select className={fieldClass} disabled={editor.draft.executor === "user"} value={editor.draft.trigger.kind} onChange={e => patch({ trigger: { kind: e.target.value, ...(e.target.value === "interval" ? { every_seconds: 1800 } : {}), ...(e.target.value === "daily" ? { local_time: "09:00", timezone: Intl.DateTimeFormat().resolvedOptions().timeZone } : {}) } })}>{(editor.id === "system:heartbeat" ? ["interval", "daily"] : ["manual", "once", "interval", "daily", "event"]).map(k => <option key={k}>{k}</option>)}</select></label>
      {editor.draft.trigger.kind === "once" && <label className="block text-sm">Timestamp with timezone (RFC3339)<Input required placeholder="2026-09-01T09:00:00+08:00" value={editor.draft.trigger.at || ""} onChange={e => patch({ trigger: { kind: "once", at: e.target.value } })} /></label>}
      {editor.draft.trigger.kind === "interval" && <label className="block text-sm">Interval in seconds<Input required type="number" min={1} max={31622400} value={editor.draft.trigger.every_seconds || ""} onChange={e => patch({ trigger: { ...editor.draft.trigger, every_seconds: Number(e.target.value) } })} /></label>}
      {editor.draft.trigger.kind === "daily" && <div className="grid gap-4 sm:grid-cols-2"><label className="text-sm">Local time<Input required type="time" value={editor.draft.trigger.local_time || ""} onChange={e => patch({ trigger: { ...editor.draft.trigger, local_time: e.target.value } })} /></label><label className="text-sm">IANA timezone<Input required value={editor.draft.trigger.timezone || ""} onChange={e => patch({ trigger: { ...editor.draft.trigger, timezone: e.target.value } })} /></label></div>}
      {editor.draft.trigger.kind === "event" && <label className="block text-sm">Event key<Input required value={editor.draft.trigger.event || ""} onChange={e => patch({ trigger: { kind: "event", event: e.target.value } })} /></label>}
      <label className="block text-sm">Notifications<select className={fieldClass} disabled={editor.draft.executor === "deliver"} value={editor.draft.policy.notify} onChange={e => patch({ policy: { ...editor.draft.policy, notify: e.target.value } })}>{["never", "on_change", "always", "on_error"].map(k => <option key={k}>{k}</option>)}</select></label>
      <label className="flex items-center gap-2 text-sm"><input type="checkbox" checked={editor.draft.policy.read_only} onChange={e => patch({ policy: { ...editor.draft.policy, read_only: e.target.checked } })} />Read-only work (no external changes)</label>
      {editor.draft.executor !== "deliver" && <label className="block text-sm">Notification freshness in seconds (0 = keep until delivered)<Input type="number" min={0} max={604800} value={editor.draft.policy.notification_ttl_seconds || 0} onChange={e => patch({ policy: { ...editor.draft.policy, notification_ttl_seconds: Number(e.target.value) } })} /></label>}
      <details className="space-y-3"><summary className="text-sm">Tool grants and execution budget</summary>
        <label className="block text-sm">Allowed tools (comma separated)<Input value={(editor.draft.policy.allowed_tools || []).join(", ")} onChange={e => patch({ policy: { ...editor.draft.policy, allowed_tools: e.target.value.split(",").map(v => v.trim()).filter(Boolean) } })} /></label>
        <p className="text-xs text-muted-foreground">Mutating and unclassified tools require an explicit grant. Opaque tools such as exec can perform external actions; grant only what this process needs. Direct messaging remains runtime-owned.</p>
        {(["max_attempts", "max_steps", "max_run_seconds"] as const).map(key => <label key={key} className="block text-sm">{key}<Input type="number" min={1} max={key === "max_attempts" ? 100 : key === "max_steps" ? 1000 : 86400} value={editor.draft.policy[key]} onChange={e => patch({ policy: { ...editor.draft.policy, [key]: Number(e.target.value) } })} /></label>)}
      </details>
      <p className="text-xs text-muted-foreground">Manual processes wait for Run now. Scheduled processes become eligible at their trigger. Draft ID: {editor.id}. A lost response never causes an automatic retry or a new ID.</p>
      <div className="flex gap-2"><Button disabled={busy || !fresh} type="submit">Save on synth</Button><Button type="button" variant="outline" disabled={busy} onClick={() => setEditor(null)}>Discard</Button></div>
    </form> : <div className="grid gap-5 lg:grid-cols-[minmax(220px,1fr)_2fr]">
      <div className="space-y-2" aria-label="Process list">{snapshot?.processes.map(p => <button key={p.id} onClick={() => setSelectedID(p.id)} className={`w-full rounded-lg border p-4 text-left ${p.id === selectedID ? "border-primary bg-muted" : "hover:bg-muted/50"}`}><span className="block break-words font-medium">{p.title}</span><span className="mt-2 flex flex-wrap gap-2 text-xs text-muted-foreground"><Badge variant="outline">{p.status}</Badge>{p.executor} · {p.trigger.kind}</span></button>)}{snapshot && snapshot.processes.length === 0 && <p className="p-4 text-sm text-muted-foreground">No processes in this owner scope yet.</p>}</div>
      <section className="min-w-0 rounded-lg border p-5 space-y-4" aria-label="Process details">
        {!selected ? <p className="text-sm text-muted-foreground">Choose a process to see its instructions, waits, results and deliveries.</p> : <>
          <h2 className="text-xl font-medium break-words">{selected.title}</h2>
          <p className="text-xs text-muted-foreground">Revision {selected.revision} · Next: {when(selected.next_at)} · Source: {selected.source}</p>
          <p className="whitespace-pre-wrap break-words text-sm">{selected.instruction_ref ? `Instructions live in ${selected.instruction_ref}. Edit the canonical file; run snapshots are history, not a second configuration.` : selected.instruction}</p>
          {selected.completion && <p className="text-sm">Done when: {selected.completion}</p>}
          <div className="flex flex-wrap gap-2">
            {selected.status === "active" && <Button variant="outline" disabled={busy || !fresh} onClick={() => control(selected, "pause")}>Pause</Button>}
            {selected.status === "paused" && <Button variant="outline" disabled={busy || !fresh} onClick={() => control(selected, "resume")}>Resume</Button>}
            {selected.status === "active" && selected.executor !== "user" && <Button variant="outline" disabled={busy || !fresh} onClick={() => control(selected, "wake")}>Run now</Button>}
            {selected.executor === "user" && ["active", "paused", "done"].includes(selected.status) && <Button variant="outline" disabled={busy || !fresh} onClick={() => control(selected, selected.status === "done" ? "reopen" : "complete")}>{selected.status === "done" ? "Reopen todo" : "Mark todo done"}</Button>}
            {["active", "paused"].includes(selected.status) && <><Button variant="outline" disabled={busy || !fresh} onClick={() => edit(selected)}>Edit</Button><Button variant="outline" disabled={busy || !fresh} onClick={() => control(selected, "cancel")}>Cancel</Button></>}
          </div>
          <h3 className="font-medium">Recent runs</h3>
          {!runs.length && <p className="text-sm text-muted-foreground">No runs. A personal todo never runs automatically.</p>}
          {runs.map(run => <article key={run.id} className="space-y-2 rounded border p-3 text-sm"><Badge variant="outline">{run.state}</Badge><p className="text-xs text-muted-foreground">Next eligible: {when(run.available_at)} · {run.attempts} attempt(s)</p>{run.reason && run.reason !== run.result?.summary && <p className="break-words">{run.reason}</p>}{run.result && <><p className="whitespace-pre-wrap">{run.result.summary}</p>{run.result.evidence && <p className="break-words">Reported evidence: {run.result.evidence}</p>}{run.result.wait && <p>Waiting: {run.result.wait.reason} {run.result.wait.event && `· event ${run.result.wait.event}`} · {when(run.result.wait.until)}</p>}{run.result.progress != null && <details><summary>Checkpoint</summary><pre className="overflow-auto whitespace-pre-wrap break-words">{JSON.stringify(run.result.progress, null, 2)}</pre></details>}</>}</article>)}
          {selected.status === "active" && runs.filter(run => run.state === "blocked" && !["delivery_unknown", "delivery_failed", "notification_expired"].includes(run.reason || "")).map(run => <form key={run.id} className="space-y-3 rounded border p-4" onSubmit={e => { e.preventDefault(); const evidence = new FormData(e.currentTarget).get("evidence"); void mutate({ action: "reconcile", id: selected.id, run_id: run.id, evidence }) }}>
            <h3 className="font-medium">Review interrupted work</h3>
            <p className="text-xs text-muted-foreground">Run {run.id}. Check previous side effects before resuming. Delivery uncertainty must be reviewed separately below.</p>
            <label className="block text-sm">What you checked<Textarea name="evidence" required maxLength={4000} /></label>
            <Button variant="outline" disabled={!fresh || busy} type="submit">Reconcile and continue</Button>
          </form>)}
          <h3 className="font-medium">Delivery receipts</h3><p className="text-xs text-muted-foreground">Sent means transport acceptance, not read. Unknown means acceptance is uncertain and is never automatically retried.</p>
          {!deliveries.length && <p className="text-sm text-muted-foreground">No delivery candidates.</p>}
          {deliveries.map(d => <article key={d.id} className="rounded border p-3 text-sm space-y-2"><Badge variant="outline">{d.state}</Badge><p className="whitespace-pre-wrap break-words">{d.message}</p>{d.reason && <p>{d.reason}</p>}{d.receipt && <p className="break-words text-xs text-muted-foreground">Receipt: {d.receipt}</p>}{["unknown", "failed"].includes(d.state) && <Button variant="outline" disabled={!fresh || busy} onClick={() => setReview({ delivery_id: d.id, verdict: "accepted", receipt: "", evidence: "" })}>Review delivery</Button>}</article>)}
          {review.delivery_id && deliveries.some(d => d.id === review.delivery_id) && <form className="space-y-3 rounded border p-4" onSubmit={e => { e.preventDefault(); void mutate({ action: "reconcile_delivery", ...review }) }}>
            <h3 className="font-medium">Transport review</h3><p className="text-xs text-muted-foreground">Check the provider or destination first. This is a recorded owner decision, not an automatic retry. Only proven non-acceptance may requeue this message.</p>
            <label className="block text-sm">Verified outcome<select className={fieldClass} value={review.verdict} onChange={e => setReview({ ...review, verdict: e.target.value })}><option value="accepted">Provider accepted it</option><option value="not_accepted">Proven not accepted — requeue delivery</option><option value="discard">Stop trying to deliver</option></select></label>
            {review.verdict === "accepted" && <label className="block text-sm">Provider receipt<Input required maxLength={2000} value={review.receipt} onChange={e => setReview({ ...review, receipt: e.target.value })} /></label>}
            <label className="block text-sm">What you checked<Textarea required maxLength={4000} value={review.evidence} onChange={e => setReview({ ...review, evidence: e.target.value })} /></label>
            <Button disabled={!fresh || busy} type="submit">Record verified outcome</Button>
          </form>}
          <details><summary className="text-sm font-medium">Audit history (loaded page)</summary><div className="mt-3 space-y-3">{snapshot?.events.filter(e => e.process_id === selectedID).map(e => <article key={e.sequence} className="text-xs"><p>{when(e.created_at)} · {e.kind} · {e.actor}</p><p className="whitespace-pre-wrap break-words text-muted-foreground">{e.detail}</p></article>)}</div></details>
        </>}
      </section>
    </div>}
  </div>
}
