import { useEffect, useMemo, useRef, useState } from "react"
import ReactMarkdown from "react-markdown"
import remarkGfm from "remark-gfm"
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { Input } from "@/components/ui/input"
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import {
  Trash2,
  Plus,
  Pencil,
  Save,
  XCircle,
  Folder,
  FolderPlus,
  Eye,
  Network,
} from "lucide-react"
import {
  assignWikiProject,
  deleteProject,
  deleteWikiPage,
  fetchProjects,
  fetchWikiList,
  fetchWikiPage,
  fetchWikiSimilar,
  linkWikiPages,
  unlinkWikiPages,
  upsertProject,
  upsertWikiPage,
  type Project,
  type WikiPageDetail,
  type WikiPageLite,
  type WikiSimilar,
} from "@/lib/api"
import { toast } from "sonner"
import { useNavigate } from "react-router-dom"

// Knowledge — the collaborative wiki workspace (v0.4.42, Wave 9.5).
// Three columns:
//   left:   projects sidebar
//   middle: page list filtered by project
//   right:  page viewer/editor with markdown render, suggested
//           connections, backlinks, project assign
//
// Wikilinks: [[Page Name]] in markdown turn into clickable jumps that
// resolve title→id via the page index. Future: typeahead inside editor
// when user types `[[`.

const NO_PROJECT = "__none__"
const ALL_PROJECTS = "__all__"

// Resolve a wikilink target — slug-style match against the page index.
function resolveWikilink(
  target: string,
  pages: WikiPageLite[],
): WikiPageLite | undefined {
  const t = target.trim().toLowerCase()
  // Exact id match.
  let hit = pages.find((p) => p.id.toLowerCase() === t)
  if (hit) return hit
  // Title match (case-insensitive).
  hit = pages.find((p) => p.title.toLowerCase() === t)
  if (hit) return hit
  // Loose contains.
  return pages.find(
    (p) =>
      p.id.toLowerCase().includes(t) ||
      p.title.toLowerCase().includes(t),
  )
}

// Pre-process [[Wikilinks]] → "[Wikilinks](#wikilink/<encoded>)" so
// react-markdown's link renderer can intercept.
function expandWikilinks(md: string): string {
  return md.replace(/\[\[([^\]]+)\]\]/g, (_, target: string) => {
    const t = target.trim()
    return `[${t}](#wikilink/${encodeURIComponent(t)})`
  })
}

