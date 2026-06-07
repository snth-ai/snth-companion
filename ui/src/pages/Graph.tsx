import { useEffect, useMemo, useRef, useState } from "react"
import ForceGraph2D from "react-force-graph-2d"
import { useNavigate } from "react-router-dom"
import ReactMarkdown from "react-markdown"
import remarkGfm from "remark-gfm"
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import { Button } from "@/components/ui/button"
import { Badge } from "@/components/ui/badge"
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { Sparkles, X, ExternalLink, Pencil, Wand2 } from "lucide-react"
import {
  dedupeNamespace,
  fetchAllEdges,
  fetchGraphExport,
  fetchGraphNode,
  fetchProjects,
  fetchWikiList,
  fetchWikiPage,
  seedSimilarEdges,
  type DedupeResponse,
  type GraphV2Edge,
  type GraphV2Node,
  type GraphV2Stats,
  type Project,
  type WikiPageDetail,
  type WikiPageLite,
} from "@/lib/api"
import { toast } from "sonner"

// Graph — full-bleed force-directed network. Two modes:
//   - "knowledge" (default on v2): the real memory-engine-v2 knowledge graph
//     (entities + typed relations from /api/graph/export). Node click → entity
//     detail (summary + 1-hop neighbours).
//   - "pages": the legacy wiki-pages graph (pages + their links). Node click →
//     page detail with markdown + links_in/out, seed-similar + dedupe tools.
// Layout strategy:
//   - Canvas fills the entire main area (escapes Layout's max-w-5xl
//     wrapper via `fixed inset-0 left-60`).
//   - Translucent glass-tile in the top-left corner holds the title,
//     mode toggle, filters, tools, and stats.
//   - Click a node → animated slide-in panel from the right.

type GraphMode = "knowledge" | "pages"

type GraphNode = {
  id: string
  name: string
  type: string
  project_id?: string | null
  color?: string
  val?: number
  summary?: string
}

type GraphLink = {
  source: string
  target: string
  relation?: string
}

// Tailwind-400 family — bright, distinct hues that pop on the deep
// dark vignette. Earlier palette mixed 500/600 shades and merged into a
// single haze on Mia's 470-node graph.
const TYPE_COLORS: Record<string, string> = {
  daily: "#94a3b8",      // slate-400
  dream: "#c084fc",      // purple-400
  theme: "#fb923c",      // orange-400
  decision: "#4ade80",   // green-400
  reflection: "#22d3ee", // cyan-400
  concept: "#60a5fa",    // blue-400
  project: "#facc15",    // yellow-400
  module: "#a3e635",     // lime-400
  entity: "#f472b6",     // pink-400
  source: "#94a3b8",     // slate-400
  meta: "#cbd5e1",       // slate-300
  summary: "#34d399",    // emerald-400
}

function nodeColor(p: WikiPageLite, projects: Project[]): string {
  if (p.project_id) {
    const proj = projects.find((pr) => pr.id === p.project_id)
    if (proj?.color) return proj.color
  }
  return TYPE_COLORS[p.type] ?? "#64748b"
}

// Convert 6-char hex to rgba string with given alpha. No 3-char shortcut.
function hexA(hex: string, a: number): string {
  const h = hex.replace("#", "")
  const r = parseInt(h.slice(0, 2), 16)
  const g = parseInt(h.slice(2, 4), 16)
  const b = parseInt(h.slice(4, 6), 16)
  return `rgba(${r},${g},${b},${a})`
}

type EntityNeighbor = {
  node: GraphV2Node
  relation: string
  dir: "out" | "in"
}

type EntityDetail = {
  center: GraphV2Node
  neighbors: EntityNeighbor[]
}

// Human labels for entity-type colors (legend). Keys match server `type`.
const TYPE_LABELS: Record<string, string> = {
  person: "person",
  project: "project",
  place: "place",
  organization: "org",
  concept: "concept",
  event: "event",
  skill: "skill",
  tool: "tool",
}

// Knowledge-graph entity colors (mirror the synth `nodeColors` map so the
// legend matches what the server paints).
const ENTITY_COLORS: Record<string, string> = {
  person: "#4A90D9",
  project: "#7B68EE",
  place: "#2ECC71",
  organization: "#E67E22",
  concept: "#9B59B6",
  event: "#E74C3C",
  skill: "#00BCD4",
  tool: "#FF9800",
}

