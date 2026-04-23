import { useEffect, useState } from "react"
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { Button } from "@/components/ui/button"
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import { Badge } from "@/components/ui/badge"
import {
  codexLoginClear,
  codexLoginStart,
  codexLoginState,
  codexLoginUpload,
  type CodexLoginState,
} from "@/lib/api"

export function CodexLoginPage() {
  const [state, setState] = useState<CodexLoginState | null>(null)
  const [err, setErr] = useState<string | null>(null)

  const poll = async () => {
    try {
      setState(await codexLoginState())
    } catch (e) {
      setErr(String(e))
    }
  }

  useEffect(() => {
    poll()
    const t = setInterval(poll, 2000)
    return () => clearInterval(t)
  }, [])

  if (!state) {
    return <div className="text-sm text-muted-foreground">Loading…</div>
  }

  const waiting = state.has_flow && !state.done
  const success = state.has_result && !!state.uploaded_at
  const uploadFailed = state.has_result && !!state.upload_err
  const completed = state.done && !state.has_result && state.err

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-2xl font-semibold tracking-tight">
          Codex Login (ChatGPT Plus/Pro)
        </h1>
        <p className="text-sm text-muted-foreground mt-1">
          Runs OAuth against <code className="font-mono">auth.openai.com</code>{" "}
          (same flow as Codex CLI). Tokens upload to the hub vault so the
          paired synth uses your subscription — no API-key required.
        </p>
      </div>

      {err && (
        <Alert variant="destructive">
          <AlertTitle>Error</AlertTitle>
          <AlertDescription>{err}</AlertDescription>
        </Alert>
      )}

      {success && (
        <Alert>
          <AlertTitle className="flex items-center gap-2">
            ✓ Authenticated & uploaded
            <Badge className="text-[10px]">hub vault</Badge>
          </AlertTitle>
          <AlertDescription>
            Account <code className="font-mono">{state.account_id}</code>{" "}
            at {new Date(state.uploaded_at as string).toLocaleString()}.
          </AlertDescription>
        </Alert>
      )}
      {uploadFailed && (
        <Alert variant="destructive">
          <AlertTitle>OAuth OK but upload failed</AlertTitle>
          <AlertDescription className="space-y-2">
            <div className="font-mono text-xs">{state.upload_err}</div>
            <Button
              size="sm"
              onClick={async () => {
                await codexLoginUpload()
                poll()
              }}
            >
              Retry upload
            </Button>
          </AlertDescription>
        </Alert>
      )}
      {completed && state.err ? (
        <Alert variant="destructive">
          <AlertTitle>Login failed</AlertTitle>
          <AlertDescription className="font-mono text-xs">
            {state.err}
          </AlertDescription>
        </Alert>
      ) : null}

      {waiting && (
        <Card>
          <CardHeader>
            <CardTitle>Waiting for callback…</CardTitle>
          </CardHeader>
          <CardContent className="space-y-3 text-sm">
            <p className="text-muted-foreground">
              The browser should have opened to OpenAI. If not, paste this
              URL manually:
            </p>
            <p>
              <a
                className="text-primary hover:underline font-mono text-xs break-all"
                href={state.auth_url}
                target="_blank"
                rel="noopener"
              >
                {state.auth_url}
              </a>
            </p>
            <Button
              variant="secondary"
              size="sm"
              onClick={async () => {
                await codexLoginClear()
                poll()
              }}
            >
              Cancel
            </Button>
          </CardContent>
        </Card>
      )}

      <Card>
        <CardHeader>
          <CardTitle>{success ? "Re-login" : "Login"}</CardTitle>
        </CardHeader>
        <CardContent className="space-y-3 text-sm">
          <p className="text-muted-foreground">
            Opens <code className="font-mono">auth.openai.com</code> in
            your default browser. After sign-in, the callback lands on
            <code className="font-mono"> localhost:1455</code> and tokens
            auto-upload to the hub.
          </p>
          <div className="flex gap-2">
            <Button
              disabled={waiting}
              onClick={async () => {
                await codexLoginStart()
                poll()
              }}
            >
              {success ? "Re-login with OpenAI" : "Login with OpenAI →"}
            </Button>
            {(state.has_result || state.err) && !waiting && (
              <Button
                variant="secondary"
                onClick={async () => {
                  await codexLoginClear()
                  poll()
                }}
              >
                Clear state
              </Button>
            )}
          </div>
        </CardContent>
      </Card>
    </div>
  )
}