export function KnowledgePage() {
  const [projects, setProjects] = useState<Project[]>([])
  const [pages, setPages] = useState<WikiPageLite[]>([])
  const [activeProject, setActiveProject] = useState<string>(ALL_PROJECTS)
  const [filter, setFilter] = useState("")
  const [selectedID, setSelectedID] = useState<string | null>(null)
  const [detail, setDetail] = useState<WikiPageDetail | null>(null)
  const [similar, setSimilar] = useState<WikiSimilar[]>([])
  const [editing, setEditing] = useState(false)
  const [editBuf, setEditBuf] = useState({ title: "", content: "" })
  const [err, setErr] = useState<string | null>(null)
  const navigate = useNavigate()

  const loadProjects = async () => {
    try {
      const d = await fetchProjects()
      setProjects(d.projects ?? [])
    } catch (e) {
      setErr(String((e as Error).message ?? e))
    }
  }

  const loadPages = async () => {
    try {
      const opts: Parameters<typeof fetchWikiList>[0] = { limit: 1000 }
      if (activeProject === NO_PROJECT) opts.project_id = "none"
      else if (activeProject !== ALL_PROJECTS) opts.project_id = activeProject
      const d = await fetchWikiList(opts)
      setPages(d.pages ?? [])
    } catch (e) {
      setErr(String((e as Error).message ?? e))
    }
  }

  useEffect(() => {
    void loadProjects()
  }, [])

  useEffect(() => {
    void loadPages()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeProject])

  // Detail + similar fetch when selection changes.
  useEffect(() => {
    if (!selectedID) {
      setDetail(null)
      setSimilar([])
      return
    }
    void (async () => {
      try {
        const d = await fetchWikiPage(selectedID)
        setDetail(d)
        setEditBuf({ title: d.title, content: d.content })
        setEditing(false)
      } catch (e) {
        setErr(String((e as Error).message ?? e))
      }
      try {
        const s = await fetchWikiSimilar(selectedID, 6)
        setSimilar(s.similar ?? [])
      } catch {
        setSimilar([])
      }
    })()
  }, [selectedID])

  const visiblePages = useMemo(() => {
    const q = filter.trim().toLowerCase()
    if (!q) return pages
    return pages.filter(
      (p) =>
        p.title.toLowerCase().includes(q) ||
        p.id.toLowerCase().includes(q) ||
        p.snippet?.toLowerCase().includes(q),
    )
  }, [pages, filter])

  const handleSave = async () => {
    if (!detail) return
    try {
      const updated = await upsertWikiPage(
        {
          page_id: detail.id,
          title: editBuf.title,
          content: editBuf.content,
          type: detail.type,
          namespace: detail.namespace,
        },
        activeProject !== ALL_PROJECTS && activeProject !== NO_PROJECT
          ? activeProject
          : undefined,
      )
      setDetail(updated)
      setEditing(false)
      toast.success("saved")
      void loadPages()
    } catch (e) {
      toast.error(String((e as Error).message ?? e))
    }
  }

  const handleNewPage = async () => {
    const slug = prompt(
      "new page slug (e.g. 'project/snth-platform'):",
      "page-" + Math.random().toString(36).slice(2, 8),
    )
    if (!slug) return
    try {
      const updated = await upsertWikiPage(
        {
          page_id: slug,
          title: slug,
          content: "# " + slug + "\n\n",
          type: "concept",
          namespace: "personal",
        },
        activeProject !== ALL_PROJECTS && activeProject !== NO_PROJECT
          ? activeProject
          : undefined,
      )
      void loadPages()
      setSelectedID(updated.id)
      setEditing(true)
    } catch (e) {
      toast.error(String((e as Error).message ?? e))
    }
  }

  const handleNewProject = async () => {
    const slug = prompt("new project slug (a-z, numbers, hyphens):")
    if (!slug) return
    const name = prompt("display name:", slug) || slug
    try {
      const p = await upsertProject({ slug, name })
      await loadProjects()
      setActiveProject(p.id)
    } catch (e) {
      toast.error(String((e as Error).message ?? e))
    }
  }

  const jumpByTitle = (target: string) => {
    const hit = resolveWikilink(target, pages)
    if (hit) setSelectedID(hit.id)
    else toast.error(`no page matches "${target}"`)
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
          <h1 className="text-2xl font-semibold tracking-tight">Knowledge</h1>
          <p className="text-sm text-muted-foreground mt-1">
            Shared wiki workspace. Use{" "}
            <code className="px-1 py-0.5 bg-muted rounded">[[Page Name]]</code>{" "}
            for inline links. Vector-similar pages auto-suggested.
          </p>
        </div>
        <div className="flex items-center gap-2">
          <Button size="sm" variant="secondary" onClick={() => navigate("/graph")}>
            <Network className="h-4 w-4 mr-1" /> Graph
          </Button>
          <Button size="sm" variant="outline" onClick={handleNewProject}>
            <FolderPlus className="h-4 w-4 mr-1" /> Project
          </Button>
          <Button size="sm" onClick={handleNewPage}>
            <Plus className="h-4 w-4 mr-1" /> Page
          </Button>
        </div>
      </div>

      <div className="grid grid-cols-1 lg:grid-cols-[200px_280px_1fr] gap-3">
        {/* Left: projects */}
        <div className="space-y-1 max-h-[80vh] overflow-y-auto pr-1">
          <ProjectButton
            label="all pages"
            active={activeProject === ALL_PROJECTS}
            onClick={() => setActiveProject(ALL_PROJECTS)}
            count={pages.length}
          />
          <ProjectButton
            label="unassigned"
            active={activeProject === NO_PROJECT}
            onClick={() => setActiveProject(NO_PROJECT)}
          />
          <div className="pt-2 text-xs uppercase tracking-wider text-muted-foreground px-2">
            Projects
          </div>
          {projects.length === 0 && (
            <div className="text-xs text-muted-foreground italic px-2 py-1">
              none yet
            </div>
          )}
          {projects.map((p) => (
            <div key={p.id} className="group relative">
              <ProjectButton
                label={p.name}
                active={activeProject === p.id}
                onClick={() => setActiveProject(p.id)}
                count={p.page_count}
                color={p.color || undefined}
                archived={p.status === "archived"}
              />
              <button
                title="delete project"
                onClick={async () => {
                  if (!confirm(`Delete project "${p.name}"? Pages stay; just unassigned.`)) return
                  try {
                    await deleteProject(p.id)
                    await loadProjects()
                    if (activeProject === p.id) setActiveProject(ALL_PROJECTS)
                  } catch (e) {
                    toast.error(String((e as Error).message ?? e))
                  }
                }}
                className="absolute right-1 top-1.5 opacity-0 group-hover:opacity-100 text-red-400 hover:text-red-300 px-1"
              >
                <Trash2 className="h-3 w-3" />
              </button>
            </div>
          ))}
        </div>

        {/* Middle: page list */}
        <div className="space-y-2">
          <Input
            placeholder="filter pages…"
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            className="h-8 text-sm"
          />
          <div className="space-y-1 max-h-[75vh] overflow-y-auto pr-1">
            {visiblePages.length === 0 && (
              <div className="text-sm text-muted-foreground italic">
                no pages
              </div>
            )}
            {visiblePages.map((p) => (
              <button
                key={p.id}
                onClick={() => setSelectedID(p.id)}
                className={
                  "w-full text-left rounded-md px-3 py-2 text-sm transition-colors " +
                  (selectedID === p.id
                    ? "bg-primary/15 text-foreground"
                    : "hover:bg-muted text-muted-foreground hover:text-foreground")
                }
              >
                <div className="flex items-center gap-2">
                  <span className="text-xs text-muted-foreground/60 font-mono">
                    {p.type[0]}
                  </span>
                  <span className="font-medium text-foreground line-clamp-1 flex-1">
                    {p.title || p.id}
                  </span>
                </div>
                {p.snippet && (
                  <div className="text-xs text-muted-foreground/80 line-clamp-1">
                    {p.snippet}
                  </div>
                )}
              </button>
            ))}
          </div>
        </div>

        {/* Right: viewer/editor */}
        <div className="space-y-3">
          {!detail && (
            <Card className="min-h-[60vh]">
              <CardContent className="pt-6 text-sm text-muted-foreground italic">
                Pick a page from the list — or hit "+ Page" to make a new one.
              </CardContent>
            </Card>
          )}

          {detail && (
            <>
              <Card>
                <CardHeader className="pb-2">
                  <CardTitle className="flex items-start gap-2 flex-wrap">
                    {editing ? (
                      <Input
                        value={editBuf.title}
                        onChange={(e) =>
                          setEditBuf((s) => ({ ...s, title: e.target.value }))
                        }
                        className="text-lg flex-1"
                      />
                    ) : (
                      <span className="flex-1">{detail.title || detail.id}</span>
                    )}
                    <Badge variant="secondary">{detail.type}</Badge>
                    <Badge variant="outline">{detail.namespace}</Badge>
                    <ProjectAssign
                      projects={projects}
                      currentID={detail.project_id ?? ""}
                      onChange={async (next) => {
                        try {
                          await assignWikiProject(detail.id, next)
                          toast.success(next ? "moved to project" : "unassigned")
                          void loadProjects()
                          void loadPages()
                          setDetail({ ...detail, project_id: next || undefined })
                        } catch (e) {
                          toast.error(String((e as Error).message ?? e))
                        }
                      }}
                    />
                    {!editing ? (
                      <>
                        <Button
                          size="sm"
                          variant="ghost"
                          onClick={() => setEditing(true)}
                        >
                          <Pencil className="h-4 w-4 mr-1" /> Edit
                        </Button>
                        <Button
                          size="sm"
                          variant="ghost"
                          className="text-red-400"
                          onClick={async () => {
                            if (!confirm(`Delete "${detail.title || detail.id}"?`)) return
                            try {
                              await deleteWikiPage(detail.id)
                              toast.success("deleted")
                              setSelectedID(null)
                              void loadPages()
                            } catch (e) {
                              toast.error(String((e as Error).message ?? e))
                            }
                          }}
                        >
                          <Trash2 className="h-4 w-4" />
                        </Button>
                      </>
                    ) : (
                      <>
                        <Button size="sm" onClick={handleSave}>
                          <Save className="h-4 w-4 mr-1" /> Save
                        </Button>
                        <Button
                          size="sm"
                          variant="ghost"
                          onClick={() => {
                            setEditing(false)
                            setEditBuf({
                              title: detail.title,
                              content: detail.content,
                            })
                          }}
                        >
                          <XCircle className="h-4 w-4" />
                        </Button>
                      </>
                    )}
                  </CardTitle>
                </CardHeader>
                <CardContent>
                  <div className="text-xs text-muted-foreground font-mono mb-3">
                    id: {detail.id} · updated {detail.updated_at}
                  </div>

                  {editing ? (
                    <WikiEditor
                      value={editBuf.content}
                      pages={pages}
                      onChange={(v) =>
                        setEditBuf((s) => ({ ...s, content: v }))
                      }
                    />
                  ) : (
                    <MarkdownView
                      content={detail.content}
                      onWikilink={jumpByTitle}
                    />
                  )}
                </CardContent>
              </Card>

              <ConnectionsPanel
                detail={detail}
                similar={similar}
                onJump={(id) => setSelectedID(id)}
                onLinked={async () => {
                  try {
                    const d = await fetchWikiPage(detail.id)
                    setDetail(d)
                  } catch {
                    /* ignore */
                  }
                }}
              />
            </>
          )}
        </div>
      </div>
    </div>
  )
}

