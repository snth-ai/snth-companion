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

// --- MCP servers (v0.5.55+, proxied to hub /api/my/mcp/servers) -----

export type MCPServerView = {
  id: number
  name: string
  transport: "stdio" | "http" | "sse" | "http_oauth"
  scope: "instance" | "global"
  command?: string
  args?: string[]
  env?: Record<string, string>
  url?: string
  has_static_token?: boolean
  has_oauth_token?: boolean
  oauth_auth_url?: string
  oauth_token_url?: string
  oauth_client_id?: string
  oauth_scopes?: string
  oauth_expires_at?: string
  enabled: boolean
  last_status?: string
  last_status_at?: string
  editable: boolean
  created_at: string
  updated_at: string
}

export type MCPServerPayload = {
  name?: string
  transport?: "stdio" | "http" | "sse" | "http_oauth"
  command?: string
  args?: string[]
  env?: Record<string, string>
  url?: string
  static_token?: string
  oauth_auth_url?: string
  oauth_token_url?: string
  oauth_client_id?: string
  oauth_client_secret?: string
  oauth_scopes?: string
  enabled?: boolean
}

export const fetchMCPServers = async (): Promise<MCPServerView[]> => {
  const r = await getJSON<{ servers: MCPServerView[] }>("/api/hub/mcp/servers")
  return r.servers ?? []
}

export const createMCPServer = (body: MCPServerPayload) =>
  postJSON<MCPServerView>("/api/hub/mcp/servers", body)

export const updateMCPServer = async (id: number, body: MCPServerPayload) => {
  const r = await fetch(`/api/hub/mcp/servers/${id}`, {
    method: "PATCH",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  })
  return jsonOrThrow<MCPServerView>(r)
}

export const deleteMCPServer = async (id: number) => {
  const r = await fetch(`/api/hub/mcp/servers/${id}`, { method: "DELETE" })
  return jsonOrThrow<{ status: string }>(r)
}

export const toggleMCPServer = (id: number) =>
  postJSON<MCPServerView>(`/api/hub/mcp/servers/${id}/toggle`, {})

export const fetchMCPOAuthURL = (id: number) =>
  getJSON<{ url: string }>(`/api/hub/mcp/servers/${id}/oauth-url`)

// --- runtime skills (v0.5.55+, proxied to hub /api/my/skills) -------

export type SkillView = {
  name: string
  source: "baked" | "runtime"
  dir: string
  manifest_json: string
  script_name: string
  script_content: string
  skill_md: string
  has_manifest: boolean
  manifest_error?: string
}

export type SkillListResponse = {
  skills: SkillView[]
  runtime_dir: string
  baked_dir: string
}

export const fetchSkills = () =>
  getJSON<SkillListResponse>("/api/hub/skills")

export type SkillUpsertPayload = {
  name: string
  manifest_json: string
  script_name?: string
  script_content?: string
  skill_md?: string
}

export const upsertSkill = (body: SkillUpsertPayload) =>
  postJSON<{ status: string; name: string }>("/api/hub/skills/upsert", body)

export const deleteSkill = (name: string) =>
  postJSON<{ status: string }>("/api/hub/skills/delete", { name })

export const reloadSkills = () =>
  postJSON<{ status: string; count?: number }>("/api/hub/skills/reload", {})

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
  links_out?: Array<{ page_id: string; page_title?: string; relation?: string; strength?: number }>
  links_in?: Array<{ page_id: string; page_title?: string; relation?: string; strength?: number }>
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

export type SeedSimilarResponse = {
  ok: boolean
  scanned: number
  edges_added: number
  edges_total: number
  threshold: number
  top_k: number
}

export type WikiEdge = {
  id: number
  source_id: string
  target_id: string
  relation: string
  strength: number
  min_strength?: number
  context?: string | null
  created_at: string
}

// One round-trip for ALL edges. Replaces the per-page fan-out the Graph
// view used to do (~470 calls × ~250 ms = ~15 s on Mia). Synth-side
// endpoint added in v0.4.45.
export const fetchAllEdges = (): Promise<{ edges: WikiEdge[] }> =>
  synthGet(`/api/wiki/edges`)

