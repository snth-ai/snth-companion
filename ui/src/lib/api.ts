// api.ts — thin fetch wrappers around the companion's Go-side JSON
// endpoints. /api/status + /api/audit are direct; /api/hub/* proxies
// inject the companion bearer server-side so the browser never sees
// it. All calls go through the dev-proxy when running on Vite; in
// production the Go server ships this bundle on the same origin.

// --- types shared with Go --------------------------------------------

export type StatusResponse = {
  status: "connected" | "connecting" | "paused" | "disconnected"
  last_error: string
  last_seen: string
  paired: boolean
  synth_url: string
  synth_id: string
  sandbox_roots: string[]
  tools: Array<{ name: string; description: string; danger_level: string }>
  version: string
}

export type AuditEntry = {
  started_at: string
  tool: string
  duration_ms: number
  outcome: string
  args_summary: string
}

export type ToolEntry = {
  name: string
  description: string
  danger_level: string
  stat: {
    last?: AuditEntry
    calls: number
    errors: number
  }
}

export type ChannelSettings = {
  instance_id: string
  instagram_enabled: boolean
  instagram_read_only: boolean
  instagram_owner_map: Record<string, string>
  whatsapp_enabled: boolean
  whatsapp_read_only: boolean
  whatsapp_proxy: string
  updated_at: string
}

export type ProviderCatalogEntry = {
  provider: string
  display: string
  example_model: string
  docs_url: string
  hint: string
}

export type LLMConfig = {
  synth_id: string
  primary?: {
    provider: string
    model: string
    key_label: string
    is_user_uploaded: boolean
  }
}

export type CodexLoginState = {
  has_flow: boolean
  done: boolean
  auth_url: string
  err: string
  has_result: boolean
  upload_err: string
  uploaded_at: string | null
  account_id?: string
  started_at?: string
}

export type LogsResponse = {
  synth_id: string
  lines: number
  log: string
}

// --- low-level helpers -----------------------------------------------

async function jsonOrThrow<T>(r: Response): Promise<T> {
  const text = await r.text()
  if (!r.ok) {
    try {
      const parsed = JSON.parse(text)
      throw new Error(parsed.error ?? text)
    } catch {
      throw new Error(`HTTP ${r.status}: ${text.slice(0, 200)}`)
    }
  }
  return text ? (JSON.parse(text) as T) : (undefined as unknown as T)
}

export async function getJSON<T>(path: string): Promise<T> {
  return jsonOrThrow(await fetch(path))
}

export async function postJSON<T>(path: string, body: unknown): Promise<T> {
  return jsonOrThrow(
    await fetch(path, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    }),
  )
}

// --- direct companion endpoints -------------------------------------

export const fetchStatus = () => getJSON<StatusResponse>("/api/status")
export const fetchAudit = () =>
  getJSON<{ entries: AuditEntry[] }>("/api/audit").then((d) => d.entries ?? [])
export const fetchTools = () =>
  getJSON<{ tools: ToolEntry[] }>("/api/tools").then((d) => d.tools ?? [])

// --- pair ------------------------------------------------------------

export const claimPair = (code: string, hub_url: string) =>
  postJSON<{ ok: boolean; synth_id: string; synth_url: string }>(
    "/api/pair/claim",
    { code, hub_url },
  )

export const savePair = (
  synth_url: string,
  token: string,
  synth_id: string,
) => postJSON("/api/pair/save", { synth_url, token, synth_id })

export const unpair = () => postJSON("/api/unpair", {})

// --- multi-synth (Phase 1) ------------------------------------------

export type SynthPair = {
  id: string
  url: string
  token?: string
  hub_url?: string
  label?: string
  role: "primary" | "secondary" | "test"
  tags?: string[]
  created_at: string
  last_seen_at?: string
}

export type SynthsResponse = {
  synths: SynthPair[]
  active_synth_id: string
}

export const fetchSynths = () => getJSON<SynthsResponse>("/api/synths")

export const setActiveSynth = (id: string) =>
  postJSON<{ ok: boolean; active_synth_id: string }>("/api/synths/active", { id })

export const updateSynth = (
  id: string,
  patch: { label?: string; role?: string; tags?: string[] },
) => {
  const body: Record<string, unknown> = { id }
  if (patch.label !== undefined) body.label = patch.label
  if (patch.role !== undefined) body.role = patch.role
  if (patch.tags !== undefined) {
    body.tags = patch.tags
    body.has_tags = true
  }
  return postJSON("/api/synths/update", body)
}

