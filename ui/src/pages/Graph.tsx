import { useEffect, useMemo, useRef, useState } from "react"
import ForceGraph2D from "react-force-graph-2d"
import { useNavigate } from "react-router-dom"
import ReactMarkdown from "react-markdown"
import remarkGfm from "remark-gfm"
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import { Button } from "@/components/ui/button"
import { Badge } from "@/components/ui/badge"
import { Sparkles, X, ExternalLink, Pencil } from "lucide-react"
import {
  fetchAllEdges,
  fetchProjects,
  fetchWikiList,
  fetchWikiPage,
  seedSimilarEdges,
  type Project,
  type WikiPageDetail,
  type WikiPageLite,
} from "@/lib/api"
import { toast } from "sonner"

// Graph — full-bleed force-directed network of wiki pages + edges.
// Layout strategy:
//   - Canvas fills the entire main area (escapes Layout's max-w-5xl
//     wrapper via `fixed inset-0 left-60`).
//   - Translucent glass-tile in the top-left corner holds the title,
//     project filter, seed-similar button, and stats (so the screen
//     reads as one unified visual).
//   - Click a node → animated slide-in panel from the right showing
//     the page detail (no router navigation; stays on /graph).

type GraphNode = {
  id: string
  name: string
  type: string
  project_id?: string | null
  color?: string
  val?: number
}

type GraphLink = {
  source: string
  target: string
  relation?: string
}

const TYPE_COLORS: Record<string, string> = {
  daily: "#94a3b8",
  dream: "#a855f7",
  theme: "#f97316",
  decision: "#22c55e",
  reflection: "#06b6d4",
  concept: "#3b82f6",
  project: "#facc15",
  module: "#84cc16",
  entity: "#ec4899",
  source: "#64748b",
  meta: "#cbd5e1",
  summary: "#10b981",
}

function nodeColor(p: WikiPageLite, projects: Project[]): string {
  if (p.project_id) {
    const proj = projects.find((pr) => pr.id === p.project_id)
    if (proj?.color) return proj.color
  }
  return TYPE_COLORS[p.type] ?? "#475569"
}