// --- v2 knowledge graph (entities + relations) ------------------------
// The Graph tab's "Knowledge" mode renders memory_entities/memory_relations
// straight from the engine (synth-side /api/graph/* routes v2 when
// MEMORY_ENGINE_ENABLED). Node = entity, edge = typed relation. This is the
// real knowledge graph; the legacy "Pages" mode (wiki pages + their links)
// stays available via the mode toggle.
export type GraphV2Node = {
  id: string
  label: string
  type: string
  mention_count: number
  summary: string
  color: string
  size: number
}

export type GraphV2Edge = {
  id: string
  from: string
  to: string
  label: string
  relation_group: string
  strength: number
  active: boolean
  color: string
  width: number
  dashes: boolean
}

export type GraphV2Stats = {
  total_nodes: number
  total_edges: number
  active_edges: number
  invalidated_edges: number
  total_episodes: number
}

export type GraphV2Export = {
  nodes: GraphV2Node[] | null
  edges: GraphV2Edge[] | null
  stats: GraphV2Stats | null
}

// Full entity graph in one round-trip (active edges, entity-entity only).
export const fetchGraphExport = (): Promise<GraphV2Export> =>
  synthGet(`/api/graph/export`)

// One entity + its direct entity neighbours (center + 1-hop) for the
// detail panel. Returns the same {nodes, edges} shape as export.
export const fetchGraphNode = (
  id: string,
): Promise<{ nodes: GraphV2Node[] | null; edges: GraphV2Edge[] | null }> =>
  synthGet(`/api/graph/node/${encodeURIComponent(id)}`)

export type DedupePlanEntry = {
  canonical: string
  canonical_title: string
  dupes: string[]
  dupe_titles: string[]
}

export type DedupeResponse = {
  ns: string
  threshold: number
  dry_run: boolean
  scanned: number
  clusters: number
  plan: DedupePlanEntry[]
  // present on dry_run=true:
  would_delete?: number
  // present on dry_run=false:
  edges_merged?: number
  deleted?: number
  errors?: string[]
}

// Cluster a wiki namespace's pages by cosine similarity and either
// preview (dry_run=true) or commit the merge of duplicates into their
// canonical sibling. Synth-side endpoint: POST /api/wiki/dedupe-namespace.
export const dedupeNamespace = (
  ns: string,
  threshold: number,
  dryRun: boolean,
): Promise<DedupeResponse> => {
  const qs = new URLSearchParams({
    ns,
    threshold: String(threshold),
    dry_run: String(dryRun),
  })
  return synthFetch<DedupeResponse>(
    `/api/wiki/dedupe-namespace?${qs}`,
    "POST",
  ).then((r) => {
    if (!r.ok) throw new Error(`synth HTTP ${r.status}`)
    return r.body
  })
}

export const seedSimilarEdges = (
  threshold = 0.85,
  topK = 3,
): Promise<SeedSimilarResponse> => {
  const qs = new URLSearchParams({
    threshold: String(threshold),
    top_k: String(topK),
  })
  return synthFetch<SeedSimilarResponse>(
    `/api/wiki/seed-similar?${qs}`,
    "POST",
  ).then((r) => {
    if (!r.ok) throw new Error(`synth HTTP ${r.status}`)
    return r.body
  })
}

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

// --- Durable facts + journal (2026-06 memory redesign) ---

export type FactItem = {
  id: number
  claim_id?: string // v2 real claim id (for edit/forget); absent on v1
  scope?: string // v2 claim scope (so forget busts the right vector cache)
  text: string
  kind: string
  occurred_at: string
  project_id: string
  source: string
  confidence: number
  created_at: string
}
export type FactsListResponse = {
  facts: FactItem[]
  counts: Record<string, number>
  total: number
  enabled: boolean
}
export const fetchFacts = (
  opts: { kind?: string; q?: string; scope?: string; limit?: number; offset?: number } = {},
): Promise<FactsListResponse> => {
  const qs = new URLSearchParams()
  if (opts.kind) qs.set("kind", opts.kind)
  if (opts.q) qs.set("q", opts.q)
  if (opts.scope) qs.set("scope", opts.scope)
  qs.set("limit", String(opts.limit ?? 200))
  qs.set("offset", String(opts.offset ?? 0))
  return synthGet(`/api/facts/list?${qs}`)
}

