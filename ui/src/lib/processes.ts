export type ProcessTrigger = { kind: string; at?: string; every_seconds?: number; local_time?: string; timezone?: string; event?: string }
export type ProcessPolicy = { notify: string; read_only: boolean; max_attempts: number; max_steps: number; max_run_seconds: number; notification_ttl_seconds?: number; allowed_tools?: string[] }
export type ProcessDraft = { title: string; instruction: string; executor: string; lifecycle: string; completion?: string; trigger: ProcessTrigger; policy: ProcessPolicy; data?: unknown }
export type WorkProcess = Omit<ProcessDraft, "instruction"> & { id: string; instruction?: string; instruction_ref?: string; status: string; revision: number; next_at?: string; source: string; scope: { synth_id: string; owner_id: string; id: string } }
export type ProcessRun = { id: string; process: WorkProcess; state: string; attempts: number; available_at: string; reason?: string; result?: { outcome: string; summary: string; evidence?: string; progress?: unknown; wait?: { reason: string; event?: string; until?: string } } }
export type ProcessDelivery = { id: string; process_id: string; run_id: string; state: string; reason?: string; receipt?: string; message: string }
export type ProcessEvent = { sequence: number; process_id: string; run_id: string; kind: string; actor: string; detail: string; created_at: string }
export type ProcessSnapshot = { authoritative: boolean; processes: WorkProcess[]; runs: ProcessRun[]; deliveries: ProcessDelivery[]; events: ProcessEvent[] }

// Expected synth is a precondition, never a routing selector. Never retry a
// mutation automatically: a lost response may already have committed.
export async function processRequest<T>(synthID: string, command?: unknown): Promise<T> {
  const response = await fetch("/api/hub/processes", {
    method: command ? "POST" : "GET",
    cache: "no-store",
    headers: { "Content-Type": "application/json", "X-Snth-Expected-Synth-Id": synthID },
    body: command ? JSON.stringify(command) : undefined,
  })
  const text = await response.text()
  if (!response.ok) {
    let message = text.slice(0, 300)
    try { message = JSON.parse(text).error || message } catch { /* plain upstream error */ }
    throw new Error(`HTTP ${response.status}: ${message}`)
  }
  return JSON.parse(text) as T
}
