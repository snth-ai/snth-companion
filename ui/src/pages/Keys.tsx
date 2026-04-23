import { useEffect, useMemo, useState } from "react"
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import { Badge } from "@/components/ui/badge"
import {
  fetchLLMConfig,
  fetchProviderCatalog,
  uploadProviderKey,
  type LLMConfig,
  type ProviderCatalogEntry,
} from "@/lib/api"

export function KeysPage() {
  const [catalog, setCatalog] = useState<ProviderCatalogEntry[] | null>(null)
  const [cfg, setCfg] = useState<LLMConfig | null>(null)
  const [err, setErr] = useState<string | null>(null)
  const [ok, setOk] = useState<string | null>(null)

  const [provider, setProvider] = useState("openrouter")
  const [model, setModel] = useState("")
  const [apiKey, setApiKey] = useState("")
  const [busy, setBusy] = useState(false)

  const reload = async () => {
    setErr(null)
    try {
      const [c, l] = await Promise.all([fetchProviderCatalog(), fetchLLMConfig()])
      setCatalog(c)
      setCfg(l)
      if (!provider && c.length > 0) setProvider(c[0].provider)
    } catch (e) {
      setErr(String(e))
    }
  }

  useEffect(() => {
    reload()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  const selected = useMemo(
    () => catalog?.find((p) => p.provider === provider),
    [catalog, provider],
  )

  if (err && !catalog) {
    return (
      <Alert variant="destructive">
        <AlertTitle>Can't load</AlertTitle>
        <AlertDescription>{err}</AlertDescription>
      </Alert>
    )
  }
  if (!catalog || !cfg) {
    return <div className="text-sm text-muted-foreground">Loading…</div>
  }

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-2xl font-semibold tracking-tight">API Keys</h1>
        <p className="text-sm text-muted-foreground mt-1">
          Upload your own provider API key. Validated against the
          provider's <code className="font-mono">/models</code> before
          store. Shared utility keys (Gemini vision, etc.) keep
          working; only the primary provider slot gets your key.
        </p>
      </div>

      {err && (
        <Alert variant="destructive">
          <AlertTitle>Error</AlertTitle>
          <AlertDescription>{err}</AlertDescription>
        </Alert>
      )}
      {ok && (
        <Alert>
          <AlertTitle>Done</AlertTitle>
          <AlertDescription>{ok}</AlertDescription>
        </Alert>
      )}

      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-3">
            Now using
            {cfg.primary?.is_user_uploaded ? (
              <Badge className="gap-1">your key</Badge>
            ) : cfg.primary ? (
              <Badge variant="secondary">shared / operator</Badge>
            ) : null}
          </CardTitle>
        </CardHeader>
        <CardContent className="space-y-2 text-sm">
          {cfg.primary ? (
            <>
              <Row label="Provider" mono value={cfg.primary.provider} />
              <Row label="Model" mono value={cfg.primary.model} />
              <Row label="Key label" mono value={cfg.primary.key_label} />
            </>
          ) : (
            <p className="text-muted-foreground">No primary assignment.</p>
          )}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>Upload your API key</CardTitle>
        </CardHeader>
        <CardContent className="space-y-4">
          <div>
            <Label>Provider</Label>
            <Select value={provider} onValueChange={setProvider}>
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {catalog.map((p) => (
                  <SelectItem key={p.provider} value={p.provider}>
                    {p.display}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            {selected && (
              <p className="text-xs text-muted-foreground mt-2">
                {selected.hint}{" "}
                <a
                  href={selected.docs_url}
                  target="_blank"
                  rel="noopener"
                  className="text-primary hover:underline"
                >
                  Docs →
                </a>
              </p>
            )}
          </div>
          <div>
            <Label>Model id</Label>
            <Input
              value={model}
              onChange={(e) => setModel(e.target.value)}
              placeholder={selected?.example_model ?? ""}
              className="font-mono text-xs"
            />
            <p className="text-xs text-muted-foreground mt-1">
              Free-form — whatever id the provider docs show.
            </p>
          </div>
          <div>
            <Label>API key</Label>
            <Input
              type="password"
              value={apiKey}
              onChange={(e) => setApiKey(e.target.value)}
              placeholder="sk-..."
              autoComplete="off"
              className="font-mono text-xs"
            />
          </div>
          <Button
            disabled={busy || !provider || !apiKey || !model}
            onClick={async () => {
              setBusy(true)
              setErr(null)
              setOk(null)
              try {
                const r = await uploadProviderKey(provider, apiKey, model)
                setOk(
                  `Validated + assigned. Container recreate: ${r.applied ? "success" : "see logs"}.`,
                )
                setApiKey("")
                await reload()
              } catch (e) {
                setErr(String(e))
              } finally {
                setBusy(false)
              }
            }}
          >
            {busy ? "Validating…" : "Validate & apply"}
          </Button>
        </CardContent>
      </Card>
    </div>
  )
}

function Row({ label, value, mono }: { label: string; value: string; mono?: boolean }) {
  return (
    <div className="flex items-center justify-between border-b border-border/50 pb-2 last:border-0 last:pb-0">
      <span className="text-muted-foreground">{label}</span>
      <span className={mono ? "font-mono text-xs" : undefined}>{value}</span>
    </div>
  )
}