// --- Memory Engine v2 overview (Wave 4.1) — dashboard payload ---
export type KV = { key: string; n: number }
export type MemConflict = { subject: string; predicate: string; count: number; claims: string[] }
export type MemTrace = {
  id?: string
  event: string
  target_type: string
  target_id: string
  query: string
  reason: string
  at: string
}
export type MemoryOverview = {
  enabled: boolean
  scope?: string
  counts?: Record<string, number>
  claims?: { live: number; superseded: number; invalidated: number }
  entities?: { live: number; archived: number }
  pages?: number
  journal?: number
  staging_pending?: number
  quarantine?: number
  kinds?: KV[]
  predicates?: KV[]
  conflicts?: MemConflict[]
  recent?: MemTrace[]
}
export const fetchMemoryOverview = (scope?: string): Promise<MemoryOverview> =>
  synthGet(`/api/memory/v2/overview${scope ? `?scope=${encodeURIComponent(scope)}` : ""}`)

// --- Memory v2 tail: forget/edit/why-recalled/agent-journal/quarantine ---
export const forgetFact = (claim_id: string, scope?: string, hard = false) =>
  synthFetch(`/api/memory/v2/forget`, "POST", { claim_id, scope, hard })

export const editFact = (claim_id: string, text: string) =>
  synthFetch(`/api/memory/v2/edit`, "POST", { claim_id, text })

export type WhyRecalled = {
  query: string
  reason: string
  items: { id: string; kind: string; text: string }[]
}
export const fetchWhyRecalled = (trace: string): Promise<WhyRecalled> =>
  synthGet(`/api/memory/v2/why-recalled?trace=${encodeURIComponent(trace)}`)

export type AgentJournalEntry = { title: string; body: string; at: string }
export const fetchAgentJournal = (): Promise<{ entries: AgentJournalEntry[] }> =>
  synthGet(`/api/memory/v2/agent-journal?limit=50`)

export type QuarantineEntry = { payload: string; reason: string; at: string }
export const fetchQuarantine = (): Promise<{ entries: QuarantineEntry[] }> =>
  synthGet(`/api/memory/v2/quarantine?limit=50`)

export type JournalItem = {
  id: number
  happened_on: string
  title: string
  body: string
  created_at: string
}
export type JournalListResponse = {
  journal: JournalItem[]
  total: number
  enabled: boolean
}
export const fetchJournal = (
  opts: { scope?: string; limit?: number; offset?: number } = {},
): Promise<JournalListResponse> => {
  const qs = new URLSearchParams()
  if (opts.scope) qs.set("scope", opts.scope)
  qs.set("limit", String(opts.limit ?? 60))
  qs.set("offset", String(opts.offset ?? 0))
  return synthGet(`/api/journal/list?${qs}`)
}

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

// --- traces (Diagnostics tab — v0.4.31+ synth-side, companion v? UI) ---

export type ToolCallTrace = {
  name: string
  duration_ms: number
  ok: boolean
  error?: string
}

export type TraceRow = {
  id: number
  trace_id: string
  kind: string
  session_id: string
  started_at: string
  ended_at: string
  duration_ms: number
  input_bytes: number
  output_bytes: number
  memory_recall_ms: number
  memory_recall_bytes: number
  memory_recall_count: number
  memory_loop_ms: number
  memory_loop_bytes: number
  memory_loop_entities: number
  wiki_ms: number
  wiki_bytes: number
  wiki_pages: number
  compact_ms: number
  llm_calls: number
  llm_prompt_tokens: number
  llm_cached_tokens: number
  llm_output_tokens: number
  llm_cost_usd: number
  outcome: string
  outcome_reason: string
  reply_preview: string
  error_text: string
  rss_bytes: number
  tool_calls: ToolCallTrace[]
  extra?: Record<string, unknown>
}

export type TracesListResponse = {
  from: string
  to: string
  count: number
  traces: TraceRow[]
  sessions: string[]
}

export type TraceRawResponse = {
  trace_id: string
  session_id: string
  path?: string
  line_count: number
  lines: string[]
}

export const fetchTraces = (
  opts: {
    from?: Date
    to?: Date
    kind?: string
    session?: string
    outcome?: string
    minDurationMs?: number
    hasError?: boolean
    limit?: number
  } = {},
): Promise<TracesListResponse> => {
  const qs = new URLSearchParams()
  if (opts.from) qs.set("from", opts.from.toISOString())
  if (opts.to) qs.set("to", opts.to.toISOString())
  if (opts.kind) qs.set("kind", opts.kind)
  if (opts.session) qs.set("session", opts.session)
  if (opts.outcome) qs.set("outcome", opts.outcome)
  if (opts.minDurationMs)
    qs.set("min_duration_ms", String(opts.minDurationMs))
  if (opts.hasError) qs.set("has_error", "true")
  qs.set("limit", String(opts.limit ?? 2000))
  return synthGet(`/api/traces?${qs}`)
}

