package daemon

// codex_login.go — UI wrapper around the codexlogin package. Shows a
// "Login with Codex" button; when clicked, starts the OAuth flow,
// opens the system browser to OpenAI, polls for the callback, then
// renders the resulting credential JSON for the user to paste into
// the hub's /keys form. Auto-upload to hub vault is future scope
// (needs a non-admin endpoint keyed to the pair token).

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"strings"
	"sync"
	"time"

	"github.com/snth-ai/snth-companion/internal/codexlogin"
	"github.com/snth-ai/snth-companion/internal/config"
)

// codexLoginState tracks a single in-flight login for the UI. The
// companion is a per-user sidecar so one slot is enough; a second
// Start wipes the previous one.
type codexLoginState struct {
	mu         sync.Mutex
	flow       *codexlogin.Flow
	authURL    string // copied out of flow for UI even after flow finishes
	result     *codexlogin.Output
	err        string
	done       bool
	started    time.Time
	uploadedAt time.Time // set when the blob has been pushed to the hub vault
	uploadErr  string    // last hub-upload error, if any
}

var codexLogin codexLoginState

func (s *UIServer) handleCodexLoginPage(w http.ResponseWriter, r *http.Request) {
	codexLogin.mu.Lock()
	authURL := codexLogin.authURL
	result := codexLogin.result
	errMsg := codexLogin.err
	done := codexLogin.done
	started := codexLogin.started
	uploadedAt := codexLogin.uploadedAt
	uploadErr := codexLogin.uploadErr
	codexLogin.mu.Unlock()

	cfg := config.Get()
	hubURL := ""
	if cfg != nil {
		hubURL = cfg.HubURL
	}
	if hubURL == "" {
		hubURL = "https://hub.snth.ai"
	}
	hubKeysURL := hubURL + "/keys"

	intro := `
<div class="card">
  <h2>Login with OpenAI (Codex / ChatGPT subscription)</h2>
  <p style="color:#94a3b8;font-size:13px">
    Runs the same OAuth flow as <code>codex-login</code> CLI. Your browser
    opens to sign in with ChatGPT; when you authorize, the callback lands
    back here and you get a credential JSON blob ready to paste into the
    hub's <a href="` + hubKeysURL + `" target="_blank" rel="noopener">/keys</a>
    form as <code>oauth_subscription</code>.
  </p>
  <form method="POST" action="/login/codex/start" style="margin-top:14px">
    <button type="submit">Login with OpenAI →</button>
    <span style="margin-left:12px;color:#64748b;font-size:12px">
      Opens <code>auth.openai.com</code> in your default browser.
    </span>
  </form>
</div>`

	switch {
	case result != nil:
		blob, _ := json.MarshalIndent(result, "", "  ")

		// Upload status block — explains whether the auto-upload to
		// hub vault succeeded. Falls back to the manual "paste into
		// /keys" flow if upload failed.
		upBlock := ""
		switch {
		case !uploadedAt.IsZero():
			upBlock = fmt.Sprintf(`
<div class="card" style="border-color:#22c55e">
  <h2>✓ Saved to hub vault</h2>
  <p style="color:#94a3b8;font-size:13px">
    Tokens uploaded via <code>/api/my/codex-creds</code> at <code>%s</code>.
    The paired synth is now using this Codex subscription. No manual
    paste needed — but the JSON below is kept for reference.
  </p>
</div>`, uploadedAt.Format("15:04:05"))
		case uploadErr != "":
			upBlock = fmt.Sprintf(`
<div class="card" style="border-color:#f59e0b">
  <h2>⚠ Auto-upload failed</h2>
  <p class="mono" style="color:#fca5a5;font-size:12px">%s</p>
  <p style="color:#94a3b8;font-size:13px;margin-top:10px">
    Tokens are still on this machine (below). Either retry the upload
    or paste the JSON into the hub's <a href="%s" target="_blank" rel="noopener">/keys</a>
    form manually.
  </p>
  <form method="POST" action="/login/codex/upload" style="margin-top:12px">
    <button type="submit">Retry upload</button>
  </form>
</div>`, htmlEscape(uploadErr), hubKeysURL)
		default:
			upBlock = `
<div class="card" style="border-color:#f59e0b">
  <h2>Uploading to hub vault…</h2>
  <p style="color:#94a3b8;font-size:13px">
    Pushing to <code>/api/my/codex-creds</code>. Refresh in a second.
  </p>
  <script>setTimeout(function(){location.reload();},1200);</script>
</div>`
		}

		body := fmt.Sprintf(`
%s
<div class="card" style="border-color:#22c55e">
  <h2>✓ Authenticated</h2>
  <p style="color:#94a3b8;font-size:13px">
    Credential JSON (kept locally for reference or manual paste into hub
    <a href="%s" target="_blank" rel="noopener">/keys</a> if the auto-upload fails):
  </p>
  <label>Credential JSON</label>
  <textarea id="blob" rows="10" readonly style="font-family:ui-monospace,Menlo,monospace">%s</textarea>
  <div style="display:flex;gap:10px;margin-top:14px;flex-wrap:wrap">
    <button type="button" onclick="navigator.clipboard.writeText(document.getElementById('blob').value);this.textContent='Copied ✓'">Copy JSON</button>
    <a class="btn" href="%s" target="_blank" rel="noopener">Open hub /keys</a>
    <form method="POST" action="/login/codex/clear" style="display:inline">
      <button class="subtle" type="submit">Start over</button>
    </form>
  </div>
  <p style="color:#64748b;font-size:12px;margin-top:14px">
    account_id: <code>%s</code>
  </p>
</div>%s`, upBlock, hubKeysURL, string(blob), hubKeysURL, result.AccountID, intro)
		s.layout(w, "/login/codex", "Codex Login", body)
		return

	case errMsg != "":
		body := fmt.Sprintf(`
<div class="card" style="border-color:#ef4444;background:#1c1917">
  <h2>✗ Login failed</h2>
  <p class="mono" style="color:#fca5a5">%s</p>
  <form method="POST" action="/login/codex/clear" style="margin-top:14px">
    <button class="danger" type="submit">Dismiss</button>
  </form>
</div>%s`, htmlEscape(errMsg), intro)
		s.layout(w, "/login/codex", "Codex Login", body)
		return

	case authURL != "" && !done:
		elapsed := time.Since(started).Round(time.Second)
		body := fmt.Sprintf(`
<div class="card" style="border-color:#f59e0b">
  <h2>Waiting for browser callback…</h2>
  <p style="color:#94a3b8;font-size:13px">
    If the browser didn't open automatically, open this URL manually:
  </p>
  <p style="margin-top:10px"><a class="btn" href="%s" target="_blank" rel="noopener">Open auth URL</a></p>
  <p style="color:#64748b;font-size:12px;margin-top:14px">
    Elapsed: %s · auto-refreshing every 2s · times out at 5 min.
  </p>
  <form method="POST" action="/login/codex/clear" style="margin-top:14px">
    <button class="subtle" type="submit">Cancel</button>
  </form>
</div>
<script>setTimeout(function(){location.reload();},2000);</script>%s`, authURL, elapsed, intro)
		s.layout(w, "/login/codex", "Codex Login", body)
		return
	}

	s.layout(w, "/login/codex", "Codex Login", intro)
}

