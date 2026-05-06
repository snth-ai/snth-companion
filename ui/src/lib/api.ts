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

// --- synth tools (per-instance config — proxied to hub /api/my/tools)

export type SynthToolEntry = {
  name: string
  description: string
  base_description: string
  parameters: unknown
  source: string // builtin | skill | remote
  scope: string // synth | companion
  disabled: boolean
  disabled_global: boolean
  active_variant: string
  updated_at: string
}

export type SynthToolsResponse = {
  synth_id: string
  tools: SynthToolEntry[]
}

export const fetchSynthTools = () =>
  getJSON<SynthToolsResponse>("/api/hub/synth-tools")

export const toggleSynthTool = (tool: string, disabled: boolean) =>
  postJSON<{ status: string; tool: string; disabled: boolean }>(
    "/api/hub/synth-tools/toggle",
    { tool, disabled },
  )

// --- mini-apps (Wave 9 — synth-authored apps proxied through the hub)

export type MiniAppManifest = {
  slug: string
  name: string
  icon?: string
  tint?: string
  description?: string
  author?: string
  created_at?: string
  updated_at?: string
}

export type MiniAppListEntry = {
  manifest: MiniAppManifest
  view_html?: string
  manifest_error?: string
}

export type MiniAppsResponse = {
  apps: MiniAppListEntry[]
  dir?: string
}

export const fetchMiniApps = () =>
  getJSON<MiniAppsResponse>("/api/hub/mini-apps")

export const miniAppRunURL = (slug: string) =>
  `/api/hub/mini-app/${encodeURIComponent(slug)}?t=${Date.now()}`

export type SynthFetchResponse<T = unknown> = {
  status: number
  ok: boolean
  body: T
}

export const synthFetch = <T = unknown>(
  path: string,
  method: string = "GET",
  body?: unknown,
) =>
  postJSON<SynthFetchResponse<T>>("/api/hub/synth-fetch", {
    path,
    method,
    body,
  })

export type MiniAppAskResponse = {
  text: string
  tokens?: number
  format?: string
  error?: string
}

export const miniAppAsk = (slug: string, prompt: string, format?: string) =>
  postJSON<MiniAppAskResponse>("/api/hub/mini-apps/ask", {
    slug,
    prompt,
    format: format ?? "text",
  })

// --- browse APIs (Wave 9 — companion browses bound synth's data)

export type WikiPageLite = {
  id: string
  title: string
  type: string
  namespace: string
  project_id?: string | null
  snippet?: string
  updated_at: string
  bytes: number
}

export type WikiPageDetail = {
  id: string
  title: string
  type: string
  namespace: string
  project_id?: string | null
  content: string
  created_at: string
  updated_at: string
  links_out?: Array<{ page_id: string; title?: string; relation?: string }>
  links_in?: Array<{ page_id: string; title?: string; relation?: string }>
}

const synthGet = <T = unknown>(path: string): Promise<T> =>
  synthFetch<T>(path, "GET").then((r) => {
    if (!r.ok) throw new Error(`synth HTTP ${r.status}`)
    return r.body
  })

export const fetchWikiList = (
  opts: {
    type?: string
    ns?: string
    project_id?: string
    limit?: number
  } = {},
): Promise<{ pages: WikiPageLite[] }> => {
  const qs = new URLSearchParams()
  if (opts.type) qs.set("type", opts.type)
  if (opts.ns) qs.set("ns", opts.ns)
  if (opts.project_id) qs.set("project_id", opts.project_id)
  qs.set("limit", String(opts.limit ?? 500))
  return synthGet(`/api/wiki/list?${qs}`)
}

export const fetchWikiPage = (id: string): Promise<WikiPageDetail> =>
  synthGet(`/api/wiki/get?id=${encodeURIComponent(id)}`)

export const deleteWikiPage = (id: string) =>
  synthFetch(`/api/wiki/delete?id=${encodeURIComponent(id)}`, "POST")

// --- v0.4.42: write + connections + projects --------------------------

export type WikiUpsertInput = {
  page_id: string
  title: string
  content: string
  type: string
  namespace: string
}

export const upsertWikiPage = (
  in_: WikiUpsertInput,
  projectID?: string,
): Promise<WikiPageDetail> => {
  const qs = projectID ? `?project_id=${encodeURIComponent(projectID)}` : ""
  return synthFetch<WikiPageDetail>(`/api/wiki/upsert${qs}`, "POST", in_).then((r) => {
    if (!r.ok) throw new Error(`synth HTTP ${r.status}`)
    return r.body
  })
}