export const fetchTraceRaw = (traceID: string): Promise<TraceRawResponse> =>
  synthGet(`/api/traces/${encodeURIComponent(traceID)}/raw`)

// --- Tasks system (v0.4.52, hub-side board) -------------------------

export type TaskRow = {
  id: string
  title: string
  description: string
  state: string
  priority: number
  template_id?: string | null
  template_overrides: string
  owner_synth_id?: string | null
  created_by: string
  created_at: string
  updated_at: string
  claimed_at?: string | null
  started_at?: string | null
  finished_at?: string | null
  assigned_companion_label: string
  workspace_path: string
  sub_agent_pid: number
  sub_agent_kind: string
  last_progress_at?: string | null
  last_progress_text: string
  retry_attempt: number
  retry_due_at?: string | null
  cost_usd: number
  total_tokens: number
  transcript_path: string
  error_text: string
  cancellation_reason: string
  wiki_page_id: string
}

export type TaskListResponse = { tasks: TaskRow[]; count: number }

export type TaskEventRow = {
  id: number
  task_id: string
  ts: string
  kind: string
  actor: string
  payload: string
}

export type TaskTemplate = {
  id: string
  name: string
  description: string
  prompt_template: string
  default_agent_config: string
  default_hooks: string
  suggested_keywords: string
  created_by_synth_id: string
  created_at: string
  updated_at: string
}

export type CreateTaskInput = {
  title: string
  description?: string
  priority?: number
  template_id?: string
  template_overrides?: Record<string, unknown>
  sub_agent_kind?: string
  state?: "backlog" | "queued"
  wiki_page_id?: string
}

export const TASK_STATES = [
  "backlog",
  "queued",
  "claimed",
  "running",
  "awaiting_input",
  "blocked",
  "done",
  "error",
  "cancelled",
] as const
export type TaskState = (typeof TASK_STATES)[number]

export const fetchTasksList = (
  filter: { state?: string; owner_synth_id?: string; limit?: number } = {},
): Promise<TaskListResponse> => {
  const qs = new URLSearchParams()
  if (filter.state) qs.set("state", filter.state)
  if (filter.owner_synth_id) qs.set("owner_synth_id", filter.owner_synth_id)
  qs.set("limit", String(filter.limit ?? 500))
  return getJSON(`/api/hub/tasks?${qs}`)
}

export const fetchTask = (id: string): Promise<TaskRow> =>
  getJSON(`/api/hub/tasks/${encodeURIComponent(id)}`)

export const createTask = (input: CreateTaskInput): Promise<TaskRow> =>
  postJSON(`/api/hub/tasks`, input)

export const patchTask = (
  id: string,
  patch: Record<string, unknown>,
): Promise<TaskRow> =>
  fetch(`/api/hub/tasks/${encodeURIComponent(id)}`, {
    method: "PATCH",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(patch),
  }).then((r) => jsonOrThrow<TaskRow>(r))

export const cancelTask = (id: string, reason: string): Promise<TaskRow> =>
  postJSON(`/api/hub/tasks/${encodeURIComponent(id)}/cancel`, { reason })

export const provideTaskInput = (
  id: string,
  answer: string,
  source: "user" | "synth" = "user",
): Promise<{ ok: boolean }> =>
  postJSON(`/api/hub/tasks/${encodeURIComponent(id)}/provide-input`, {
    answer,
    source,
  })

export const fetchTaskEvents = (
  id: string,
  limit = 200,
): Promise<{ events: TaskEventRow[]; count: number }> =>
  getJSON(`/api/hub/tasks/${encodeURIComponent(id)}/events?limit=${limit}`)

export type TranscriptResponse = {
  task_id: string
  transcript_path: string
  tail: string
  updated_at: string
}

export const fetchTaskTranscript = (
  id: string,
  lines = 100,
): Promise<TranscriptResponse> =>
  getJSON(`/api/hub/tasks/${encodeURIComponent(id)}/transcript?lines=${lines}`)

// --- Synth owner settings (per-session feature toggles, owner-scoped)