function ProjectButton({
  label,
  active,
  onClick,
  count,
  color,
  archived,
}: {
  label: string
  active: boolean
  onClick: () => void
  count?: number
  color?: string
  archived?: boolean
}) {
  return (
    <button
      onClick={onClick}
      className={
        "w-full text-left rounded-md px-3 py-1.5 text-sm transition-colors flex items-center gap-2 " +
        (active
          ? "bg-primary/15 text-foreground"
          : "hover:bg-muted text-muted-foreground hover:text-foreground")
      }
    >
      <Folder
        className="h-3.5 w-3.5 shrink-0"
        style={color ? { color } : undefined}
      />
      <span className={archived ? "line-through opacity-60" : ""}>{label}</span>
      {typeof count === "number" && (
        <span className="ml-auto text-xs text-muted-foreground/70">{count}</span>
      )}
    </button>
  )
}

function ProjectAssign({
  projects,
  currentID,
  onChange,
}: {
  projects: Project[]
  currentID: string
  onChange: (id: string) => void
}) {
  return (
    <select
      value={currentID}
      onChange={(e) => onChange(e.target.value)}
      className="text-xs bg-muted/40 border border-border rounded px-1.5 py-0.5"
      title="assign to project"
    >
      <option value="">— project —</option>
      {projects.map((p) => (
        <option key={p.id} value={p.id}>
          {p.name}
        </option>
      ))}
    </select>
  )
}

