import { useEffect, useMemo, useState } from "react"
import {
  ArrowDown,
  ArrowUp,
  Brain,
  FolderGit2,
  Library,
  Loader2,
  RefreshCw,
  Trash2,
} from "lucide-react"
import { toast } from "sonner"
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { Button } from "@/components/ui/button"
import { Badge } from "@/components/ui/badge"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import {
  fetchProjects,
  projectRecall,
  projectLibraryList,
  projectLibraryDelete,
  projectForget,
  type Project,
  type LibDoc,
  type LibList,
  type ProjectRecall,
} from "@/lib/api"

function fmtBytes(n: number): string {
  if (n < 1024) return `${n} B`
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`
  return `${(n / 1024 / 1024).toFixed(1)} MB`
}

function fmtWhen(s: string | null | undefined): string {
  if (!s) return "never"
  const t = new Date(s).getTime()
  if (!t) return "never"
  const days = Math.floor((Date.now() - t) / 86400000)
  if (days <= 0) return "today"
  if (days === 1) return "1d ago"
  if (days < 30) return `${days}d ago`
  if (days < 365) return `${Math.floor(days / 30)}mo ago`
  return `${Math.floor(days / 365)}y ago`
}

type SortKey = "path" | "bytes" | "updated_at" | "accessed_at" | "access_count"

export function ProjectsV2Page() {
  const [projects, setProjects] = useState<Project[]>([])
  const [slug, setSlug] = useState<string>("")
  const [recall, setRecall] = useState<ProjectRecall | null>(null)
  const [lib, setLib] = useState<LibList | null>(null)
  const [busy, setBusy] = useState(false)
  const [err, setErr] = useState<string | null>(null)
  const [sort, setSort] = useState<{ key: SortKey; dir: "asc" | "desc" }>({
    key: "updated_at",
    dir: "desc",
  })

  useEffect(() => {
    fetchProjects()
      .then((r) => {
        setProjects(r.projects ?? [])
        if (!slug && r.projects?.length) setSlug(r.projects[0].slug)
      })
      .catch((e) => setErr(String(e)))
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  const load = async (s: string) => {
    if (!s) return
    setBusy(true)
    setErr(null)
    try {
      const [rc, ll] = await Promise.all([
        projectRecall(s).catch(() => null),
        projectLibraryList(s),
      ])
      setRecall(rc)
      setLib(ll)
    } catch (e) {
      setErr(String((e as Error).message ?? e))
    } finally {
      setBusy(false)
    }
  }

  useEffect(() => {
    if (slug) void load(slug)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [slug])

  const sortedDocs = useMemo(() => {
    const docs = [...(lib?.docs ?? [])]
    docs.sort((a, b) => {
      let av: number | string
      let bv: number | string
      switch (sort.key) {
        case "path":
          av = a.title || a.path
          bv = b.title || b.path
          break
        case "bytes":
          av = a.bytes
          bv = b.bytes
          break
        case "access_count":
          av = a.access_count
          bv = b.access_count
          break
        case "accessed_at":
          av = a.accessed_at ? new Date(a.accessed_at).getTime() : 0
          bv = b.accessed_at ? new Date(b.accessed_at).getTime() : 0
          break
        default:
          av = new Date(a.updated_at).getTime()
          bv = new Date(b.updated_at).getTime()
      }
      const cmp = av < bv ? -1 : av > bv ? 1 : 0
      return sort.dir === "asc" ? cmp : -cmp
    })
    return docs
  }, [lib, sort])

  const toggleSort = (key: SortKey) =>
    setSort((s) =>
      s.key === key ? { key, dir: s.dir === "asc" ? "desc" : "asc" } : { key, dir: "desc" },
    )

  const del = async (d: LibDoc) => {
    if (!confirm(`Delete library doc "${d.title || d.path}"? This cannot be undone.`)) return
    try {
      await projectLibraryDelete(slug, d.path)
      toast.success(`Deleted ${d.path}`)
      await load(slug)
    } catch (e) {
      toast.error(String((e as Error).message ?? e))
    }
  }

  const forget = async () => {
    if (!confirm(`Forget EVERYTHING for project "${slug}" (memory card + library)? This cannot be undone.`)) return
    try {
      await projectForget(slug, true)
      toast.success(`Forgot project ${slug}`)
      await load(slug)
    } catch (e) {
      toast.error(String((e as Error).message ?? e))
    }
  }

  const SortHead = ({ k, label, right }: { k: SortKey; label: string; right?: boolean }) => (
    <button
      onClick={() => toggleSort(k)}
      className={`flex items-center gap-1 hover:text-foreground ${right ? "ml-auto" : ""}`}
    >
      {label}
      {sort.key === k ? (
        sort.dir === "asc" ? <ArrowUp className="size-3" /> : <ArrowDown className="size-3" />
      ) : null}
    </button>
  )

  return (
    <div className="space-y-6">
      <div className="flex flex-wrap items-end justify-between gap-3">
        <div>
          <h1 className="flex items-center gap-2 text-2xl font-semibold tracking-tight">
            <FolderGit2 className="size-6" /> Projects
          </h1>
          <p className="mt-1 max-w-2xl text-muted-foreground">
            What your synth remembers per project, and the heavy library it can
            reach on demand. Prune what you do not need: the library lives on the
            synth, so old or never-opened docs are just weight.
          </p>
        </div>
        <div className="flex items-center gap-2">
          <Select value={slug} onValueChange={setSlug}>
            <SelectTrigger className="w-64">
              <SelectValue placeholder="Pick a project" />
            </SelectTrigger>
            <SelectContent>
              {projects.map((p) => (
                <SelectItem key={p.id} value={p.slug}>
                  {p.name || p.slug}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <Button variant="outline" size="icon" onClick={() => load(slug)} disabled={busy || !slug}>
            {busy ? <Loader2 className="size-4 animate-spin" /> : <RefreshCw className="size-4" />}
          </Button>
        </div>
      </div>

      {err ? <p className="text-sm text-destructive">{err}</p> : null}
      {!projects.length ? (
        <Card>
          <CardContent className="py-10 text-center text-muted-foreground">
            No projects yet. Connect a coding tool and push a checkpoint to create one.
          </CardContent>
        </Card>
      ) : null}

      {slug ? (
        <>
          <Card>
            <CardHeader className="pb-3">
              <CardTitle className="flex items-center gap-2 text-base">
                <Brain className="size-4" /> In memory
                <span className="text-sm font-normal text-muted-foreground">
                  always recalled (the Tier-1 card)
                </span>
              </CardTitle>
            </CardHeader>
            <CardContent>
              {recall && recall.context?.trim() ? (
                <pre className="overflow-x-auto rounded-md bg-muted/60 p-3 font-mono text-xs leading-relaxed whitespace-pre-wrap">
                  {recall.context}
                </pre>
              ) : (
                <p className="text-sm text-muted-foreground">No memory card for this project yet.</p>
              )}
            </CardContent>
          </Card>

          <Card>
            <CardHeader className="pb-3">
              <div className="flex flex-wrap items-center justify-between gap-2">
                <CardTitle className="flex items-center gap-2 text-base">
                  <Library className="size-4" /> Library
                  <span className="text-sm font-normal text-muted-foreground">
                    reached on demand (Tier-2, not in recall)
                  </span>
                </CardTitle>
                <div className="flex items-center gap-2">
                  <Badge variant="secondary">
                    {lib?.count ?? 0} docs · {fmtBytes(lib?.total_bytes ?? 0)}
                  </Badge>
                  <Button variant="ghost" size="sm" className="text-destructive" onClick={forget} disabled={!slug}>
                    <Trash2 className="size-3.5" /> Forget project
                  </Button>
                </div>
              </div>
            </CardHeader>
            <CardContent>
              {sortedDocs.length ? (
                <div className="overflow-x-auto">
                  <table className="w-full text-sm">
                    <thead className="text-xs text-muted-foreground">
                      <tr className="border-b">
                        <th className="py-2 text-left font-normal"><SortHead k="path" label="Doc" /></th>
                        <th className="py-2 text-right font-normal"><SortHead k="bytes" label="Size" right /></th>
                        <th className="py-2 text-right font-normal"><SortHead k="updated_at" label="Updated" right /></th>
                        <th className="py-2 text-right font-normal"><SortHead k="accessed_at" label="Last opened" right /></th>
                        <th className="py-2 text-right font-normal"><SortHead k="access_count" label="Opens" right /></th>
                        <th className="py-2"></th>
                      </tr>
                    </thead>
                    <tbody>
                      {sortedDocs.map((d) => (
                        <tr key={d.id} className="border-b border-border/40 last:border-0">
                          <td className="py-2 pr-3">
                            <div className="font-medium">{d.title || d.path}</div>
                            {d.title ? <div className="text-xs text-muted-foreground">{d.path}</div> : null}
                          </td>
                          <td className="py-2 text-right tabular-nums">{fmtBytes(d.bytes)}</td>
                          <td className="py-2 text-right tabular-nums text-muted-foreground">{fmtWhen(d.updated_at)}</td>
                          <td className={`py-2 text-right tabular-nums ${d.accessed_at ? "text-muted-foreground" : "text-amber-500"}`}>
                            {fmtWhen(d.accessed_at)}
                          </td>
                          <td className="py-2 text-right tabular-nums text-muted-foreground">{d.access_count}</td>
                          <td className="py-2 text-right">
                            <Button variant="ghost" size="icon-xs" className="text-destructive" onClick={() => del(d)}>
                              <Trash2 className="size-3.5" />
                            </Button>
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              ) : (
                <p className="text-sm text-muted-foreground">
                  No library docs. The card above is all this project keeps on the synth.
                </p>
              )}
            </CardContent>
          </Card>
        </>
      ) : null}
    </div>
  )
}