export type SynthOwnerSettings = {
  session_id: string
  timezone: string
  bio: string
  heartbeat: boolean
  emotional_state_enabled: boolean
  preferred_name: string
  updated_at: string
}

export const fetchSynthOwnerSettings = (): Promise<SynthOwnerSettings> =>
  synthGet(`/api/settings/owner`)

export const patchSynthOwnerSettings = (
  patch: Partial<{
    timezone: string
    bio: string
    heartbeat: boolean
    emotional_state_enabled: boolean
  }>,
): Promise<{ ok: boolean }> =>
  synthFetch<{ ok: boolean }>(`/api/settings/owner`, "PUT", patch).then((r) => {
    if (!r.ok) throw new Error(`synth HTTP ${r.status}`)
    return r.body
  })

export const fetchTaskTemplates = (): Promise<{
  templates: TaskTemplate[]
  count: number
}> => getJSON(`/api/hub/task-templates`)

export type TaskTemplateInput = {
  name: string
  description?: string
  prompt_template: string
  default_agent_config?: Record<string, unknown>
  default_hooks?: Record<string, unknown>
  suggested_keywords?: string[]
}

export const fetchTaskTemplate = (id: string): Promise<TaskTemplate> =>
  getJSON(`/api/hub/task-templates/${encodeURIComponent(id)}`)

export const createTaskTemplate = (
  in_: TaskTemplateInput,
): Promise<TaskTemplate> => postJSON(`/api/hub/task-templates`, in_)

export const patchTaskTemplate = (
  id: string,
  patch: Record<string, unknown>,
): Promise<TaskTemplate> =>
  fetch(`/api/hub/task-templates/${encodeURIComponent(id)}`, {
    method: "PATCH",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(patch),
  }).then((r) => jsonOrThrow<TaskTemplate>(r))

export const deleteTaskTemplate = (id: string): Promise<{ ok: boolean }> =>
  fetch(`/api/hub/task-templates/${encodeURIComponent(id)}`, {
    method: "DELETE",
  }).then((r) => jsonOrThrow<{ ok: boolean }>(r))

// --- Mia Public V1 (2026-05-11) ---------------------------------------
//
// Multi-user TG group/channel surface. Synth-side endpoints exposed via
// the existing /api/hub/synth-fetch pass-through. See atlas
// `20-mia-public-surface.md` for the design.

export type GroupConfig = {
  group_chat_id: string
  name: string
  kind: "group" | "channel" | "discussion"
  linked_channel_id?: string
  linked_discussion_id?: string
  mode: "strict" | "soft" | "trust"
  bot_privacy: "on" | "off"
  triggers: {
    mention: boolean
    reply: boolean
    scheduled: boolean
    topic: boolean
  }
  cooldown_min: number
  daily_max_sent: number
  trusted_users: string[]
  banned_topics: string[]
  tone_overlay: string
  private_memory_blocked: boolean
  enabled: boolean
  // PUB-1..5 per-channel overrides (2026-05-13). All optional; empty
  // string = inherit from the synth's instance default.
  model_override: string
  memory_addendum: string
  soul_full_override: string
  soul_addendum: string
  allowed_tools: string
  trigger_mode: "always" | "selective" | "mention_or_reply" | "mention_only" | ""
  created_at: string
  updated_at: string
}

export type PendingOutbound = {
  id: number
  group_chat_id: string
  trigger_kind: string
  draft_text: string
  draft_attachments?: string
  /**
   * Rich-media payload (post_to_channel v2). JSON-encoded; parsed by
   * PendingCard to render a kind-aware preview. Empty / undefined for
   * text-only drafts (legacy path). Schema:
   *   { kind: "photo"|"video"|"album"|..., path?, caption?, items?, ... }
   * See openpaw_server/mia_public.go AttachmentPayload for the full
   * field set.
   */
  attachment_json?: string
  reply_to_msg_id?: string
  message_thread_id?: number
  source_session_id?: string
  source_turn_trace?: string
  reason?: string
  status: "pending" | "approved" | "rejected" | "sent" | "expired"
  final_text?: string
  approver?: string
  approved_at?: string
  sent_at?: string
  rejection_reason?: string
  expires_at?: string
  created_at: string
}

/**
 * AttachmentPayload — decoded shape of PendingOutbound.attachment_json.
 * Mirrors openpaw_server/mia_public.go.
 */