function MarkdownView({
  content,
  onWikilink,
}: {
  content: string
  onWikilink: (target: string) => void
}) {
  const expanded = useMemo(() => expandWikilinks(content), [content])
  return (
    <div className="prose prose-invert prose-sm max-w-none prose-pre:bg-muted/50 prose-code:before:content-[''] prose-code:after:content-[''] max-h-[70vh] overflow-y-auto">
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        components={{
          a: ({ href, children, ...rest }) => {
            if (typeof href === "string" && href.startsWith("#wikilink/")) {
              const target = decodeURIComponent(href.slice("#wikilink/".length))
              return (
                <button
                  onClick={() => onWikilink(target)}
                  className="text-primary underline underline-offset-2 hover:text-primary/80"
                >
                  {children}
                </button>
              )
            }
            return (
              <a href={href} target="_blank" rel="noreferrer" {...rest}>
                {children}
              </a>
            )
          },
        }}
      >
        {expanded}
      </ReactMarkdown>
    </div>
  )
}

function ConnectionsPanel({
  detail,
  similar,
  onJump,
  onLinked,
}: {
  detail: WikiPageDetail
  similar: WikiSimilar[]
  onJump: (id: string) => void
  onLinked: () => Promise<void> | void
}) {
  const linkedIDs = useMemo(() => {
    const s = new Set<string>()
    for (const e of detail.links_out ?? []) s.add(e.page_id)
    for (const e of detail.links_in ?? []) s.add(e.page_id)
    return s
  }, [detail])
  const suggestions = useMemo(
    () =>
      similar.filter(
        (s) => !linkedIDs.has(s.page_id) && s.page_id !== detail.id,
      ),
    [similar, linkedIDs, detail.id],
  )
  return (
    <Card>
      <CardHeader className="pb-2">
        <CardTitle className="text-sm">Connections</CardTitle>
      </CardHeader>
      <CardContent className="space-y-3 text-sm">
        <ConnectionsRow
          label="links out"
          items={(detail.links_out ?? []).map((l) => ({
            id: l.page_id,
            title: l.title || l.page_id,
            relation: l.relation,
          }))}
          onJump={onJump}
          onUnlink={async (target) => {
            await unlinkWikiPages(detail.id, target)
            await onLinked()
          }}
        />
        <ConnectionsRow
          label="referenced by"
          items={(detail.links_in ?? []).map((l) => ({
            id: l.page_id,
            title: l.title || l.page_id,
            relation: l.relation,
          }))}
          onJump={onJump}
        />
        {suggestions.length > 0 && (
          <div>
            <div className="text-xs uppercase tracking-wider text-muted-foreground mb-2">
              suggested (vector similarity)
            </div>
            <div className="flex flex-wrap gap-2">
              {suggestions.map((s) => (
                <div
                  key={s.page_id}
                  className="flex items-center gap-1 bg-muted/40 rounded-full pl-3 pr-1 py-0.5"
                >
                  <button
                    onClick={() => onJump(s.page_id)}
                    className="text-xs hover:text-primary"
                    title={`similarity ${s.score.toFixed(2)}`}
                  >
                    {s.title || s.page_id}{" "}
                    <span className="text-muted-foreground/60">
                      {(s.score * 100).toFixed(0)}%
                    </span>
                  </button>
                  <button
                    title="link"
                    onClick={async () => {
                      try {
                        await linkWikiPages(detail.id, s.page_id, "related")
                        toast.success("linked")
                        await onLinked()
                      } catch (e) {
                        toast.error(String((e as Error).message ?? e))
                      }
                    }}
                    className="text-xs text-primary hover:bg-primary/15 rounded-full w-5 h-5 inline-flex items-center justify-center"
                  >
                    +
                  </button>
                </div>
              ))}
            </div>
          </div>
        )}
        {(detail.links_out?.length ?? 0) === 0 &&
          (detail.links_in?.length ?? 0) === 0 &&
          suggestions.length === 0 && (
            <div className="text-xs text-muted-foreground italic">
              no connections yet — write more pages, vector engine will start
              suggesting links automatically.
            </div>
          )}
      </CardContent>
    </Card>
  )
}