export function GraphPage() {
  const [projects, setProjects] = useState<Project[]>([])
  const [pages, setPages] = useState<WikiPageLite[]>([])
  const [edges, setEdges] = useState<GraphLink[]>([])
  const [err, setErr] = useState<string | null>(null)
  const [loadingEdges, setLoadingEdges] = useState(false)
  const [filterProj, setFilterProj] = useState<string>("__all__")
  const [seeding, setSeeding] = useState(false)
  const [selectedID, setSelectedID] = useState<string | null>(null)
  const [selectedDetail, setSelectedDetail] = useState<WikiPageDetail | null>(
    null,
  )
  const [detailLoading, setDetailLoading] = useState(false)
  const navigate = useNavigate()
  const containerRef = useRef<HTMLDivElement | null>(null)
  const [size, setSize] = useState({ w: 800, h: 600 })

  // Initial load.
  useEffect(() => {
    void (async () => {
      try {
        const [pj, pg] = await Promise.all([
          fetchProjects(),
          fetchWikiList({ limit: 1000 }),
        ])
        setProjects(pj.projects ?? [])
        setPages(pg.pages ?? [])
      } catch (e) {
        setErr(String((e as Error).message ?? e))
      }
    })()
  }, [])

  // Resize observer on the full-bleed container — canvas tracks it 1:1.
  useEffect(() => {
    if (!containerRef.current) return
    const el = containerRef.current
    const ro = new ResizeObserver(() => {
      setSize({ w: el.clientWidth, h: el.clientHeight })
    })
    ro.observe(el)
    return () => ro.disconnect()
  }, [])

  // Bulk edge fetch — one round-trip via /api/wiki/edges (v0.4.45+).
  useEffect(() => {
    if (pages.length === 0) return
    let cancelled = false
    setLoadingEdges(true)
    void (async () => {
      try {
        const r = await fetchAllEdges()
        if (cancelled) return
        const out: GraphLink[] = (r.edges ?? []).map((e) => ({
          source: e.source_id,
          target: e.target_id,
          relation: e.relation,
        }))
        setEdges(out)
      } catch (e) {
        setErr(String((e as Error).message ?? e))
      } finally {
        if (!cancelled) setLoadingEdges(false)
      }
    })()
    return () => {
      cancelled = true
    }
  }, [pages])

  // Lazy detail fetch when a node is clicked.
  useEffect(() => {
    if (!selectedID) {
      setSelectedDetail(null)
      return
    }
    let cancelled = false
    setDetailLoading(true)
    setSelectedDetail(null)
    void (async () => {
      try {
        const d = await fetchWikiPage(selectedID)
        if (!cancelled) setSelectedDetail(d)
      } catch (e) {
        if (!cancelled)
          toast.error(String((e as Error).message ?? e))
      } finally {
        if (!cancelled) setDetailLoading(false)
      }
    })()
    return () => {
      cancelled = true
    }
  }, [selectedID])

  // Esc closes the panel.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setSelectedID(null)
    }
    window.addEventListener("keydown", onKey)
    return () => window.removeEventListener("keydown", onKey)
  }, [])

  const { nodes, links } = useMemo(() => {
    const idOf = (v: unknown): string =>
      typeof v === "string" ? v : ((v as { id?: string })?.id ?? "")

    const filteredPages =
      filterProj === "__all__"
        ? pages
        : filterProj === "__none__"
        ? pages.filter((p) => !p.project_id)
        : pages.filter((p) => p.project_id === filterProj)
    const idSet = new Set(filteredPages.map((p) => p.id))
    const ns: GraphNode[] = filteredPages.map((p) => ({
      id: p.id,
      name: p.title || p.id,
      type: p.type,
      project_id: p.project_id ?? null,
      color: nodeColor(p, projects),
      val: 1,
    }))
    const inDeg = new Map<string, number>()
    for (const e of edges) {
      const tgt = idOf(e.target)
      if (idSet.has(tgt)) inDeg.set(tgt, (inDeg.get(tgt) ?? 0) + 1)
    }
    for (const n of ns) {
      n.val = 1 + (inDeg.get(n.id) ?? 0)
    }
    const ls: GraphLink[] = []
    for (const e of edges) {
      const src = idOf(e.source)
      const tgt = idOf(e.target)
      if (!idSet.has(src) || !idSet.has(tgt)) continue
      ls.push({ source: src, target: tgt, relation: e.relation })
    }
    return { nodes: ns, links: ls }
  }, [pages, projects, edges, filterProj])

  const projectOf = (pid?: string | null) =>
    pid ? projects.find((p) => p.id === pid) : undefined

  return (
    // fixed inset-0 left-60 = full-bleed of main area (sidebar is w-60 = 240px)
    <div className="fixed inset-0 left-60 bg-background overflow-hidden">
      {/* full-bleed canvas wrapper — ResizeObserver feeds canvas dimensions */}
      <div ref={containerRef} className="absolute inset-0">
        {nodes.length > 0 ? (
          <ForceGraph2D
            graphData={{ nodes, links }}
            width={size.w}
            height={size.h}
            backgroundColor="hsl(var(--background))"
            nodeColor={(n) => (n as GraphNode).color || "#475569"}
            nodeVal={(n) => Math.min(20, (n as GraphNode).val ?? 1) * 4}
            nodeLabel={(n) => (n as GraphNode).name}
            linkColor={() => "rgba(148, 163, 184, 0.25)"}
            linkWidth={1}
            cooldownTicks={120}
            onNodeClick={(n) => setSelectedID((n as GraphNode).id)}
            onBackgroundClick={() => setSelectedID(null)}
            nodeCanvasObjectMode={() => "after"}
            nodeCanvasObject={(n, ctx, scale) => {
              const node = n as GraphNode & { x?: number; y?: number }
              if (scale < 1.5 || !node.x || !node.y) return
              ctx.font = `${10 / scale}px sans-serif`
              ctx.fillStyle = "#cbd5e1"
              ctx.textAlign = "center"
              ctx.textBaseline = "top"
              ctx.fillText(node.name, node.x, node.y + 5)
            }}
          />
        ) : (
          <div className="absolute inset-0 grid place-items-center text-sm text-muted-foreground italic">
            no pages to visualize
          </div>
        )}
      </div>

      {/* Glass-tile overlay — title + filter + seed + stats */}
      <div className="absolute top-4 left-4 max-w-md pointer-events-auto">
        <div className="bg-card/70 backdrop-blur-md border border-border/50 rounded-lg shadow-2xl p-4 space-y-3">
          <div>
            <h1 className="text-xl font-semibold tracking-tight">Graph</h1>
            <p className="text-xs text-muted-foreground mt-1">
              Force-directed view of pages + their links. Click a node to
              preview it on the right. Bigger nodes = more inbound links.
            </p>
          </div>
          <div className="flex items-center gap-2 flex-wrap">
            <select
              value={filterProj}
              onChange={(e) => setFilterProj(e.target.value)}
              className="text-xs bg-background/80 border border-border rounded px-2 py-1.5"
            >
              <option value="__all__">all pages</option>
              <option value="__none__">unassigned</option>
              {projects.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.name}
                </option>
              ))}
            </select>
            <Button
              size="sm"
              variant="secondary"
              disabled={seeding}
              className="bg-background/80"
              onClick={async () => {
                if (
                  !confirm(
                    "Seed edges from vector similarity?\n\nFor every page, top-3 similar pages above 0.85 cosine become a `related` edge. Idempotent (re-running just nudges existing strengths).",
                  )
                )
                  return
                setSeeding(true)
                try {
                  const r = await seedSimilarEdges(0.85, 3)
                  toast.success(
                    `seeded ${r.edges_added} new edges (total ${r.edges_total} across ${r.scanned} pages)`,
                  )
                  const pg = await fetchWikiList({ limit: 1000 })
                  setPages([...(pg.pages ?? [])])
                } catch (e) {
                  toast.error(String((e as Error).message ?? e))
                } finally {
                  setSeeding(false)
                }
              }}
            >
              <Sparkles className="h-3.5 w-3.5 mr-1" />
              {seeding ? "seeding…" : "seed similar"}
            </Button>
          </div>
          <div className="text-[11px] text-muted-foreground tabular-nums">
            {nodes.length} nodes · {links.length} edges
            {loadingEdges && " · loading edges…"}
          </div>
        </div>

        {err && (
          <Alert variant="destructive" className="mt-3 backdrop-blur-md bg-destructive/80">
            <AlertTitle>Error</AlertTitle>
            <AlertDescription>{err}</AlertDescription>
          </Alert>
        )}
      </div>

      {/* Slide-in side panel — animated from the right */}
      <aside
        className={
          "absolute right-0 top-0 bottom-0 w-[440px] bg-card/95 backdrop-blur-md border-l border-border shadow-2xl transition-transform duration-300 ease-out " +
          (selectedID ? "translate-x-0" : "translate-x-full")
        }
        aria-hidden={!selectedID}
      >
        {selectedID && (
          <div className="h-full flex flex-col">
            <div className="flex items-center gap-2 px-4 py-3 border-b border-border">
              <button
                onClick={() => setSelectedID(null)}
                className="p-1 rounded hover:bg-muted text-muted-foreground hover:text-foreground"
                aria-label="close"
              >
                <X className="h-4 w-4" />
              </button>
              <span className="text-xs text-muted-foreground font-mono truncate flex-1">
                {selectedID}
              </span>
              <Button
                size="sm"
                variant="ghost"
                onClick={() =>
                  navigate(`/knowledge?id=${encodeURIComponent(selectedID)}`)
                }
                title="Open in Knowledge for editing"
              >
                <Pencil className="h-3.5 w-3.5 mr-1" /> edit
              </Button>
              <Button
                size="sm"
                variant="ghost"
                onClick={() =>
                  navigate(`/knowledge?id=${encodeURIComponent(selectedID)}`)
                }
                title="Open in Knowledge"
              >
                <ExternalLink className="h-3.5 w-3.5" />
              </Button>
            </div>

            <div className="flex-1 overflow-auto px-5 py-4 space-y-3">
              {detailLoading ? (
                <div className="text-sm italic text-muted-foreground">
                  loading…
                </div>
              ) : selectedDetail ? (
                <>
                  <div>
                    <div className="flex items-center gap-2 flex-wrap mb-1">
                      <Badge
                        variant="secondary"
                        className="text-[10px] uppercase tracking-wider"
                      >
                        {selectedDetail.type}
                      </Badge>
                      {selectedDetail.namespace !== "personal" && (
                        <Badge variant="outline" className="text-[10px]">
                          {selectedDetail.namespace}
                        </Badge>
                      )}
                      {projectOf(selectedDetail.project_id) && (
                        <Badge
                          className="text-[10px]"
                          style={{
                            backgroundColor:
                              projectOf(selectedDetail.project_id)?.color +
                              "30",
                            color: projectOf(selectedDetail.project_id)
                              ?.color,
                            borderColor: projectOf(selectedDetail.project_id)
                              ?.color,
                          }}
                        >
                          {projectOf(selectedDetail.project_id)?.name}
                        </Badge>
                      )}
                    </div>
                    <h2 className="text-lg font-semibold leading-tight">
                      {selectedDetail.title || selectedDetail.id}
                    </h2>
                  </div>

                  <article className="prose prose-invert prose-sm max-w-none">
                    <ReactMarkdown remarkPlugins={[remarkGfm]}>
                      {selectedDetail.content || "_(empty)_"}
                    </ReactMarkdown>
                  </article>

                  {(selectedDetail.links_out?.length ||
                    selectedDetail.links_in?.length) && (
                    <div className="border-t border-border pt-3 space-y-3">
                      {(selectedDetail.links_out?.length ?? 0) > 0 && (
                        <div>
                          <div className="text-[10px] uppercase tracking-wider text-muted-foreground mb-1">
                            links out ({selectedDetail.links_out!.length})
                          </div>
                          <div className="flex flex-wrap gap-1.5">
                            {selectedDetail.links_out!.map((l) => (
                              <button
                                key={l.page_id + (l.relation ?? "")}
                                onClick={() => setSelectedID(l.page_id)}
                                className="text-xs px-2 py-1 rounded bg-muted/50 hover:bg-muted text-foreground transition"
                                title={l.relation}
                              >
                                {l.page_title || l.page_id}
                              </button>
                            ))}
                          </div>
                        </div>
                      )}
                      {(selectedDetail.links_in?.length ?? 0) > 0 && (
                        <div>
                          <div className="text-[10px] uppercase tracking-wider text-muted-foreground mb-1">
                            referenced by ({selectedDetail.links_in!.length})
                          </div>
                          <div className="flex flex-wrap gap-1.5">
                            {selectedDetail.links_in!.map((l) => (
                              <button
                                key={l.page_id + (l.relation ?? "")}
                                onClick={() => setSelectedID(l.page_id)}
                                className="text-xs px-2 py-1 rounded bg-muted/50 hover:bg-muted text-foreground transition"
                                title={l.relation}
                              >
                                {l.page_title || l.page_id}
                              </button>
                            ))}
                          </div>
                        </div>
                      )}
                    </div>
                  )}
                </>
              ) : (
                <div className="text-sm italic text-muted-foreground">
                  no detail
                </div>
              )}
            </div>
          </div>
        )}
      </aside>
    </div>
  )
}