export type AttachmentPayload = {
  kind:
    | "text"
    | "photo"
    | "video"
    | "video_note"
    | "voice"
    | "audio"
    | "document"
    | "sticker"
    | "animation"
    | "album"
    | "poll"
  path?: string
  caption?: string
  width?: number
  height?: number
  items?: Array<{
    kind: "photo" | "video"
    path: string
    caption?: string
    width?: number
    height?: number
  }>
  title?: string
  performer?: string
  sticker_id?: string
  gif_url?: string
  question?: string
  options?: string[]
  anonymous?: boolean
  multi?: boolean
  quiz?: boolean
  correct_option?: number
  explanation?: string
}

export const parseAttachment = (raw?: string): AttachmentPayload | null => {
  if (!raw) return null
  try {
    return JSON.parse(raw) as AttachmentPayload
  } catch {
    return null
  }
}

/**
 * synthFileURL builds the browser-usable URL for streaming a file
 * straight out of the bound synth's workspace. Used to <img>/<video>
 * preview queued media drafts in the Public-tab pending feed before
 * the operator approves. Goes through companion → hub → synth. The
 * synth's /api/file handler enforces path-traversal guards (must
 * resolve under ./workspace/).
 *
 * `absPath` is the path the synth recorded into post_to_channel's
 * attachment_json (always pre-resolved through the tool's sandbox at
 * queue-time).
 */
export const synthFileURL = (absPath: string): string => {
  // Two layers of URL-encoding: the outer path= carries the inner
  // /api/file?path=... query as a single literal value.
  const inner = `/api/file?path=${encodeURIComponent(absPath)}`
  return `/api/hub/synth-fetch-raw?path=${encodeURIComponent(inner)}`
}

export const fetchGroupConfigs = async (): Promise<GroupConfig[]> => {
  const r = await synthFetch<{ groups: GroupConfig[] }>(`/api/group-config`, "GET")
  if (!r.ok) throw new Error(`synth HTTP ${r.status}`)
  return r.body.groups ?? []
}

export const upsertGroupConfig = async (cfg: Partial<GroupConfig>): Promise<{ ok: boolean; group_chat_id: string }> => {
  const r = await synthFetch<{ ok: boolean; group_chat_id: string }>(`/api/group-config`, "POST", cfg)
  if (!r.ok) throw new Error(`synth HTTP ${r.status}`)
  return r.body
}

export const deleteGroupConfig = async (groupChatID: string): Promise<{ ok: boolean }> => {
  const r = await synthFetch<{ ok: boolean }>(
    `/api/group-config/delete?id=${encodeURIComponent(groupChatID)}`,
    "POST",
  )
  if (!r.ok) throw new Error(`synth HTTP ${r.status}`)
  return r.body
}

// --- Public-tab settings drawer helpers (PUBUI, 2026-05-16) ----------

// SynthModelOption — one selectable (provider, model) pair for the
// per-channel model_override dropdown. The value stored in group config
// is `${provider}:${model}`.
export type SynthModelOption = {
  provider: string
  model: string
  label: string // "OpenRouter — Claude Sonnet 4.6"
}

// fetchSynthModels pulls the synth's registered provider/model catalog
// via /api/llm/providers and flattens it to a flat dropdown list.
export const fetchSynthModels = async (): Promise<SynthModelOption[]> => {
  const r = await synthFetch<{
    providers: Array<{
      name: string
      display: string
      models: Array<{ id: string; display: string }>
    }>
  }>("/api/llm/providers", "GET")
  if (!r.ok) throw new Error(`synth HTTP ${r.status}`)
  const out: SynthModelOption[] = []
  for (const p of r.body.providers ?? []) {
    for (const m of p.models ?? []) {
      out.push({
        provider: p.name,
        model: m.id,
        label: `${p.display || p.name} — ${m.display || m.id}`,
      })
    }
  }
  return out
}

// fetchSynthTools pulls the synth's full tool catalog (builtin + skills
// + remote) so the per-channel allowed-tools toggle grid can render the
// real, current set rather than a free-text CSV.
export const fetchSynthToolNames = async (): Promise<
  Array<{ name: string; description: string; source: string }>
> => {
  const r = await synthFetch<{
    tools: Array<{ name: string; description: string; source: string }>
  }>("/api/tools", "GET")
  if (!r.ok) throw new Error(`synth HTTP ${r.status}`)
  return r.body.tools ?? []
}

