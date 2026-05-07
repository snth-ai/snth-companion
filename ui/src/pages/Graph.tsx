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
  fetchProjects,
  fetchWikiList,
  fetchWikiPage,
  seedSimilarEdges,
  type DedupeResponse,
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
  const fgRef = useRef<{ refresh?: () => void } | null>(null)
  const [size, setSize] = useState({ w: 800, h: 600 })
  const [dedupeOpen, setDedupeOpen] = useState(false)
  const [dedupeNS, setDedupeNS] = useState("themes")
  const [dedupeThreshold, setDedupeThreshold] = useState(0.94)
  const [dedupePlan, setDedupePlan] = useState<DedupeResponse | null>(null)
  const [dedupeLoading, setDedupeLoading] = useState(false)
  const [dedupeApplying, setDedupeApplying] = useState(false)

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

  // Connected node IDs — 1-hop neighbours of selectedID.
  const connectedIDs = useMemo(() => {
    const s = new Set<string>()
    if (!selectedID) return s
    const idOf = (v: unknown) =>
      typeof v === "string" ? v : ((v as { id?: string })?.id ?? "")
    for (const e of edges) {
      const src = idOf(e.source)
      const tgt = idOf(e.target)
      if (src === selectedID) s.add(tgt)
      if (tgt === selectedID) s.add(src)
    }
    return s
  }, [selectedID, edges])

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