export function GraphPage() {
  const [mode, setMode] = useState<GraphMode>("knowledge")

  // knowledge-mode view controls (declutter the ~2000-node hairball)
  const [typeFilter, setTypeFilter] = useState<string>("__all__")
  const [minMentions, setMinMentions] = useState(0)
  const [maxNodes, setMaxNodes] = useState(250)
  const [search, setSearch] = useState("")

  // pages-mode state
  const [projects, setProjects] = useState<Project[]>([])
  const [pages, setPages] = useState<WikiPageLite[]>([])
  const [edges, setEdges] = useState<GraphLink[]>([])
  const [loadingEdges, setLoadingEdges] = useState(false)
  const [filterProj, setFilterProj] = useState<string>("__all__")
  const [seeding, setSeeding] = useState(false)
  const [selectedDetail, setSelectedDetail] = useState<WikiPageDetail | null>(
    null,
  )

  // knowledge-mode state
  const [gNodes, setGNodes] = useState<GraphV2Node[]>([])
  const [gEdges, setGEdges] = useState<GraphV2Edge[]>([])
  const [gStats, setGStats] = useState<GraphV2Stats | null>(null)
  const [loadingGraph, setLoadingGraph] = useState(false)
  const [entityDetail, setEntityDetail] = useState<EntityDetail | null>(null)

  // shared
  const [err, setErr] = useState<string | null>(null)
  const [selectedID, setSelectedID] = useState<string | null>(null)
  const [detailLoading, setDetailLoading] = useState(false)
  const navigate = useNavigate()
  const containerRef = useRef<HTMLDivElement | null>(null)
  const fgRef = useRef<{
    refresh?: () => void
    d3Force?: (name: string) => { strength?: (n: number) => void; distance?: (n: number) => void } | undefined
    d3ReheatSimulation?: () => void
  } | null>(null)
  const [size, setSize] = useState({ w: 800, h: 600 })
  const [dedupeOpen, setDedupeOpen] = useState(false)
  const [dedupeNS, setDedupeNS] = useState("themes")
  const [dedupeThreshold, setDedupeThreshold] = useState(0.94)
  const [dedupePlan, setDedupePlan] = useState<DedupeResponse | null>(null)
  const [dedupeLoading, setDedupeLoading] = useState(false)
  const [dedupeApplying, setDedupeApplying] = useState(false)

  // Reset selection when switching modes (ids are not interchangeable).
  useEffect(() => {
    setSelectedID(null)
    setErr(null)
  }, [mode])

  // Knowledge-mode load: the full entity graph in one round-trip.
  useEffect(() => {
    if (mode !== "knowledge") return
    let cancelled = false
    setLoadingGraph(true)
    void (async () => {
      try {
        const g = await fetchGraphExport()
        if (cancelled) return
        setGNodes(g.nodes ?? [])
        setGEdges(g.edges ?? [])
        setGStats(g.stats ?? null)
      } catch (e) {
        if (!cancelled) setErr(String((e as Error).message ?? e))
      } finally {
        if (!cancelled) setLoadingGraph(false)
      }
    })()
    return () => {
      cancelled = true
    }
  }, [mode])

  // Pages-mode initial load.
  useEffect(() => {
    if (mode !== "pages") return
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
  }, [mode])

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

  // Pages-mode bulk edge fetch — one round-trip via /api/wiki/edges (v0.4.45+).
  useEffect(() => {
    if (mode !== "pages") return
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
  }, [pages, mode])

  // Lazy detail fetch when a node is clicked (mode-aware).
  useEffect(() => {
    if (!selectedID) {
      setSelectedDetail(null)
      setEntityDetail(null)
      return
    }
    let cancelled = false
    setDetailLoading(true)
    setSelectedDetail(null)
    setEntityDetail(null)
    void (async () => {
      try {
        if (mode === "knowledge") {
          const d = await fetchGraphNode(selectedID)
          if (cancelled) return
          const nodes = d.nodes ?? []
          const dedges = d.edges ?? []
          const center = nodes.find((n) => n.id === selectedID) ?? null
          const relOf = (
            nid: string,
          ): { relation: string; dir: "out" | "in" } => {
            for (const e of dedges) {
              if (e.from === selectedID && e.to === nid)
                return { relation: e.label, dir: "out" }
              if (e.to === selectedID && e.from === nid)
                return { relation: e.label, dir: "in" }
            }
            return { relation: "", dir: "out" }
          }
          const neighbors: EntityNeighbor[] = nodes
            .filter((n) => n.id !== selectedID)
            .map((n) => ({ node: n, ...relOf(n.id) }))
            .sort((a, b) => b.node.mention_count - a.node.mention_count)
          if (center) setEntityDetail({ center, neighbors })
        } else {
          const d = await fetchWikiPage(selectedID)
          if (!cancelled) setSelectedDetail(d)
        }
      } catch (e) {
        if (!cancelled) toast.error(String((e as Error).message ?? e))
      } finally {
        if (!cancelled) setDetailLoading(false)
      }
    })()
    return () => {
      cancelled = true
    }
  }, [selectedID, mode])

  // Esc closes the panel.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setSelectedID(null)
    }
    window.addEventListener("keydown", onKey)
    return () => window.removeEventListener("keydown", onKey)
  }, [])

  // Pulse loop — keeps the canvas redrawing at 60 fps while a node is
  // selected. Without this, ForceGraph stops painting once the simulation
  // settles, so our time-based pulse colors freeze. Time read inside the
  // canvas callbacks is the actual driver; we just need the refresh
  // ticker to fire.
  useEffect(() => {
    if (!selectedID) return
    let raf = 0
    const tick = () => {
      fgRef.current?.refresh?.()
      raf = requestAnimationFrame(tick)
    }
    raf = requestAnimationFrame(tick)
    return () => cancelAnimationFrame(raf)
  }, [selectedID])

  const { nodes, links } = useMemo(() => {
    const idOf = (v: unknown): string =>
      typeof v === "string" ? v : ((v as { id?: string })?.id ?? "")

    if (mode === "knowledge") {
      const q = search.trim().toLowerCase()
      // 1. base filter: type + salience floor.
      let pool = gNodes.filter(
        (n) =>
          (typeFilter === "__all__" || n.type === typeFilter) &&
          n.mention_count >= minMentions,
      )
      // 2. search: narrow to name matches + their 1-hop neighbours so a single
      //    entity's neighbourhood is legible instead of the whole hairball.
      if (q) {
        const matches = new Set(
          pool.filter((n) => (n.label || "").toLowerCase().includes(q)).map((n) => n.id),
        )
        const keep = new Set(matches)
        for (const e of gEdges) {
          if (matches.has(e.from)) keep.add(e.to)
          if (matches.has(e.to)) keep.add(e.from)
        }
        pool = pool.filter((n) => keep.has(n.id))
      }
      // 3. cap: render only the top-N by salience so the default isn't a mush.
      pool = [...pool].sort((a, b) => b.mention_count - a.mention_count)
      if (maxNodes > 0 && pool.length > maxNodes) pool = pool.slice(0, maxNodes)

      const idSet = new Set(pool.map((n) => n.id))
      const ns: GraphNode[] = pool.map((n) => ({
        id: n.id,
        name: n.label || n.id,
        type: n.type,
        color: n.color || ENTITY_COLORS[n.type] || "#64748b",
        // Size by salience: mention_count drives radius (capped by nodeVal).
        val: 1 + Math.min(19, n.mention_count),
        summary: n.summary,
      }))
      const ls: GraphLink[] = []
      for (const e of gEdges) {
        if (!idSet.has(e.from) || !idSet.has(e.to)) continue
        ls.push({ source: e.from, target: e.to, relation: e.label })
      }
      return { nodes: ns, links: ls }
    }

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
  }, [
    mode,
    gNodes,
    gEdges,
    pages,
    projects,
    edges,
    filterProj,
    typeFilter,
    minMentions,
    maxNodes,
    search,
  ])

  // Total knowledge-graph node count (pre-cap) for the "showing N of M" stat
  // and to bound the min-mentions slider.
  const gTotals = useMemo(() => {
    let max = 0
    const types = new Map<string, number>()
    for (const n of gNodes) {
      if (n.mention_count > max) max = n.mention_count
      types.set(n.type, (types.get(n.type) ?? 0) + 1)
    }
    const present = [...types.entries()].sort((a, b) => b[1] - a[1])
    return { total: gNodes.length, maxMentions: max, types: present }
  }, [gNodes])

  // Spread the knowledge graph so it reads as a network, not a clump: stronger
  // charge repulsion + longer link distance, then reheat the sim.
  useEffect(() => {
    if (mode !== "knowledge") return
    const fg = fgRef.current
    if (!fg?.d3Force) return
    fg.d3Force("charge")?.strength?.(-160)
    fg.d3Force("link")?.distance?.(60)
    fg.d3ReheatSimulation?.()
  }, [mode, nodes.length, links.length])

  // Connected node IDs — 1-hop neighbours of selectedID (from the rendered links).
  const connectedIDs = useMemo(() => {
    const s = new Set<string>()
    if (!selectedID) return s
    const idOf = (v: unknown) =>
      typeof v === "string" ? v : ((v as { id?: string })?.id ?? "")
    for (const e of links) {
      const src = idOf(e.source)
      const tgt = idOf(e.target)
      if (src === selectedID) s.add(tgt)
      if (tgt === selectedID) s.add(src)
    }
    return s
  }, [selectedID, links])

  const projectOf = (pid?: string | null) =>
    pid ? projects.find((p) => p.id === pid) : undefined

  // Open the dedupe dialog & immediately fetch the dry-run plan so the
  // user sees what would be merged before they commit.
  const openDedupeDialog = async () => {
    setDedupeOpen(true)
    setDedupePlan(null)
    setDedupeLoading(true)
    try {
      const r = await dedupeNamespace(dedupeNS, dedupeThreshold, true)
      setDedupePlan(r)
    } catch (e) {
      toast.error(String((e as Error).message ?? e))
      setDedupeOpen(false)
    } finally {
      setDedupeLoading(false)
    }
  }

  // Refresh the dry-run plan when threshold changes (debounced via a
  // simple guard — only fire when dialog is open and not currently
  // applying).
  useEffect(() => {
    if (!dedupeOpen || dedupeApplying) return
    let cancelled = false
    setDedupeLoading(true)
    void (async () => {
      try {
        const r = await dedupeNamespace(dedupeNS, dedupeThreshold, true)
        if (!cancelled) setDedupePlan(r)
      } catch (e) {
        if (!cancelled) toast.error(String((e as Error).message ?? e))
      } finally {
        if (!cancelled) setDedupeLoading(false)
      }
    })()
    return () => {
      cancelled = true
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [dedupeNS, dedupeThreshold])

  const applyDedupe = async () => {
    setDedupeApplying(true)
    try {
      const r = await dedupeNamespace(dedupeNS, dedupeThreshold, false)
      toast.success(
        `merged ${r.deleted} pages into ${r.clusters} canonicals (${r.edges_merged} edges)`,
      )
      setDedupeOpen(false)
      // Refetch graph data so cleanup is visible immediately.
      const [pj, pg] = await Promise.all([
        fetchProjects(),
        fetchWikiList({ limit: 1000 }),
      ])
      setProjects(pj.projects ?? [])
      setPages(pg.pages ?? [])
    } catch (e) {
      toast.error(String((e as Error).message ?? e))
    } finally {
      setDedupeApplying(false)
    }
  }

  const busyLoading = mode === "knowledge" ? loadingGraph : loadingEdges

  return (
    // fixed inset-0 left-60 = full-bleed of main area (sidebar is w-60 = 240px)
    // Subtle radial vignette pushes focus toward the centre of the network.
    <div
      className="fixed inset-0 left-60 overflow-hidden"
      style={{
        background:
          "radial-gradient(ellipse at center, hsl(220 30% 8%) 0%, hsl(220 35% 4%) 80%)",
      }}
    >
      {/* full-bleed canvas wrapper — ResizeObserver feeds canvas dimensions */}
      <div ref={containerRef} className="absolute inset-0">
        {nodes.length > 0 ? (
          <ForceGraph2D
            ref={fgRef as never}
            graphData={{ nodes, links }}
            width={size.w}
            height={size.h}
            backgroundColor="rgba(0,0,0,0)"
            nodeLabel={(n) => (n as GraphNode).name}
            linkColor={(l) => {
              const idOf = (v: unknown) =>
                typeof v === "string"
                  ? v
                  : ((v as { id?: string })?.id ?? "")
              const src = idOf((l as GraphLink).source)
              const tgt = idOf((l as GraphLink).target)
              const isConnected =
                selectedID && (src === selectedID || tgt === selectedID)
              if (!isConnected) {
                return "rgba(148, 163, 184, 0.12)"
              }
              // Pulsing cyan-ish glow for the selected node's edges.
              const phase = 0.5 + 0.5 * Math.sin(Date.now() / 380)
              const a = 0.35 + 0.5 * phase
              return `rgba(125, 211, 252, ${a})` // sky-300
            }}
            linkWidth={(l) => {
              const idOf = (v: unknown) =>
                typeof v === "string"
                  ? v
                  : ((v as { id?: string })?.id ?? "")
              const src = idOf((l as GraphLink).source)
              const tgt = idOf((l as GraphLink).target)
              const isConnected =
                selectedID && (src === selectedID || tgt === selectedID)
              if (!isConnected) return 0.6
              const phase = 0.5 + 0.5 * Math.sin(Date.now() / 380)
              return 1.5 + 1.5 * phase
            }}
            linkDirectionalParticles={(l) => {
              const idOf = (v: unknown) =>
                typeof v === "string"
                  ? v
                  : ((v as { id?: string })?.id ?? "")
              const src = idOf((l as GraphLink).source)
              const tgt = idOf((l as GraphLink).target)
              return selectedID && (src === selectedID || tgt === selectedID)
                ? 2
                : 0
            }}
            linkDirectionalParticleSpeed={0.006}
            linkDirectionalParticleColor={() => "rgba(186, 230, 253, 0.9)"}
            cooldownTicks={120}
            onNodeClick={(n) => setSelectedID((n as GraphNode).id)}
            onBackgroundClick={() => setSelectedID(null)}
            nodeCanvasObjectMode={(n) => {
              const isFocus =
                (n as GraphNode).id === selectedID ||
                connectedIDs.has((n as GraphNode).id)
              return isFocus ? "replace" : "after"
            }}
            nodeCanvasObject={(n, ctx, scale) => {
              const node = n as GraphNode & { x?: number; y?: number }
              if (!node.x || !node.y) return
              const isSelected = node.id === selectedID
              const isConnected = connectedIDs.has(node.id)
              const color = node.color || "#64748b"
              // Match ForceGraph's default radius formula: sqrt(val * 4) when
              // the lib draws the circle; we replicate so our halo aligns.
              const baseR = Math.sqrt(Math.min(20, node.val ?? 1) * 4)

              if (isSelected || isConnected) {
                // Halo via radial gradient — bigger + brighter for the
                // selected node, smaller for 1-hop neighbours.
                const phase = 0.5 + 0.5 * Math.sin(Date.now() / 600)
                const haloMul = isSelected ? 4.5 + phase * 1.5 : 2.6
                const haloR = baseR * haloMul
                const grad = ctx.createRadialGradient(
                  node.x,
                  node.y,
                  baseR,
                  node.x,
                  node.y,
                  haloR,
                )
                grad.addColorStop(
                  0,
                  hexA(color, isSelected ? 0.55 : 0.3),
                )
                grad.addColorStop(1, hexA(color, 0))
                ctx.fillStyle = grad
                ctx.beginPath()
                ctx.arc(node.x, node.y, haloR, 0, Math.PI * 2)
                ctx.fill()

                // Solid node fill on top of the halo.
                const r = baseR * (isSelected ? 1.35 : 1)
                ctx.fillStyle = color
                ctx.beginPath()
                ctx.arc(node.x, node.y, r, 0, Math.PI * 2)
                ctx.fill()

                // Crisp outline — bright white for selected, soft for connected.
                ctx.strokeStyle = isSelected
                  ? "rgba(255,255,255,0.95)"
                  : "rgba(255,255,255,0.45)"
                ctx.lineWidth = (isSelected ? 1.8 : 1.0) / scale
                ctx.stroke()

                // Always-visible label for focus nodes (selected always,
                // connected only at decent zoom).
                if (isSelected || scale > 0.9) {
                  ctx.font = `${(isSelected ? 12 : 10) / scale}px sans-serif`
                  ctx.fillStyle = isSelected ? "#fff" : "#cbd5e1"
                  ctx.textAlign = "center"
                  ctx.textBaseline = "top"
                  ctx.fillText(node.name, node.x, node.y + r + 4 / scale)
                }
                return
              }

              // After-mode for the 99% case — lib has already drawn the
              // circle, we just paint the label at higher zoom levels.
              if (scale < 1.5) return
              ctx.font = `${10 / scale}px sans-serif`
              ctx.fillStyle = "#cbd5e1"
              ctx.textAlign = "center"
              ctx.textBaseline = "top"
              ctx.fillText(node.name, node.x, node.y + 5)
            }}
            nodeColor={(n) => (n as GraphNode).color || "#64748b"}
            nodeVal={(n) => Math.min(20, (n as GraphNode).val ?? 1) * 4}
          />
        ) : (
          <div className="absolute inset-0 grid place-items-center text-sm text-muted-foreground italic">
            {busyLoading
              ? "loading graph…"
              : mode === "knowledge"
              ? "no entities to visualize"
              : "no pages to visualize"}
          </div>
        )}
      </div>

      {/* Glass-tile overlay — title + mode toggle + filter + tools + stats */}
      <div className="absolute top-4 left-4 max-w-md pointer-events-auto">
        <div className="bg-card/70 backdrop-blur-md border border-border/50 rounded-lg shadow-2xl p-4 space-y-3">
          <div>
            <h1 className="text-xl font-semibold tracking-tight">Graph</h1>
            <p className="text-xs text-muted-foreground mt-1">
              {mode === "knowledge"
                ? "Knowledge graph — entities and their relations. Click a node to see its connections. Bigger nodes = mentioned more."
                : "Force-directed view of pages + their links. Click a node to preview it. Bigger nodes = more inbound links."}
            </p>
          </div>

          {/* Mode toggle */}
          <div className="inline-flex rounded-md border border-border overflow-hidden text-xs">
            <button
              className={
                "px-3 py-1.5 transition " +
                (mode === "knowledge"
                  ? "bg-primary text-primary-foreground"
                  : "bg-background/80 text-muted-foreground hover:text-foreground")
              }
              onClick={() => setMode("knowledge")}
            >
              Knowledge
            </button>
            <button
              className={
                "px-3 py-1.5 transition border-l border-border " +
                (mode === "pages"
                  ? "bg-primary text-primary-foreground"
                  : "bg-background/80 text-muted-foreground hover:text-foreground")
              }
              onClick={() => setMode("pages")}
            >
              Pages
            </button>
          </div>

          {mode === "knowledge" && (
            <div className="space-y-2">
              <div className="flex items-center gap-2 flex-wrap">
                <input
                  value={search}
                  onChange={(e) => setSearch(e.target.value)}
                  placeholder="search entity…"
                  className="text-xs bg-background/80 border border-border rounded px-2 py-1.5 w-36"
                />
                <select
                  value={typeFilter}
                  onChange={(e) => setTypeFilter(e.target.value)}
                  className="text-xs bg-background/80 border border-border rounded px-2 py-1.5"
                  title="filter by entity type"
                >
                  <option value="__all__">all types</option>
                  {gTotals.types.map(([t, c]) => (
                    <option key={t} value={t}>
                      {(TYPE_LABELS[t] ?? t) + ` (${c})`}
                    </option>
                  ))}
                </select>
                <select
                  value={maxNodes}
                  onChange={(e) => setMaxNodes(Number(e.target.value))}
                  className="text-xs bg-background/80 border border-border rounded px-2 py-1.5"
                  title="max nodes rendered (top by mentions)"
                >
                  <option value={100}>top 100</option>
                  <option value={250}>top 250</option>
                  <option value={500}>top 500</option>
                  <option value={0}>all</option>
                </select>
              </div>
              <div className="flex items-center gap-2">
                <label className="text-[11px] text-muted-foreground whitespace-nowrap">
                  min mentions: {minMentions}
                </label>
                <input
                  type="range"
                  min={0}
                  max={Math.max(1, Math.min(20, gTotals.maxMentions))}
                  value={minMentions}
                  onChange={(e) => setMinMentions(Number(e.target.value))}
                  className="flex-1 accent-primary"
                />
              </div>
              {gTotals.types.length > 0 && (
                <div className="flex flex-wrap gap-x-3 gap-y-1 pt-0.5">
                  {gTotals.types.slice(0, 8).map(([t]) => (
                    <span
                      key={t}
                      className="inline-flex items-center gap-1 text-[10px] text-muted-foreground"
                    >
                      <span
                        className="inline-block h-2 w-2 rounded-full"
                        style={{ backgroundColor: ENTITY_COLORS[t] ?? "#64748b" }}
                      />
                      {TYPE_LABELS[t] ?? t}
                    </span>
                  ))}
                </div>
              )}
            </div>
          )}

          {mode === "pages" && (
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
              <Button
                size="sm"
                variant="outline"
                className="bg-background/80"
                onClick={openDedupeDialog}
                title="Find + merge near-duplicate themes (preview first)"
              >
                <Wand2 className="h-3.5 w-3.5 mr-1" />
                dedupe
              </Button>
            </div>
          )}

          <div className="text-[11px] text-muted-foreground tabular-nums">
            {mode === "knowledge"
              ? `showing ${nodes.length} of ${gTotals.total} nodes · ${links.length}${
                  gStats ? "/" + gStats.total_edges : ""
                } edges`
              : `${nodes.length} nodes · ${links.length} edges`}
            {busyLoading && " · loading…"}
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
              {mode === "pages" && (
                <>
                  <Button
                    size="sm"
                    variant="ghost"
                    onClick={() =>
                      navigate(
                        `/knowledge?id=${encodeURIComponent(selectedID)}`,
                      )
                    }
                    title="Open in Knowledge for editing"
                  >
                    <Pencil className="h-3.5 w-3.5 mr-1" /> edit
                  </Button>
                  <Button
                    size="sm"
                    variant="ghost"
                    onClick={() =>
                      navigate(
                        `/knowledge?id=${encodeURIComponent(selectedID)}`,
                      )
                    }
                    title="Open in Knowledge"
                  >
                    <ExternalLink className="h-3.5 w-3.5" />
                  </Button>
                </>
              )}
            </div>

            <div className="flex-1 overflow-auto px-5 py-4 space-y-3">
              {detailLoading ? (
                <div className="text-sm italic text-muted-foreground">
                  loading…
                </div>
              ) : mode === "knowledge" ? (
                entityDetail ? (
                  <>
                    <div>
                      <div className="flex items-center gap-2 flex-wrap mb-1">
                        <Badge
                          variant="secondary"
                          className="text-[10px] uppercase tracking-wider"
                        >
                          {entityDetail.center.type || "entity"}
                        </Badge>
                        {entityDetail.center.mention_count > 0 && (
                          <Badge variant="outline" className="text-[10px]">
                            {entityDetail.center.mention_count} mentions
                          </Badge>
                        )}
                      </div>
                      <h2 className="text-lg font-semibold leading-tight">
                        {entityDetail.center.label || entityDetail.center.id}
                      </h2>
                    </div>

                    {entityDetail.center.summary ? (
                      <article className="prose prose-invert prose-sm max-w-none">
                        <ReactMarkdown remarkPlugins={[remarkGfm]}>
                          {entityDetail.center.summary}
                        </ReactMarkdown>
                      </article>
                    ) : (
                      <div className="text-sm italic text-muted-foreground">
                        no summary
                      </div>
                    )}

                    {entityDetail.neighbors.length > 0 && (
                      <div className="border-t border-border pt-3 space-y-2">
                        <div className="text-[10px] uppercase tracking-wider text-muted-foreground">
                          connected to ({entityDetail.neighbors.length})
                        </div>
                        <div className="flex flex-col gap-1">
                          {entityDetail.neighbors.map((nb) => (
                            <button
                              key={nb.node.id}
                              onClick={() => setSelectedID(nb.node.id)}
                              className="text-xs px-2 py-1 rounded bg-muted/50 hover:bg-muted text-foreground transition flex items-center gap-1.5 text-left"
                              title={nb.node.type}
                            >
                              <span
                                className="inline-block h-2 w-2 rounded-full shrink-0"
                                style={{ backgroundColor: nb.node.color }}
                              />
                              <span className="text-muted-foreground shrink-0">
                                {nb.dir === "in" ? "←" : "→"}
                              </span>
                              <span className="truncate">
                                {nb.node.label || nb.node.id}
                              </span>
                              {nb.relation && (
                                <span className="text-[10px] text-muted-foreground/70 italic ml-auto pl-2 shrink-0">
                                  {nb.relation}
                                </span>
                              )}
                            </button>
                          ))}
                        </div>
                      </div>
                    )}
                  </>
                ) : (
                  <div className="text-sm italic text-muted-foreground">
                    no detail
                  </div>
                )
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

      <Dialog
        open={dedupeOpen}
        onOpenChange={(o) => !dedupeApplying && setDedupeOpen(o)}
      >
        <DialogContent className="sm:max-w-2xl">
          <DialogHeader>
            <DialogTitle className="flex items-center gap-2">
              <Wand2 className="h-4 w-4" />
              Cleanup duplicate pages
            </DialogTitle>
          </DialogHeader>
          <div className="space-y-3">
            <div className="flex items-end gap-3">
              <div className="space-y-1">
                <label className="text-xs uppercase tracking-wider text-muted-foreground">
                  Namespace
                </label>
                <select
                  value={dedupeNS}
                  onChange={(e) => setDedupeNS(e.target.value)}
                  className="text-sm bg-background border border-border rounded px-2 h-8 w-32"
                  disabled={dedupeApplying}
                >
                  <option value="themes">themes</option>
                  <option value="dreams">dreams</option>
                  <option value="personal">personal</option>
                </select>
              </div>
              <div className="space-y-1">
                <label className="text-xs uppercase tracking-wider text-muted-foreground">
                  Cosine threshold
                </label>
                <select
                  value={dedupeThreshold}
                  onChange={(e) => setDedupeThreshold(Number(e.target.value))}
                  className="text-sm bg-background border border-border rounded px-2 h-8 w-32"
                  disabled={dedupeApplying}
                >
                  <option value="0.90">0.90 (broad)</option>
                  <option value="0.92">0.92</option>
                  <option value="0.94">0.94 (recommended)</option>
                  <option value="0.96">0.96</option>
                  <option value="0.98">0.98 (strict)</option>
                </select>
              </div>
              {dedupeLoading && (
                <span className="text-xs italic text-muted-foreground self-center pb-1">
                  computing plan…
                </span>
              )}
            </div>

            {dedupePlan && !dedupeLoading && (
              <>
                <div className="flex flex-wrap gap-3 text-sm pt-2 border-t border-border">
                  <Stat label="scanned" value={String(dedupePlan.scanned)} />
                  <Stat
                    label="clusters"
                    value={String(dedupePlan.clusters)}
                  />
                  <Stat
                    label="will delete"
                    value={String(dedupePlan.would_delete ?? 0)}
                    accent="text-orange-400"
                  />
                  <Stat
                    label="after cleanup"
                    value={String(
                      dedupePlan.scanned -
                        (dedupePlan.would_delete ?? 0),
                    )}
                    accent="text-emerald-400"
                  />
                </div>

                {dedupePlan.plan.length === 0 ? (
                  <div className="text-sm italic text-muted-foreground py-6 text-center">
                    No duplicates at this threshold — nothing to merge.
                  </div>
                ) : (
                  <div className="max-h-80 overflow-auto border border-border rounded p-3 space-y-3 bg-background/50">
                    {dedupePlan.plan
                      .slice()
                      .sort(
                        (a, b) => b.dupes.length - a.dupes.length,
                      )
                      .map((pe) => (
                        <div key={pe.canonical} className="space-y-1">
                          <div className="text-xs">
                            <span className="text-emerald-400">
                              KEEP
                            </span>{" "}
                            <span className="font-medium text-foreground">
                              {pe.canonical_title}
                            </span>{" "}
                            <span className="font-mono text-[10px] text-muted-foreground">
                              {pe.canonical}
                            </span>
                          </div>
                          {pe.dupes.map((d, i) => (
                            <div
                              key={d}
                              className="text-xs pl-6 text-muted-foreground line-through decoration-orange-500/60"
                            >
                              {pe.dupe_titles[i] || d}{" "}
                              <span className="font-mono text-[10px] opacity-60">
                                {d}
                              </span>
                            </div>
                          ))}
                        </div>
                      ))}
                  </div>
                )}
              </>
            )}
          </div>
          <DialogFooter>
            <Button
              variant="ghost"
              onClick={() => setDedupeOpen(false)}
              disabled={dedupeApplying}
            >
              Cancel
            </Button>
            <Button
              onClick={applyDedupe}
              disabled={
                dedupeApplying ||
                dedupeLoading ||
                !dedupePlan ||
                (dedupePlan.would_delete ?? 0) === 0
              }
              variant="destructive"
            >
              {dedupeApplying
                ? "applying…"
                : `Apply (delete ${dedupePlan?.would_delete ?? 0})`}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  )
}

function Stat({
  label,
  value,
  accent,
}: {
  label: string
  value: string
  accent?: string
}) {
  return (
    <div className="space-y-0.5">
      <div
        className={
          "text-base font-semibold tabular-nums " + (accent ?? "")
        }
      >
        {value}
      </div>
      <div className="text-[10px] uppercase tracking-wider text-muted-foreground">
        {label}
      </div>
    </div>
  )
}