// fetchSynthSoul returns the synth's current base SOUL.md content so the
// per-channel SOUL override textarea can be pre-filled for editing.
export const fetchSynthSoul = async (): Promise<string> => {
  const r = await synthFetch<{ soul_md: string }>("/api/config", "GET")
  if (!r.ok) throw new Error(`synth HTTP ${r.status}`)
  return r.body.soul_md ?? ""
}

// SynthConfigFiles — the full workspace-md file set the synth exposes
// via GET /api/config. Each key is the in-LLM identity / runtime input
// the synth reads on every turn from its workspace dir. Used by the
// companion Context tab editor (Wave D).
export type SynthConfigFiles = {
  soul_md: string
  rules_md: string
  agents_md: string
  heartbeat_md: string
  memory_md: string
}

// fetchSynthConfigFiles pulls all 5 workspace md files via synth
// /api/config GET. Empty strings for files that don't exist on disk.
export const fetchSynthConfigFiles = async (): Promise<SynthConfigFiles> => {
  const r = await synthFetch<Partial<SynthConfigFiles>>("/api/config", "GET")
  if (!r.ok) throw new Error(`synth HTTP ${r.status}`)
  return {
    soul_md: r.body.soul_md ?? "",
    rules_md: r.body.rules_md ?? "",
    agents_md: r.body.agents_md ?? "",
    heartbeat_md: r.body.heartbeat_md ?? "",
    memory_md: r.body.memory_md ?? "",
  }
}

// saveSynthConfigFile writes a single whitelisted file via synth
// /api/config POST. `file` is the BARE filename — "SOUL.md",
// "RULES.md", "AGENTS.md", "HEARTBEAT.md", "MEMORY.md" — not the
// snake_case key. Synth-side rejects anything outside that allow-list.
export const saveSynthConfigFile = async (
  file: string,
  content: string,
): Promise<void> => {
  const r = await synthFetch<{ ok: boolean }>("/api/config", "POST", {
    file,
    content,
  })
  if (!r.ok) throw new Error(`synth HTTP ${r.status}`)
}

export const fetchPendingOutbound = async (
  groupChatID?: string,
  limit = 50,
): Promise<PendingOutbound[]> => {
  const qs = new URLSearchParams()
  if (groupChatID) qs.set("group", groupChatID)
  qs.set("limit", String(limit))
  const r = await synthFetch<{ pending: PendingOutbound[]; count: number }>(
    `/api/outbound/pending?${qs}`,
    "GET",
  )
  if (!r.ok) throw new Error(`synth HTTP ${r.status}`)
  return r.body.pending ?? []
}

export const approveOutbound = async (
  id: number,
  approver: string,
  finalText?: string,
  force = false,
): Promise<{ ok: boolean; id: number }> => {
  // force=true → backend bypasses the per-channel cooldown + daily cap and
  // dispatches on the next tick (~8s). See openpaw mia_public.handleOutboundApprove (#106).
  const r = await synthFetch<{ ok: boolean; id: number }>(`/api/outbound/approve`, "POST", {
    id,
    approver,
    final_text: finalText ?? "",
    force,
  })
  if (!r.ok) throw new Error(`synth HTTP ${r.status}`)
  return r.body
}

export const rejectOutbound = async (
  id: number,
  approver: string,
  reason: string,
): Promise<{ ok: boolean; id: number }> => {
  const r = await synthFetch<{ ok: boolean; id: number }>(`/api/outbound/reject`, "POST", {
    id,
    approver,
    reason,
  })
  if (!r.ok) throw new Error(`synth HTTP ${r.status}`)
  return r.body
}

export const queueOutboundManual = async (
  groupChatID: string,
  text: string,
  opts?: { replyToMsgID?: string; approver?: string; sendNow?: boolean },
): Promise<{ ok: boolean; id: number; approved: boolean }> => {
  const r = await synthFetch<{ ok: boolean; id: number; approved: boolean }>(
    `/api/outbound/queue`,
    "POST",
    {
      group_chat_id: groupChatID,
      text,
      reply_to_msg_id: opts?.replyToMsgID ?? "",
      approver: opts?.approver ?? "operator",
      send_now: opts?.sendNow ?? false,
    },
  )
  if (!r.ok) throw new Error(`synth HTTP ${r.status}`)
  return r.body
}