function ConnectionsRow({
  label,
  items,
  onJump,
  onUnlink,
}: {
  label: string
  items: Array<{ id: string; title: string; relation?: string }>
  onJump: (id: string) => void
  onUnlink?: (target: string) => Promise<void> | void
}) {
  if (items.length === 0) return null
  return (
    <div>
      <div className="text-xs uppercase tracking-wider text-muted-foreground mb-1">
        {label}
      </div>
      <div className="flex flex-wrap gap-2">
        {items.map((it) => (
          <div
            key={it.id + (it.relation ?? "")}
            className="flex items-center gap-1 bg-muted/30 rounded-full pl-3 pr-1 py-0.5"
          >
            <button
              onClick={() => onJump(it.id)}
              className="text-xs hover:text-primary"
            >
              {it.title}
              {it.relation && it.relation !== "related" && (
                <span className="text-muted-foreground/60">
                  {" "}
                  · {it.relation}
                </span>
              )}
            </button>
            {onUnlink && (
              <button
                title="unlink"
                onClick={async () => {
                  await onUnlink(it.id)
                }}
                className="text-xs text-muted-foreground hover:text-red-400 rounded-full w-5 h-5 inline-flex items-center justify-center"
              >
                ×
              </button>
            )}
          </div>
        ))}
      </div>
    </div>
  )
}