export type WikiSimilar = {
  page_id: string
  title: string
  type: string
  namespace: string
  score: number
}

export const fetchWikiSimilar = (
  id: string,
  limit = 5,
): Promise<{ similar: WikiSimilar[] }> =>
  synthGet(
    `/api/wiki/similar?id=${encodeURIComponent(id)}&limit=${limit}`,
  )

export const linkWikiPages = (
  source: string,
  target: string,
  relation = "related",
) => {
  const qs = new URLSearchParams({ source, target, relation })
  return synthFetch(`/api/wiki/link?${qs}`, "POST")
}

export const unlinkWikiPages = (
  source: string,
  target: string,
  relation?: string,
) => {
  const qs = new URLSearchParams({ source, target })
  if (relation) qs.set("relation", relation)
  return synthFetch(`/api/wiki/unlink?${qs}`, "POST")
}

export const assignWikiProject = (id: string, projectID: string) => {
  const qs = new URLSearchParams({ id, project_id: projectID })
  return synthFetch(`/api/wiki/assign-project?${qs}`, "POST")
}

export type Project = {
  id: string
  slug: string
  name: string
  description: string
  status: string
  color: string
  created_at: string
  updated_at: string
  page_count: number
}

export const fetchProjects = (): Promise<{ projects: Project[] }> =>
  synthGet(`/api/projects/list`)

export const upsertProject = (
  p: Partial<Project> & { slug: string; name: string },
): Promise<Project> =>
  synthFetch<Project>(`/api/projects/upsert`, "POST", p).then((r) => {
    if (!r.ok) throw new Error(`synth HTTP ${r.status}`)
    return r.body
  })

export const deleteProject = (id: string) =>
  synthFetch(`/api/projects/delete?id=${encodeURIComponent(id)}`, "POST")

export type MemoryEntry = {
  id: string
  text: string
  category: string
  scope: string
  importance: number
  created_at: string
}

export type MemoryListResponse = {
  memories: MemoryEntry[]
  total: number
  filtered_total: number
  offset: number
  limit: number
  categories: Record<string, number>
}

export const fetchMemoryList = (
  opts: {
    scope?: string
    category?: string
    limit?: number
    offset?: number
  } = {},
): Promise<MemoryListResponse> => {
  const qs = new URLSearchParams()
  if (opts.scope) qs.set("scope", opts.scope)
  if (opts.category) qs.set("category", opts.category)
  qs.set("limit", String(opts.limit ?? 200))
  qs.set("offset", String(opts.offset ?? 0))
  return synthGet(`/api/memory/list?${qs}`)
}

export const deleteMemory = (id: string) =>
  synthFetch(`/api/memory/delete?id=${encodeURIComponent(id)}`, "POST")

export type DreamPage = {
  id: string
  title: string
  type: string
  namespace?: string
  updated_at?: string
  snippet?: string
}

// Dream-feed endpoint returns a flat `pages` array filtered by
// type/namespace query params. We fan out two queries — one for diary
// narratives, one for theme extracts — to populate the two columns.
export const fetchDreamDiaries = (): Promise<{ pages: DreamPage[] }> =>
  synthGet(`/api/dream/list?type=dream&namespace=dreams&limit=60`)

export const fetchDreamThemes = (): Promise<{ pages: DreamPage[] }> =>
  synthGet(`/api/dream/list?type=theme&namespace=themes&limit=100`)

export const fetchDream = (
  id: string,
): Promise<{ id: string; title: string; content: string; updated_at?: string }> =>
  synthGet(`/api/dream/get?id=${encodeURIComponent(id)}`)

export type MediaItem = {
  name: string
  path: string
  is_dir: boolean
  size: number
  mod_time: string
  mime?: string
}

export const fetchMediaList = (dir?: string): Promise<{ items: MediaItem[]; dir: string }> => {
  const qs = new URLSearchParams()
  if (dir) qs.set("dir", dir)
  return synthGet(`/api/media/list?${qs}`)
}

// Direct file URL — companion's Library page renders <img>/<video>
// pointing here. The hub /api/my/synth-fetch path is too JSON-shaped
// for binary streams; we serve raw via a dedicated proxy.
export const mediaFileURL = (path: string) =>
  `/api/hub/synth-fetch-raw?path=${encodeURIComponent("/api/media/stream?path=" + path)}`