export type BriefingMember = {
  tg_user_id: string
  name: string
  relationship?: string
  trusted?: boolean
  topics?: string[]
  notes?: string
  authority?: string
  group_chat_id?: string
}

export const importBriefing = async (
  members?: BriefingMember[],
  markdown?: string,
  defaultGroupChatID?: string,
): Promise<{ imported_count: number; failed_count: number; imported: unknown[]; failed: unknown[] }> => {
  const r = await synthFetch<{
    imported_count: number
    failed_count: number
    imported: unknown[]
    failed: unknown[]
  }>(`/api/briefing/import`, "POST", {
    members: members ?? [],
    markdown: markdown ?? "",
    default_group_chat_id: defaultGroupChatID ?? "",
  })
  if (!r.ok) throw new Error(`synth HTTP ${r.status}`)
  return r.body
}

// --- mobile companion: landmarks (hub-side, snth-mobile feature) -----
//
// Landmarks are user-scoped geofences (home, gym, work, custom) defined
// here on the Mac companion and consumed by the paired iPhone for
// CLCircularRegion monitoring. iOS never sends raw coordinates — only
// the landmark name on entry/exit. Hub stores per-user, pushes
// `landmarks.updated` over WS to mobile peer on every CRUD.
//
// Hub endpoint: /api/landmarks (GET + POST), /api/landmarks/<id> (PUT + DELETE)
// Hub-side impl status: codex's snth-hub commit 6 (TBD as of 2026-05-13)

export type LandmarkTag = "home" | "gym" | "work" | "store" | "custom"

export type Landmark = {
  id: number
  name: string
  tag: LandmarkTag
  lat: number
  lng: number
  radius_m: number
  created_at: number
  updated_at: number
}

export type LandmarkInput = {
  name: string
  tag: LandmarkTag
  lat: number
  lng: number
  radius_m: number
}

export const fetchLandmarks = (): Promise<Landmark[]> =>
  getJSON<{ landmarks: Landmark[] }>("/api/hub/landmarks").then(
    (r) => r.landmarks ?? [],
  )

export const createLandmark = (
  input: LandmarkInput,
): Promise<{ id: number }> => postJSON<{ id: number }>("/api/hub/landmarks", input)

export const updateLandmark = (
  id: number,
  input: LandmarkInput,
): Promise<{ ok: boolean }> =>
  fetch(`/api/hub/landmarks/${id}`, {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(input),
  }).then((r) => jsonOrThrow<{ ok: boolean }>(r))

export const deleteLandmark = (id: number): Promise<{ ok: boolean }> =>
  fetch(`/api/hub/landmarks/${id}`, { method: "DELETE" }).then((r) =>
    jsonOrThrow<{ ok: boolean }>(r),
  )

// ----------------------------------------------------------------------
// Context snapshot — CTXMAP. Per-turn breakdown of what fills the synth's
// LLM context, captured ring-buffer style in synth /api/context-snapshot
// and proxied through the hub. Mirror of synth-side soul.PromptSnapshot.

export type CtxPromptSectionSize = {
  key: string
  group: string
  bytes: number
}

export type CtxPromptSnapshot = {
  built_at: string
  mode: string
  total_bytes: number
  sections: CtxPromptSectionSize[]
}

export type CtxMessageHistorySize = {
  total_bytes: number
  user_bytes: number
  assistant_bytes: number
  tool_result_bytes: number
  system_bytes: number
  message_count: number
}

export type CtxSnapshot = {
  session_id: string
  built_at: string
  channel?: string
  provider?: string
  model?: string
  system_prompt: CtxPromptSnapshot
  message_history: CtxMessageHistorySize
  tool_schemas_bytes: number
  dynamic_context_bytes: number
  total_bytes: number
}

export const fetchContextSessions = async (): Promise<string[]> => {
  const r = await synthFetch<{ sessions?: string[] }>(
    "/api/context-snapshot",
    "GET",
  )
  if (!r.ok) throw new Error(`synth HTTP ${r.status}`)
  return r.body?.sessions ?? []
}

export const fetchContextSnapshots = async (
  session: string,
): Promise<CtxSnapshot[]> => {
  const r = await synthFetch<{ snapshots?: CtxSnapshot[] }>(
    `/api/context-snapshot?session=${encodeURIComponent(session)}`,
    "GET",
  )
  if (!r.ok) throw new Error(`synth HTTP ${r.status}`)
  return r.body?.snapshots ?? []
}