func (s *UIServer) handleCodexLoginStart(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "POST only", 405)
		return
	}

	codexLogin.mu.Lock()
	// If a previous flow is in-flight, cancel its listener before starting a new one.
	if codexLogin.flow != nil && !codexLogin.done {
		codexLogin.flow.Cancel()
	}
	codexLogin.mu.Unlock()

	flow, err := codexlogin.Start()
	if err != nil {
		codexLogin.mu.Lock()
		codexLogin.flow = nil
		codexLogin.authURL = ""
		codexLogin.result = nil
		codexLogin.err = "start flow: " + err.Error()
		codexLogin.done = true
		codexLogin.started = time.Time{}
		codexLogin.mu.Unlock()
		http.Redirect(w, r, "/login/codex", http.StatusSeeOther)
		return
	}

	codexLogin.mu.Lock()
	codexLogin.flow = flow
	codexLogin.authURL = flow.AuthURL
	codexLogin.result = nil
	codexLogin.err = ""
	codexLogin.done = false
	codexLogin.started = time.Now()
	codexLogin.mu.Unlock()

	// Open the browser so the user doesn't have to click "Open auth URL".
	openURL(flow.AuthURL)

	// Drive the callback wait off the request goroutine.
	go func() {
		out, err := flow.Finish(5 * time.Minute)
		codexLogin.mu.Lock()
		codexLogin.done = true
		if err != nil {
			codexLogin.err = err.Error()
			codexLogin.mu.Unlock()
			return
		}
		codexLogin.result = out
		codexLogin.mu.Unlock()

		// Fire and forget the hub upload. State is updated from
		// inside uploadCodexToHub.
		uploadCodexToHub()
	}()

	http.Redirect(w, r, "/login/codex", http.StatusSeeOther)
}