export const removeSynth = (id: string) => postJSON("/api/unpair", { id })

export type CompanionConfig = {
  role: "synth-host" | "user-device" | "shared" | ""
  tags: string[]
}

export const fetchCompanionConfig = () =>
  getJSON<CompanionConfig>("/api/companion-config")

export const updateCompanionConfig = (patch: {
  role?: string
  tags?: string[]
}) => {
  const body: Record<string, unknown> = {}
  if (patch.role !== undefined) body.role = patch.role
  if (patch.tags !== undefined) {
    body.tags = patch.tags
    body.has_tags = true
  }
  return postJSON("/api/companion-config", body)
}

// --- sandbox ---------------------------------------------------------

export const fetchSandbox = () =>
  getJSON<{ roots: string[] }>("/api/sandbox").then((d) => d.roots)

export const addSandbox = (path: string) =>
  postJSON("/api/sandbox/add", { path })

export const removeSandbox = (path: string) =>
  postJSON("/api/sandbox/remove", { path })

// --- codex login -----------------------------------------------------

export const codexLoginState = () =>
  getJSON<CodexLoginState>("/api/codex-login/state")

export const codexLoginStart = () =>
  fetch("/api/codex-login/start", { method: "POST" }).then((r) => r.ok)

export const codexLoginUpload = () =>
  fetch("/api/codex-login/upload", { method: "POST" }).then((r) => r.ok)

export const codexLoginClear = () =>
  fetch("/api/codex-login/clear", { method: "POST" }).then((r) => r.ok)

// --- hub-proxied (pair-token auth server-side) ----------------------

export const fetchChannelSettings = () =>
  getJSON<ChannelSettings>("/api/hub/channel-settings")

export const saveChannelSettings = (cs: Partial<ChannelSettings>) =>
  postJSON<ChannelSettings | { ok: boolean; env_push_error?: string }>(
    "/api/hub/channel-settings",
    cs,
  )

export const fetchProviderCatalog = () =>
  getJSON<{ providers: ProviderCatalogEntry[] }>(
    "/api/hub/provider-catalog",
  ).then((d) => d.providers)

export const fetchLLMConfig = () => getJSON<LLMConfig>("/api/hub/llm-config")

export const uploadProviderKey = (
  provider: string,
  api_key: string,
  model: string,
) =>
  postJSON<{
    ok: boolean
    key_id: string
    provider: string
    model: string
    applied: boolean
  }>("/api/hub/provider-key", { provider, api_key, model })

export const fetchRemoteLogs = (lines = 200) =>
  getJSON<LogsResponse>(`/api/hub/logs-remote?lines=${lines}`)

// --- Privacy / trust center ----------------------------------------

export type ToolMode = "prompt" | "trusted" | "denied"

export type TrustToolDef = {
  id: string
  label: string
  danger: "safe" | "prompt" | "always-prompt"
  always_prompt?: boolean
  current_mode: ToolMode
  description: string
}

export type TrustState = {
  master: boolean
  master_expires?: string | null
  tools?: Record<string, ToolMode>
  allowed_write_roots?: string[]
  updated_at: string
}

export type TrustResponse = { state: TrustState; tools: TrustToolDef[] }

export type TrustAuditEntry = AuditEntry & {
  decision?: "approved" | "denied"
  source?: string
}

export const fetchTrust = () => getJSON<TrustResponse>("/api/trust")

export const setTrustMaster = (on: boolean, expires?: string) =>
  postJSON("/api/trust/master", { on, expires: expires ?? "" })

export const setTrustTool = (tool: string, mode: ToolMode) =>
  postJSON("/api/trust/tool", { tool, mode })

export const trustRevokeAll = () => postJSON("/api/trust/revoke-all", {})

export const trustPathAdd = (path: string) =>
  postJSON("/api/trust/path", { op: "add", path })

export const trustPathRemove = (path: string) =>
  postJSON("/api/trust/path", { op: "remove", path })

export const fetchTrustAudit = () =>
  getJSON<{ entries: TrustAuditEntry[] }>("/api/trust/audit").then(
    (d) => d.entries ?? [],
  )
