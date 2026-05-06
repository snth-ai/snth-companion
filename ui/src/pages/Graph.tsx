import { useEffect, useMemo, useRef, useState } from "react"
import ForceGraph2D from "react-force-graph-2d"
import { useNavigate } from "react-router-dom"
import { Card, CardContent } from "@/components/ui/card"
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import { Button } from "@/components/ui/button"
import { Sparkles } from "lucide-react"
import {
  fetchAllEdges,
  fetchProjects,
  fetchWikiList,
  seedSimilarEdges,
  type Project,
  type WikiPageLite,
} from "@/lib/api"
import { toast } from "sonner"

// Graph — force-directed network of wiki pages + edges. Nodes coloured
// by project (or grey if unassigned). Click a node → jump to that page
// in /knowledge. Edges come from /api/wiki/edges in one round-trip
// (v0.4.45+). Pre-v0.4.45 versions did per-page fan-out and took ~15 s
// on Mia before any edge rendered — the bulk endpoint is ~150 ms.

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

  // Resize observer for the canvas container.
  useEffect(() => {
    if (!containerRef.current) return
    const el = containerRef.current
    const ro = new ResizeObserver(() => {
      setSize({ w: el.clientWidth, h: Math.max(500, window.innerHeight - 220) })
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

  const { nodes, links } = useMemo(() => {
    // react-force-graph-2d mutates link.source/target into node-object refs
    // after the first simulation tick. Normalize back to string IDs every
    // memo run, and emit FRESH link objects so the next mutation only damages
    // disposable copies — never our edges state.
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

  return (
    <div className="space-y-3">
      <div className="flex items-end justify-between gap-3 flex-wrap">
        <div>
          <h1 className="text-2xl font-semibold tracking-tight">Graph</h1>
          <p className="text-sm text-muted-foreground mt-1">
            Force-directed view of pages + their links. Click a node to open
            the page. Bigger nodes = more inbound links.
          </p>
        </div>
        <div className="flex items-center gap-2">
          <select
            value={filterProj}
            onChange={(e) => setFilterProj(e.target.value)}
            className="text-sm bg-card border border-border rounded px-2 py-1"
          >
            <option value="__all__">all pages</option>
            <option value="__none__">unassigned</option>
            {projects.map((p) => (
              <option key={p.id} value={p.id}>
                {p.name}
              </option>
            ))}
          </select>
          <span className="text-xs text-muted-foreground">
            {nodes.length} nodes · {links.length} edges
            {loadingEdges && " · loading edges…"}
          </span>
          <Button
            size="sm"
            variant="secondary"
            disabled={seeding}
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
                // Force a refetch of pages so the edges fan-out reruns.
                const pg = await fetchWikiList({ limit: 1000 })
                setPages([...(pg.pages ?? [])])
              } catch (e) {
                toast.error(String((e as Error).message ?? e))
              } finally {
                setSeeding(false)
              }
            }}
          >
            <Sparkles className="h-4 w-4 mr-1" />
            {seeding ? "seeding…" : "seed similar"}
          </Button>
        </div>
      </div>

      {err && (
        <Alert variant="destructive">
          <AlertTitle>Error</AlertTitle>
          <AlertDescription>{err}</AlertDescription>
        </Alert>
      )}

      <Card>
        <CardContent className="p-0" ref={containerRef as never}>
          {nodes.length === 0 ? (
            <div className="p-10 text-center text-sm text-muted-foreground italic">
              no pages to visualize
            </div>
          ) : (
            <ForceGraph2D
              graphData={{ nodes, links }}
              width={size.w}
              height={size.h}
              nodeColor={(n) => (n as GraphNode).color || "#475569"}
              nodeVal={(n) => Math.min(20, (n as GraphNode).val ?? 1) * 4}
              nodeLabel={(n) => (n as GraphNode).name}
              linkColor={() => "rgba(148, 163, 184, 0.3)"}
              linkWidth={1}
              cooldownTicks={120}
              onNodeClick={(n) => {
                navigate(`/knowledge?id=${encodeURIComponent((n as GraphNode).id)}`)
              }}
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
          )}
        </CardContent>
      </Card>
    </div>
  )
}
