import { useState, type ReactNode } from "react"
import { Code2, Copy, Loader2, RefreshCw, ShieldAlert } from "lucide-react"
import { toast } from "sonner"
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { Button } from "@/components/ui/button"
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import { projectConnect, type ProjectConnect } from "@/lib/api"

function copy(text: string, label: string) {
  void navigator.clipboard.writeText(text)
  toast.success(`Copied ${label}`)
}

function CodeBlock({ children }: { children: string }) {
  return (
    <pre className="overflow-x-auto rounded-md bg-muted/60 p-3 font-mono text-xs leading-relaxed whitespace-pre-wrap break-all">
      {children}
    </pre>
  )
}

function ToolCard({
  title,
  hint,
  code,
  copyLabel,
  children,
}: {
  title: string
  hint?: string
  code?: string
  copyLabel?: string
  children?: ReactNode
}) {
  return (
    <Card>
      <CardHeader className="pb-3">
        <div className="flex items-center justify-between gap-3">
          <CardTitle className="text-base">{title}</CardTitle>
          {code ? (
            <Button variant="outline" size="sm" onClick={() => copy(code, copyLabel ?? title)}>
              <Copy className="size-3.5" /> Copy
            </Button>
          ) : null}
        </div>
        {hint ? <CardDescription>{hint}</CardDescription> : null}
      </CardHeader>
      <CardContent className="space-y-3">
        {code ? <CodeBlock>{code}</CodeBlock> : null}
        {children}
      </CardContent>
    </Card>
  )
}

function Field({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex items-center gap-2">
      <span className="w-24 shrink-0 text-muted-foreground text-sm">{label}</span>
      <code className="flex-1 truncate rounded bg-muted/60 px-2 py-1 font-mono text-xs">{value}</code>
      <Button variant="ghost" size="icon-xs" onClick={() => copy(value, label)}>
        <Copy className="size-3.5" />
      </Button>
    </div>
  )
}

export function CodingToolsPage() {
  const [data, setData] = useState<ProjectConnect | null>(null)
  const [busy, setBusy] = useState(false)
  const [err, setErr] = useState<string | null>(null)

  const generate = async () => {
    setBusy(true)
    setErr(null)
    try {
      const d = await projectConnect()
      setData(d)
      toast.success("Connection token generated")
    } catch (e) {
      setErr(String((e as Error).message ?? e))
    } finally {
      setBusy(false)
    }
  }

  const headerName = data?.auth_header?.name ?? "Authorization"
  const headerValue = data?.auth_header?.value ?? ""

  return (
    <div className="space-y-6">
      <div>
        <h1 className="flex items-center gap-2 text-2xl font-semibold tracking-tight">
          <Code2 className="size-6" /> Connect your coding tool
        </h1>
        <p className="mt-1 max-w-2xl text-muted-foreground">
          Let your coding agent (Claude Code, Codex, Cursor) push project status,
          decisions and docs to your synth over MCP, so it stays in the loop on
          what you are building. The synth never reaches into your machine: your
          agent pushes, the synth receives.
        </p>
      </div>

      {!data ? (
        <Card>
          <CardContent className="flex flex-col items-center gap-4 py-10 text-center">
            <p className="max-w-md text-muted-foreground">
              Generate a connection. You will get a private MCP URL plus a
              ready-to-paste setup for each tool.
            </p>
            <Button onClick={generate} disabled={busy} size="lg">
              {busy ? <Loader2 className="size-4 animate-spin" /> : <Code2 className="size-4" />}
              Generate connection
            </Button>
            {err ? <p className="text-sm text-destructive">{err}</p> : null}
          </CardContent>
        </Card>
      ) : (
        <div className="space-y-5">
          <Alert>
            <ShieldAlert className="size-4" />
            <AlertTitle>This token is a secret</AlertTitle>
            <AlertDescription>
              Anyone holding it can read and write your synth&apos;s project
              memory. Paste it into your coding tool only. Generate a new one
              anytime to rotate it.
            </AlertDescription>
          </Alert>

          <ToolCard
            title="MCP endpoint"
            hint="Transport: Streamable HTTP"
            code={data.mcp_url}
            copyLabel="URL"
          />

          <ToolCard
            title="Claude Code"
            hint="Run this in your terminal, then check with: claude mcp list"
            code={data.commands.claude}
            copyLabel="Claude command"
          />

          <ToolCard title="Codex app">
            <p className="text-sm text-muted-foreground">
              Settings &rarr; MCP servers &rarr; Add server &rarr; Streamable
              HTTP. Use a static <b>Header</b>, not the &quot;Bearer token env
              var&quot; field.
            </p>
            <div className="grid gap-2">
              <Field label="URL" value={data.app_setup.url} />
              <Field label="Header name" value={headerName} />
              <Field label="Header value" value={headerValue} />
            </div>
            {data.app_setup.note ? (
              <Alert>
                <ShieldAlert className="size-4" />
                <AlertDescription>{data.app_setup.note}</AlertDescription>
              </Alert>
            ) : null}
          </ToolCard>

          <ToolCard
            title="Codex CLI"
            hint="The CLI reads the token from an env var (a GUI app cannot, hence the header above)."
            code={data.commands.codex_cli}
            copyLabel="Codex CLI command"
          />

          <ToolCard
            title="Cursor"
            hint="Add to ~/.cursor/mcp.json (global) or .cursor/mcp.json (per project)."
            code={data.commands.cursor}
            copyLabel="Cursor config"
          />

          <div>
            <Button variant="outline" onClick={generate} disabled={busy}>
              {busy ? <Loader2 className="size-4 animate-spin" /> : <RefreshCw className="size-4" />}
              Generate a new token
            </Button>
            {err ? <p className="mt-2 text-sm text-destructive">{err}</p> : null}
          </div>
        </div>
      )}
    </div>
  )
}