// WikiEditor — textarea with `[[…` typeahead. When the user types
// `[[` (or text inside an unclosed `[[`), we surface a dropdown of
// matching pages by title. Tab / click closes with `]]` appended.
//
// Implementation: track caret position, find last `[[` before caret
// without a closing `]]` in between. Filter pages by the substring
// after `[[`. Keyboard nav (↑↓ + Enter) handled inside.
function WikiEditor({
  value,
  pages,
  onChange,
}: {
  value: string
  pages: WikiPageLite[]
  onChange: (v: string) => void
}) {
  const ref = useRef<HTMLTextAreaElement | null>(null)
  const [suggest, setSuggest] = useState<{
    open: boolean
    query: string
    start: number // caret position of "[[" inside value
    sel: number
  }>({ open: false, query: "", start: 0, sel: 0 })

  const computeSuggest = (text: string, caret: number) => {
    // Walk backward from caret; close on `]]` or whitespace gap.
    const head = text.slice(0, caret)
    const m = /\[\[([^\]\n]*)$/.exec(head)
    if (!m) {
      setSuggest((s) => ({ ...s, open: false }))
      return
    }
    setSuggest({ open: true, query: m[1], start: m.index, sel: 0 })
  }

  const onKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (!suggest.open) return
    if (e.key === "Escape") {
      e.preventDefault()
      setSuggest((s) => ({ ...s, open: false }))
      return
    }
    if (e.key === "ArrowDown") {
      e.preventDefault()
      setSuggest((s) => ({
        ...s,
        sel: Math.min(filtered.length - 1, s.sel + 1),
      }))
      return
    }
    if (e.key === "ArrowUp") {
      e.preventDefault()
      setSuggest((s) => ({ ...s, sel: Math.max(0, s.sel - 1) }))
      return
    }
    if ((e.key === "Enter" || e.key === "Tab") && filtered.length > 0) {
      e.preventDefault()
      pick(filtered[suggest.sel])
    }
  }

  const filtered = useMemo(() => {
    if (!suggest.open) return []
    const q = suggest.query.toLowerCase()
    const all = pages.filter(
      (p) =>
        p.title.toLowerCase().includes(q) ||
        p.id.toLowerCase().includes(q),
    )
    return all.slice(0, 8)
  }, [pages, suggest])

  const pick = (page: WikiPageLite) => {
    if (!ref.current) return
    const ta = ref.current
    const before = value.slice(0, suggest.start)
    const after = value.slice(ta.selectionStart)
    const insertion = `[[${page.title || page.id}]]`
    const next = before + insertion + after
    onChange(next)
    setSuggest((s) => ({ ...s, open: false }))
    // Restore caret after the inserted token.
    setTimeout(() => {
      const pos = before.length + insertion.length
      ta.selectionStart = ta.selectionEnd = pos
      ta.focus()
    }, 0)
  }

  return (
    <div className="relative">
      <textarea
        ref={ref}
        value={value}
        onChange={(e) => {
          onChange(e.target.value)
          computeSuggest(e.target.value, e.target.selectionStart)
        }}
        onKeyDown={onKeyDown}
        onClick={(e) =>
          computeSuggest(value, e.currentTarget.selectionStart)
        }
        className="w-full h-[60vh] bg-muted/30 rounded-md p-4 font-mono text-sm leading-relaxed border border-border"
      />
      {suggest.open && filtered.length > 0 && (
        <div className="absolute left-4 top-12 z-10 w-72 max-h-72 overflow-y-auto rounded-md border border-border bg-popover shadow-lg">
          <div className="px-3 py-1.5 text-xs text-muted-foreground border-b border-border">
            insert wikilink — ↑↓ + Enter
          </div>
          {filtered.map((p, i) => (
            <button
              key={p.id}
              onMouseDown={(e) => {
                e.preventDefault()
                pick(p)
              }}
              className={
                "block w-full text-left px-3 py-1.5 text-sm transition-colors " +
                (i === suggest.sel
                  ? "bg-primary/15 text-foreground"
                  : "text-muted-foreground hover:bg-muted hover:text-foreground")
              }
            >
              <div className="font-medium text-foreground line-clamp-1">
                {p.title || p.id}
              </div>
              <div className="text-xs text-muted-foreground/70 font-mono">
                {p.id}
              </div>
            </button>
          ))}
        </div>
      )}
    </div>
  )
}

// Suppress unused warnings — Eye is reserved for future "preview while
// editing" mode I haven't shipped.
const _unused = { Eye }
const _ref: typeof useRef = useRef
void _unused
void _ref