// handleCodexLoginUpload is the manual "Retry upload" button. Kicks
// uploadCodexToHub on the same in-memory result and redirects back.
func (s *UIServer) handleCodexLoginUpload(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "POST only", 405)
		return
	}
	codexLogin.mu.Lock()
	hasResult := codexLogin.result != nil
	codexLogin.mu.Unlock()
	if !hasResult {
		http.Error(w, "no credential to upload — run login first", http.StatusBadRequest)
		return
	}
	go uploadCodexToHub()
	http.Redirect(w, r, "/login/codex", http.StatusSeeOther)
}

// uploadCodexToHub POSTs the current result to the hub's
// /api/my/codex-creds. Sets uploadedAt / uploadErr on the global
// codexLogin state; called both from the Start goroutine on OAuth
// success and from the manual retry button.
func uploadCodexToHub() {
	codexLogin.mu.Lock()
	result := codexLogin.result
	codexLogin.mu.Unlock()
	if result == nil {
		return
	}

	cfg := config.Get()
	if cfg == nil || cfg.CompanionToken == "" {
		codexLogin.mu.Lock()
		codexLogin.uploadErr = "companion not paired — no companion token in config"
		codexLogin.mu.Unlock()
		return
	}
	// Fall back to the prod hub when HubURL is empty in legacy configs
	// (paired pre-Wave-7.1 / pre-Wave-9 multi-pair refactor — those
	// configs never persisted hub_url at the top level). The
	// pair-claim endpoint sets it now, but existing pairs may not
	// have it populated.
	hubURL := cfg.HubURL
	if hubURL == "" {
		hubURL = "https://hub.snth.ai"
	}

	payload, _ := json.Marshal(map[string]any{
		"access_token":       result.AccessToken,
		"refresh_token":      result.RefreshToken,
		"expires_at_unix_ms": result.ExpiresAt,
		"account_id":         result.AccountID,
	})
	url := strings.TrimRight(hubURL, "/") + "/api/my/codex-creds"

	req, err := http.NewRequest("POST", url, bytes.NewReader(payload))
	if err != nil {
		codexLogin.mu.Lock()
		codexLogin.uploadErr = "build request: " + err.Error()
		codexLogin.mu.Unlock()
		return
	}
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("Authorization", "Bearer "+cfg.CompanionToken)

	client := &http.Client{Timeout: 30 * time.Second}
	resp, err := client.Do(req)
	if err != nil {
		codexLogin.mu.Lock()
		codexLogin.uploadErr = "hub unreachable: " + err.Error()
		codexLogin.mu.Unlock()
		return
	}
	defer resp.Body.Close()
	body, _ := io.ReadAll(resp.Body)
	if resp.StatusCode/100 != 2 {
		codexLogin.mu.Lock()
		codexLogin.uploadErr = fmt.Sprintf("hub returned %d: %s", resp.StatusCode, string(body))
		codexLogin.mu.Unlock()
		return
	}

	codexLogin.mu.Lock()
	codexLogin.uploadedAt = time.Now()
	codexLogin.uploadErr = ""
	codexLogin.mu.Unlock()
}

func (s *UIServer) handleCodexLoginClear(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "POST only", 405)
		return
	}
	codexLogin.mu.Lock()
	if codexLogin.flow != nil && !codexLogin.done {
		codexLogin.flow.Cancel()
	}
	codexLogin.flow = nil
	codexLogin.authURL = ""
	codexLogin.result = nil
	codexLogin.err = ""
	codexLogin.done = false
	codexLogin.started = time.Time{}
	codexLogin.uploadedAt = time.Time{}
	codexLogin.uploadErr = ""
	codexLogin.mu.Unlock()
	http.Redirect(w, r, "/login/codex", http.StatusSeeOther)
}

