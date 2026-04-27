package daemon

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"log"
	"net"
	"net/http"
	"strings"
	"time"

	"github.com/snth-ai/snth-companion/internal/config"
	"github.com/snth-ai/snth-companion/internal/tools"
	"github.com/snth-ai/snth-companion/internal/ui"
)

// UIServer is the local HTTP endpoint bound on 127.0.0.1:<random>. It
// serves a small multi-page UI (status / pair / tools / sandbox /
// audit) and a handful of JSON endpoints consumed by that UI + any
// future menubar app. Never bound to anything other than localhost.
type UIServer struct {
	Listener net.Listener
	Client   *Client
}

func StartUIServer(client *Client) (*UIServer, string, error) {
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		return nil, "", fmt.Errorf("bind ui server: %w", err)
	}
	s := &UIServer{Listener: ln, Client: client}
	go func() {
		srv := &http.Server{
			Handler:      s.routes(),
			ReadTimeout:  10 * time.Second,
			WriteTimeout: 10 * time.Second,
		}
		if err := srv.Serve(ln); err != nil && err != http.ErrServerClosed {
			log.Printf("[ui] serve: %v", err)
		}
	}()
	return s, "http://" + ln.Addr().String(), nil
}

func (s *UIServer) routes() http.Handler {
	mux := http.NewServeMux()
	mux.HandleFunc("/", s.handleStatus)
	mux.HandleFunc("/pair", s.handlePairPage)
	mux.HandleFunc("/pair/save", s.handlePairSave)
	mux.HandleFunc("/pair/claim", s.handlePairClaim)
	mux.HandleFunc("/unpair", s.handleUnpair)
	mux.HandleFunc("/tools", s.handleTools)
	mux.HandleFunc("/channels", s.handleChannelsPage)
	mux.HandleFunc("/channels/save", s.handleChannelsSave)
	mux.HandleFunc("/channels/save-settings", s.handleChannelsSaveSettings)
	mux.HandleFunc("/login/codex", s.handleCodexLoginPage)
	mux.HandleFunc("/login/codex/start", s.handleCodexLoginStart)
	mux.HandleFunc("/login/codex/upload", s.handleCodexLoginUpload)
	mux.HandleFunc("/login/codex/clear", s.handleCodexLoginClear)
	mux.HandleFunc("/keys", s.handleKeysPage)
	mux.HandleFunc("/keys/save", s.handleKeysSave)
	mux.HandleFunc("/sandbox", s.handleSandboxPage)
	mux.HandleFunc("/sandbox/add", s.handleSandboxAdd)
	mux.HandleFunc("/sandbox/remove", s.handleSandboxRemove)
	mux.HandleFunc("/audit", s.handleAudit)
	mux.HandleFunc("/logs", s.handleLogsPage)
	mux.HandleFunc("/api/status", s.apiStatus)
	mux.HandleFunc("/api/audit", s.apiAudit)
	mux.HandleFunc("/health", func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte("ok"))
	})
	// Hub proxies (/api/hub/*) — inject companion bearer server-side
	// so the SPA never sees the token. See hub_proxy.go.
	s.registerHubProxies(mux)
	// Companion-local JSON endpoints the SPA calls for state that
	// doesn't need the hub (sandbox, pair, audit mirror, ...).
	// See spa_api.go.
	s.registerSPAAPIs(mux)
	// Privacy Center endpoints (per-tool trust state, master toggle,
	// path scopes, kill-switch, audit feed). See trust_api.go.
	s.registerTrustAPIs(mux)
	// React SPA — served at /ui/*. Legacy server-rendered pages
	// stay at their old paths during the porting transition; the
	// React router links back to them via Placeholder cards.
	mux.Handle("/ui/", http.StripPrefix("/ui", ui.Handler()))
	return localhostOnly(mux)
}

func localhostOnly(h http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		host, _, err := net.SplitHostPort(r.RemoteAddr)
		if err != nil {
			host = r.RemoteAddr
		}
		if host != "127.0.0.1" && host != "::1" && host != "localhost" {
			http.Error(w, "localhost only", http.StatusForbidden)
			return
		}
		h.ServeHTTP(w, r)
	})
}

// --- shared layout ----------------------------------------------------------

const layoutCSS = `
body { font-family: -apple-system, system-ui, sans-serif; background: #0f172a; color: #e2e8f0; margin: 0; }
.wrap { max-width: 860px; margin: 0 auto; padding: 24px; }
header { display: flex; justify-content: space-between; align-items: center; padding-bottom: 16px; border-bottom: 1px solid #334155; }
header h1 { font-size: 20px; margin: 0; }
nav { display: flex; gap: 18px; margin-top: 8px; margin-bottom: 24px; padding-bottom: 10px; border-bottom: 1px solid #1e293b; }
nav a { color: #94a3b8; text-decoration: none; font-size: 13px; padding-bottom: 4px; }
nav a.active { color: #60a5fa; border-bottom: 2px solid #60a5fa; }
nav a:hover { color: #e2e8f0; }
.card { background: #1e293b; border: 1px solid #334155; border-radius: 8px; padding: 20px; margin-bottom: 18px; }
.row { display: flex; justify-content: space-between; padding: 6px 0; font-size: 14px; }
.row + .row { border-top: 1px solid #233148; }
.label { color: #94a3b8; }
.ok  { color: #22c55e; }
.mid { color: #f59e0b; }
.bad { color: #ef4444; }
h2 { font-size: 16px; margin: 0 0 12px 0; color: #f8fafc; }
h3 { font-size: 14px; margin: 0 0 8px 0; color: #cbd5e1; }
table { width: 100%; border-collapse: collapse; font-size: 13px; }
th { text-align: left; color: #94a3b8; font-weight: 500; font-size: 11px; text-transform: uppercase; padding: 8px 10px; border-bottom: 1px solid #334155; }
td { padding: 8px 10px; border-bottom: 1px solid #233148; }
tr:last-child td { border-bottom: none; }
code { background: #0f172a; color: #e2e8f0; padding: 2px 6px; border-radius: 4px; font-size: 12px; }
.mono { font-family: ui-monospace, Menlo, monospace; }
button, .btn { background: #3b82f6; color: white; border: 0; padding: 8px 14px; border-radius: 6px; cursor: pointer; font-size: 13px; text-decoration: none; display: inline-block; }
button:hover { background: #2563eb; }
button.danger { background: #ef4444; }
button.danger:hover { background: #dc2626; }
button.subtle { background: #334155; }
input, textarea { background: #0f172a; color: #e2e8f0; border: 1px solid #334155; padding: 8px 10px; border-radius: 6px; width: 100%; box-sizing: border-box; font-family: ui-monospace, Menlo, monospace; font-size: 13px; }
label { display: block; color: #94a3b8; font-size: 12px; margin: 12px 0 4px 0; }
.empty { text-align: center; color: #64748b; padding: 28px 0; font-size: 13px; }
.dot { display: inline-block; width: 8px; height: 8px; border-radius: 50%; margin-right: 6px; vertical-align: middle; }
.dot.ok { background: #22c55e; } .dot.mid { background: #f59e0b; } .dot.bad { background: #ef4444; }
`

func (s *UIServer) layout(w http.ResponseWriter, active, title, body string) {
	w.Header().Set("Content-Type", "text/html; charset=utf-8")
	nav := func(path, label string) string {
		cls := ""
		if path == active {
			cls = ` class="active"`
		}
		return fmt.Sprintf(`<a href="%s"%s>%s</a>`, path, cls, label)
	}
	fmt.Fprintf(w, `<!doctype html><html><head><meta charset="utf-8"><title>SNTH Companion — %s</title><style>%s</style></head>
<body><div class="wrap">
<header><h1>SNTH Companion</h1><span class="label mono">v%s</span></header>
<nav>%s %s %s %s %s %s %s %s %s</nav>
%s
</div></body></html>`,
		title, layoutCSS, Version,
		nav("/", "Status"),
		nav("/pair", "Pair"),
		nav("/channels", "Channels"),
		nav("/login/codex", "Codex Login"),
		nav("/keys", "API Keys"),
		nav("/tools", "Tools"),
		nav("/sandbox", "Sandbox"),
		nav("/audit", "Audit"),
		nav("/logs", "Logs"),
		body,
	)
}

// --- pages -------------------------------------------------------------------

func (s *UIServer) handleStatus(w http.ResponseWriter, r *http.Request) {
	if r.URL.Path != "/" {
		http.NotFound(w, r)
		return
	}
	cfg := config.Get()
	paired := cfg != nil && cfg.CompanionToken != ""
	st := s.Client.Status()

	dotClass := "bad"
	switch st.Status {
	case "connected":
		dotClass = "ok"
	case "connecting", "paused":
		dotClass = "mid"
	}

	seen := "—"
	if !st.LastSeen.IsZero() {
		seen = st.LastSeen.Format(time.RFC3339)
	}
	synthID := "—"
	if cfg != nil && cfg.PairedSynthID != "" {
		synthID = cfg.PairedSynthID
	}
	synthURL := "—"
	if cfg != nil && cfg.PairedSynthURL != "" {
		synthURL = cfg.PairedSynthURL
	}
	lastErr := "—"
	if st.LastErr != "" {
		lastErr = st.LastErr
	}

	pairNote := ""
	if !paired {
		pairNote = `<div class="card"><h2>Not paired yet</h2><p style="color:#94a3b8;font-size:13px">Use the <a href="/pair">Pair</a> tab to connect this companion to a synth.</p></div>`
	}

	body := fmt.Sprintf(`
%s
<div class="card">
  <h2>Connection</h2>
  <div class="row"><span class="label">Status</span><span><span class="dot %s"></span>%s</span></div>
  <div class="row"><span class="label">Paired synth</span><span class="mono">%s</span></div>
  <div class="row"><span class="label">Synth URL</span><span class="mono">%s</span></div>
  <div class="row"><span class="label">Last seen</span><span class="mono">%s</span></div>
  <div class="row"><span class="label">Last error</span><span class="mono">%s</span></div>
  <div class="row"><span class="label">Tools advertised</span><span>%d</span></div>
</div>`,
		pairNote, dotClass, st.Status, synthID, synthURL, seen, lastErr, len(tools.Catalog()),
	)
	s.layout(w, "/", "Status", body)
}

func (s *UIServer) handlePairPage(w http.ResponseWriter, r *http.Request) {
	cfg := config.Get()
	paired := cfg != nil && cfg.CompanionToken != ""

	form := `
<div class="card">
  <h2>Pair with a 6-digit code</h2>
  <p style="color:#94a3b8;font-size:13px">
    In Telegram, send <code>/pair_companion</code> to your synth's bot. It will
    reply with a 6-digit code that expires in 5 minutes. Paste it here —
    synth URL, token, and synth ID are fetched from the hub automatically.
  </p>
  <form method="POST" action="/pair/claim">
    <label>Code</label>
    <input name="code" placeholder="123456" pattern="[0-9 ]+" maxlength="10" required style="font-size:20px;letter-spacing:0.15em;text-align:center" />
    <label>Hub URL</label>
    <input name="hub_url" value="https://hub.snth.ai" required />
    <div style="margin-top:16px"><button type="submit">Claim &amp; pair</button></div>
  </form>
</div>
<div class="card">
  <h2>Advanced: paste credentials manually</h2>
  <p style="color:#94a3b8;font-size:13px">
    For the operator flow (pre-pair-code) or when debugging. Paste
    the synth's public URL, the bearer token, and the synth ID.
  </p>
  <form method="POST" action="/pair/save">
    <label>Synth URL</label>
    <input name="synth_url" placeholder="https://mia-snthai-bot.synth.snth.ai" required />
    <label>Companion token (64 hex chars)</label>
    <input name="token" placeholder="…" required />
    <label>Synth ID</label>
    <input name="synth_id" placeholder="mia_snthai_bot" required />
    <div style="margin-top:16px"><button type="submit">Pair</button></div>
  </form>
</div>`

	if paired {
		form = fmt.Sprintf(`
<div class="card">
  <h2>Currently paired</h2>
  <div class="row"><span class="label">Synth</span><span class="mono">%s</span></div>
  <div class="row"><span class="label">URL</span><span class="mono">%s</span></div>
  <form method="POST" action="/unpair" style="margin-top:16px"><button class="danger" type="submit">Unpair</button></form>
</div>
<div class="card">
  <h2>Re-pair with a 6-digit code</h2>
  <p style="color:#94a3b8;font-size:13px">
    Send <code>/pair_companion</code> in Telegram to the synth you want to control. Paste the code you get back here.
  </p>
  <form method="POST" action="/pair/claim">
    <label>Code</label>
    <input name="code" placeholder="123456" pattern="[0-9 ]+" maxlength="10" required style="font-size:20px;letter-spacing:0.15em;text-align:center" />
    <label>Hub URL</label>
    <input name="hub_url" value="https://hub.snth.ai" required />
    <div style="margin-top:16px"><button type="submit">Claim &amp; re-pair</button></div>
  </form>
</div>
<div class="card">
  <h2>Advanced: manual re-pair</h2>
  <form method="POST" action="/pair/save">
    <label>Synth URL</label>
    <input name="synth_url" required />
    <label>Companion token</label>
    <input name="token" required />
    <label>Synth ID</label>
    <input name="synth_id" required />
    <div style="margin-top:12px"><button type="submit">Re-pair</button></div>
  </form>
</div>`, cfg.PairedSynthID, cfg.PairedSynthURL)
	}
	s.layout(w, "/pair", "Pair", form)
}

func (s *UIServer) handlePairSave(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "POST only", 405)
		return
	}
	if err := r.ParseForm(); err != nil {
		http.Error(w, err.Error(), 400)
		return
	}
	synthURL := strings.TrimSpace(r.FormValue("synth_url"))
	token := strings.TrimSpace(r.FormValue("token"))
	synthID := strings.TrimSpace(r.FormValue("synth_id"))
	if synthURL == "" || token == "" || synthID == "" {
		http.Error(w, "all fields required", 400)
		return
	}
	if err := config.Update(func(c *config.Config) {
		c.PairedSynthURL = synthURL
		c.CompanionToken = token
		c.PairedSynthID = synthID
		// Seed default sandbox root for the new synth if user hadn't
		// overridden. ensureDefaults takes care of the append-if-missing.
		c.SandboxRoots = append(c.SandboxRoots[:0], c.SandboxRoots...)
	}); err != nil {
		http.Error(w, "save config: "+err.Error(), 500)
		return
	}
	s.Client.Stop()
	s.Client.Start()
	http.Redirect(w, r, "/", http.StatusSeeOther)
}

// handlePairClaim takes a 6-digit code + hub URL and exchanges the
// code for {synth_url, synth_id, companion_token} via the hub's
// /api/companion/claim-pair. On success writes config + reconnects
// the WS client. On failure shows the error in the pair page.
func (s *UIServer) handlePairClaim(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "POST only", 405)
		return
	}
	if err := r.ParseForm(); err != nil {
		http.Error(w, err.Error(), 400)
		return
	}

	// Normalise: strip spaces / dashes.
	raw := strings.TrimSpace(r.FormValue("code"))
	var digits strings.Builder
	for _, ch := range raw {
		if ch >= '0' && ch <= '9' {
			digits.WriteRune(ch)
		}
	}
	code := digits.String()
	if len(code) != 6 {
		s.renderPairError(w, "Code must be exactly 6 digits (got "+fmt.Sprint(len(code))+").")
		return
	}

	hubURL := strings.TrimRight(strings.TrimSpace(r.FormValue("hub_url")), "/")
	if hubURL == "" {
		hubURL = "https://hub.snth.ai"
	}

	payload, _ := json.Marshal(map[string]string{"code": code})
	req, err := http.NewRequestWithContext(r.Context(), "POST",
		hubURL+"/api/companion/claim-pair", bytes.NewReader(payload))
	if err != nil {
		s.renderPairError(w, "Build request: "+err.Error())
		return
	}
	req.Header.Set("Content-Type", "application/json")
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		s.renderPairError(w, "Could not reach hub at "+hubURL+": "+err.Error())
		return
	}
	defer resp.Body.Close()
	body, _ := io.ReadAll(resp.Body)
	if resp.StatusCode == 404 {
		s.renderPairError(w, "Code not found — either it expired, was already used, or you typed it wrong. Ask your synth for a fresh one with /pair_companion.")
		return
	}
	if resp.StatusCode == 410 {
		s.renderPairError(w, "Code expired. Codes are valid for 5 minutes — ask the synth for a fresh one.")
		return
	}
	if resp.StatusCode/100 != 2 {
		s.renderPairError(w, fmt.Sprintf("Hub returned %d: %s", resp.StatusCode, string(body)))
		return
	}

	var hubResp struct {
		SynthURL       string `json:"synth_url"`
		SynthID        string `json:"synth_id"`
		CompanionToken string `json:"companion_token"`
	}
	if err := json.Unmarshal(body, &hubResp); err != nil {
		s.renderPairError(w, "Parse hub response: "+err.Error())
		return
	}

	if err := config.Update(func(c *config.Config) {
		c.PairedSynthURL = hubResp.SynthURL
		c.PairedSynthID = hubResp.SynthID
		c.CompanionToken = hubResp.CompanionToken
		c.HubURL = hubURL
		c.SandboxRoots = append(c.SandboxRoots[:0], c.SandboxRoots...)
	}); err != nil {
		s.renderPairError(w, "Save config: "+err.Error())
		return
	}
	s.Client.Stop()
	s.Client.Start()
	http.Redirect(w, r, "/", http.StatusSeeOther)
}

// renderPairError bounces the user back to /pair with a red banner.
func (s *UIServer) renderPairError(w http.ResponseWriter, msg string) {
	body := fmt.Sprintf(`
<div class="card" style="border-color:#ef4444;background:#1c1917">
  <h2 style="color:#ef4444">Pair failed</h2>
  <p style="color:#fca5a5;font-size:13px">%s</p>
  <p style="margin-top:12px"><a href="/pair">← Back to pair form</a></p>
</div>`, htmlEscape(msg))
	s.layout(w, "/pair", "Pair", body)
}

func htmlEscape(s string) string {
	r := strings.NewReplacer("&", "&amp;", "<", "&lt;", ">", "&gt;", `"`, "&quot;")
	return r.Replace(s)
}

func (s *UIServer) handleUnpair(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "POST only", 405)
		return
	}
	_ = config.Update(func(c *config.Config) {
		c.PairedSynthURL = ""
		c.CompanionToken = ""
		c.PairedSynthID = ""
	})
	s.Client.Stop()
	s.Client.Start()
	http.Redirect(w, r, "/pair", http.StatusSeeOther)
}

// handleTools renders the tool catalog as a visual grid with
// per-tool last-invocation status (pulled from the in-memory audit
// log). Full permission probing (macOS TCC: Calendar, Contacts,
// Reminders, FDA) and per-tool Test buttons are deferred.
func (s *UIServer) handleTools(w http.ResponseWriter, r *http.Request) {
	cat := tools.Catalog()

	// Pull last invocation per tool from the audit log so the grid can
	// show which tools are actually being used and which are failing.
	audit := RecentAudit(500)
	lastByTool := map[string]AuditEntry{}
	callCount := map[string]int{}
	errCount := map[string]int{}
	for _, e := range audit {
		callCount[e.Tool]++
		if e.Outcome == "error" {
			errCount[e.Tool]++
		}
		if prev, ok := lastByTool[e.Tool]; !ok || e.StartedAt.After(prev.StartedAt) {
			lastByTool[e.Tool] = e
		}
	}

	totalCalls := 0
	totalErrors := 0
	for _, n := range callCount {
		totalCalls += n
	}
	for _, n := range errCount {
		totalErrors += n
	}

	summary := fmt.Sprintf(`
<div class="card">
  <h2>Tool surface</h2>
  <div class="row"><span class="label">Registered</span><span>%d</span></div>
  <div class="row"><span class="label">Calls seen (session)</span><span>%d</span></div>
  <div class="row"><span class="label">Errors (session)</span><span class="%s">%d</span></div>
  <p style="color:#64748b;font-size:12px;margin-top:10px">
    Tools are advertised to the paired synth in the <code>hello</code> frame on
    connect. To enable a new tool, restart the companion.
  </p>
</div>`,
		len(cat), totalCalls,
		map[bool]string{true: "bad", false: "ok"}[totalErrors > 0], totalErrors)

	grid := ""
	for _, d := range cat {
		badgeColor := "#334155"
		switch d.DangerLevel {
		case "safe":
			badgeColor = "#22c55e"
		case "prompt":
			badgeColor = "#f59e0b"
		case "always-prompt":
			badgeColor = "#ef4444"
		}

		status := ""
		if e, ok := lastByTool[d.Name]; ok {
			outcomeCls := "ok"
			switch e.Outcome {
			case "error":
				outcomeCls = "bad"
			case "denied":
				outcomeCls = "mid"
			}
			status = fmt.Sprintf(`
<div style="margin-top:10px;padding-top:10px;border-top:1px solid #233148;font-size:12px;color:#94a3b8">
  Last: <span class="mono">%s</span> · <span class="%s">%s</span> · <span class="mono">%dms</span><br>
  <span class="label">Calls: %d · Errors: %d</span>
</div>`,
				e.StartedAt.Format("15:04:05"), outcomeCls, e.Outcome, e.DurationMs,
				callCount[d.Name], errCount[d.Name])
		} else {
			status = `<div style="margin-top:10px;padding-top:10px;border-top:1px solid #233148;font-size:12px;color:#64748b">Not yet used this session.</div>`
		}

		grid += fmt.Sprintf(`
<div class="card" style="margin-bottom:0">
  <div style="display:flex;align-items:center;justify-content:space-between;gap:8px">
    <h3 class="mono" style="margin:0;font-size:14px;color:#f8fafc">%s</h3>
    <span style="background:%s;padding:2px 8px;border-radius:10px;font-size:11px;color:#0f172a">%s</span>
  </div>
  <p style="color:#94a3b8;font-size:12px;margin:10px 0 0 0">%s</p>
  %s
</div>`, d.Name, badgeColor, d.DangerLevel, d.Description, status)
	}

	if grid == "" {
		grid = `<div class="card"><div class="empty">No tools registered.</div></div>`
	}

	body := summary + fmt.Sprintf(`
<div style="display:grid;grid-template-columns:repeat(auto-fill,minmax(300px,1fr));gap:14px">%s</div>`, grid)
	s.layout(w, "/tools", "Tools", body)
}

func (s *UIServer) handleSandboxPage(w http.ResponseWriter, r *http.Request) {
	cfg := config.Get()
	roots := ""
	for i, root := range cfg.SandboxRoots {
		roots += fmt.Sprintf(
			`<tr><td class="mono">%s</td><td style="text-align:right"><form method="POST" action="/sandbox/remove" style="display:inline"><input type="hidden" name="idx" value="%d"/><button class="danger subtle" style="background:#334155;padding:4px 10px;font-size:11px" type="submit">Remove</button></form></td></tr>`,
			root, i,
		)
	}
	if roots == "" {
		roots = `<tr><td colspan="2" class="empty">No sandbox roots. Every file operation will require explicit approval.</td></tr>`
	}
	body := fmt.Sprintf(`
<div class="card">
  <h2>Sandbox roots</h2>
  <p style="color:#94a3b8;font-size:13px">Tools operating inside any of these paths skip the approval dialog for safe commands. Paths outside always prompt.</p>
  <table><tbody>%s</tbody></table>
  <form method="POST" action="/sandbox/add" style="margin-top:16px; display:flex; gap:8px">
    <input name="path" placeholder="/Users/…/some-folder" required style="flex:1" />
    <button type="submit">Add</button>
  </form>
</div>`, roots)
	s.layout(w, "/sandbox", "Sandbox", body)
}

func (s *UIServer) handleSandboxAdd(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "POST only", 405)
		return
	}
	_ = r.ParseForm()
	p := strings.TrimSpace(r.FormValue("path"))
	if p == "" {
		http.Error(w, "path required", 400)
		return
	}
	_ = config.Update(func(c *config.Config) {
		for _, root := range c.SandboxRoots {
			if root == p {
				return
			}
		}
		c.SandboxRoots = append(c.SandboxRoots, p)
	})
	http.Redirect(w, r, "/sandbox", http.StatusSeeOther)
}

func (s *UIServer) handleSandboxRemove(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "POST only", 405)
		return
	}
	_ = r.ParseForm()
	idx := r.FormValue("idx")
	var target int
	fmt.Sscanf(idx, "%d", &target)
	_ = config.Update(func(c *config.Config) {
		if target >= 0 && target < len(c.SandboxRoots) {
			c.SandboxRoots = append(c.SandboxRoots[:target], c.SandboxRoots[target+1:]...)
		}
	})
	http.Redirect(w, r, "/sandbox", http.StatusSeeOther)
}

// channelSettings is the companion-side mirror of
// registry.ChannelSettings on the hub. Deliberately not imported from
// there (we keep the companion free of hub internals) — the on-wire
// JSON shape matches, which is all that matters.
type channelSettings struct {
	InstanceID        string            `json:"instance_id"`
	InstagramEnabled  bool              `json:"instagram_enabled"`
	InstagramReadOnly bool              `json:"instagram_read_only"`
	InstagramOwnerMap map[string]string `json:"instagram_owner_map"`
	WhatsAppEnabled   bool              `json:"whatsapp_enabled"`
	WhatsAppReadOnly  bool              `json:"whatsapp_read_only"`
	WhatsAppProxy     string            `json:"whatsapp_proxy"`
	UpdatedAt         string            `json:"updated_at"`
}

// handleChannelsPage renders the inline Instagram + WhatsApp form. On
// GET it fetches current settings from the hub via /api/my/channel-settings;
// on POST it parses the form and pushes back. The hub URL override form
// is kept as a secondary card for ops rescue if the hub moves.
func (s *UIServer) handleChannelsPage(w http.ResponseWriter, r *http.Request) {
	cfg := config.Get()
	paired := cfg != nil && cfg.CompanionToken != ""
	if !paired {
		s.layout(w, "/channels", "Channels",
			`<div class="card"><h2>Not paired</h2><p style="color:#94a3b8;font-size:13px">Channel settings live on the hub, scoped to the synth this companion is paired with. Use the <a href="/pair">Pair</a> tab first.</p></div>`)
		return
	}

	hubURL := cfg.HubURL
	if hubURL == "" {
		hubURL = "https://hub.snth.ai"
	}

	settings, fetchErr := fetchMyChannelSettings(hubURL, cfg.CompanionToken)

	settingsCard := ""
	if fetchErr != nil {
		settingsCard = fmt.Sprintf(`
<div class="card" style="border-color:#ef4444">
  <h2>✗ Failed to load settings from hub</h2>
  <p class="mono" style="color:#fca5a5;font-size:12px">%s</p>
  <p style="color:#94a3b8;font-size:13px;margin-top:10px">
    Check the hub URL below and that you're paired to a current synth.
  </p>
</div>`, htmlEscape(fetchErr.Error()))
	} else {
		ownerMap := formatOwnerMapText(settings.InstagramOwnerMap)
		settingsCard = fmt.Sprintf(`
<div class="card">
  <h2>Instagram</h2>
  <form method="POST" action="/channels/save-settings">
    <label><input type="checkbox" name="instagram_enabled" %s /> Enabled</label>
    <label><input type="checkbox" name="instagram_read_only" %s /> Read-only (archive inbound, don't send)</label>
    <label>Owner map <span style="color:#64748b;font-size:11px">— one per line, <code>IGSID = canonical_session_id</code></span></label>
    <textarea name="instagram_owner_map" rows="4" placeholder="1477095863960189 = tg_7392742">%s</textarea>

    <h2 style="margin-top:26px">WhatsApp</h2>
    <label><input type="checkbox" name="whatsapp_enabled" %s /> Enabled</label>
    <label><input type="checkbox" name="whatsapp_read_only" %s /> Read-only</label>
    <label>Proxy <span style="color:#64748b;font-size:11px">— empty = hub residential pool, <code>off</code> = direct (Hetzner IP), or explicit <code>http://…</code> / <code>socks5://…</code></span></label>
    <input name="whatsapp_proxy" value="%s" />

    <div style="margin-top:20px"><button type="submit">Save &amp; push to synth</button></div>
  </form>
  <p style="color:#64748b;font-size:12px;margin-top:14px">
    Saving updates the hub DB and then recreates the synth's container with
    the new env subset via node-agent. Owner-map changes don't restart —
    the hub reads them fresh on every webhook.
  </p>
</div>`,
			checkedIf(settings.InstagramEnabled), checkedIf(settings.InstagramReadOnly),
			htmlEscape(ownerMap),
			checkedIf(settings.WhatsAppEnabled), checkedIf(settings.WhatsAppReadOnly),
			htmlEscape(settings.WhatsAppProxy),
		)
	}

	hubForm := fmt.Sprintf(`
<div class="card">
  <h2>Hub URL</h2>
  <p style="color:#94a3b8;font-size:13px">
    Saved from the pair-claim flow. Change this only if the hub moves.
  </p>
  <form method="POST" action="/channels/save">
    <label>Hub base URL</label>
    <input name="hub_url" value="%s" required />
    <div style="margin-top:16px"><button type="submit">Save</button></div>
  </form>
</div>`, htmlEscape(hubURL))

	s.layout(w, "/channels", "Channels", settingsCard+hubForm)
}

func (s *UIServer) handleChannelsSave(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "POST only", 405)
		return
	}
	_ = r.ParseForm()
	hubURL := strings.TrimRight(strings.TrimSpace(r.FormValue("hub_url")), "/")
	_ = config.Update(func(c *config.Config) {
		c.HubURL = hubURL
	})
	http.Redirect(w, r, "/channels", http.StatusSeeOther)
}

func (s *UIServer) handleChannelsSaveSettings(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "POST only", 405)
		return
	}
	if err := r.ParseForm(); err != nil {
		http.Error(w, "parse form: "+err.Error(), 400)
		return
	}
	cfg := config.Get()
	if cfg == nil || cfg.CompanionToken == "" {
		http.Error(w, "not paired", 400)
		return
	}
	hubURL := cfg.HubURL
	if hubURL == "" {
		hubURL = "https://hub.snth.ai"
	}
	cs := &channelSettings{
		InstagramEnabled:  r.FormValue("instagram_enabled") != "",
		InstagramReadOnly: r.FormValue("instagram_read_only") != "",
		InstagramOwnerMap: parseOwnerMapText(r.FormValue("instagram_owner_map")),
		WhatsAppEnabled:   r.FormValue("whatsapp_enabled") != "",
		WhatsAppReadOnly:  r.FormValue("whatsapp_read_only") != "",
		WhatsAppProxy:     strings.TrimSpace(r.FormValue("whatsapp_proxy")),
	}
	if err := saveMyChannelSettings(hubURL, cfg.CompanionToken, cs); err != nil {
		s.layout(w, "/channels", "Channels",
			fmt.Sprintf(`<div class="card" style="border-color:#ef4444"><h2>Save failed</h2><p class="mono" style="color:#fca5a5">%s</p><p><a href="/channels">Back</a></p></div>`, htmlEscape(err.Error())))
		return
	}
	http.Redirect(w, r, "/channels", http.StatusSeeOther)
}

// --- /api/my/channel-settings helpers ---------------------------------------

func fetchMyChannelSettings(hubURL, token string) (*channelSettings, error) {
	req, err := http.NewRequest("GET", strings.TrimRight(hubURL, "/")+"/api/my/channel-settings", nil)
	if err != nil {
		return nil, err
	}
	req.Header.Set("Authorization", "Bearer "+token)
	client := &http.Client{Timeout: 15 * time.Second}
	resp, err := client.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	body, _ := io.ReadAll(resp.Body)
	if resp.StatusCode/100 != 2 {
		return nil, fmt.Errorf("hub %d: %s", resp.StatusCode, strings.TrimSpace(string(body)))
	}
	var cs channelSettings
	if err := json.Unmarshal(body, &cs); err != nil {
		return nil, fmt.Errorf("decode: %w (body: %s)", err, string(body))
	}
	if cs.InstagramOwnerMap == nil {
		cs.InstagramOwnerMap = map[string]string{}
	}
	return &cs, nil
}

func saveMyChannelSettings(hubURL, token string, cs *channelSettings) error {
	body, err := json.Marshal(cs)
	if err != nil {
		return err
	}
	req, err := http.NewRequest("POST", strings.TrimRight(hubURL, "/")+"/api/my/channel-settings", bytes.NewReader(body))
	if err != nil {
		return err
	}
	req.Header.Set("Authorization", "Bearer "+token)
	req.Header.Set("Content-Type", "application/json")
	client := &http.Client{Timeout: 30 * time.Second}
	resp, err := client.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	raw, _ := io.ReadAll(resp.Body)
	if resp.StatusCode/100 != 2 {
		return fmt.Errorf("hub %d: %s", resp.StatusCode, strings.TrimSpace(string(raw)))
	}
	// Surface non-fatal env-push errors so the user sees the partial
	// success rather than blindly assuming everything worked.
	var resp2 map[string]any
	if err := json.Unmarshal(raw, &resp2); err == nil {
		if msg, ok := resp2["env_push_error"].(string); ok && msg != "" {
			return fmt.Errorf("saved to DB, env push to node failed: %s", msg)
		}
	}
	return nil
}

// fetchMyLogs pulls the last `lines` of the paired synth's container log
// through the hub's /api/my/logs endpoint. The response JSON shape is
// {synth_id, lines, log}. 15s timeout: node-agent's docker-logs call
// can take a few seconds on a loaded host.
func fetchMyLogs(hubURL, token string, lines int) (string, error) {
	if lines <= 0 {
		lines = 200
	}
	url := fmt.Sprintf("%s/api/my/logs?lines=%d", strings.TrimRight(hubURL, "/"), lines)
	req, err := http.NewRequest("GET", url, nil)
	if err != nil {
		return "", err
	}
	req.Header.Set("Authorization", "Bearer "+token)
	client := &http.Client{Timeout: 20 * time.Second}
	resp, err := client.Do(req)
	if err != nil {
		return "", err
	}
	defer resp.Body.Close()
	body, _ := io.ReadAll(resp.Body)
	if resp.StatusCode/100 != 2 {
		return "", fmt.Errorf("hub %d: %s", resp.StatusCode, strings.TrimSpace(string(body)))
	}
	var out struct {
		SynthID string `json:"synth_id"`
		Log     string `json:"log"`
	}
	if err := json.Unmarshal(body, &out); err != nil {
		return "", fmt.Errorf("decode: %w", err)
	}
	return out.Log, nil
}

func checkedIf(b bool) string {
	if b {
		return `checked`
	}
	return ""
}

// parseOwnerMapText mirrors snth-hub/admin/channel_settings.go:parseOwnerMap
// so the on-disk format matches what the admin UI uses.
func parseOwnerMapText(raw string) map[string]string {
	out := map[string]string{}
	for _, line := range strings.Split(raw, "\n") {
		line = strings.TrimSpace(line)
		if line == "" || strings.HasPrefix(line, "#") {
			continue
		}
		sep := -1
		if idx := strings.Index(line, "="); idx > 0 {
			sep = idx
		} else if idx := strings.Index(line, ":"); idx > 0 {
			sep = idx
		}
		if sep < 0 {
			continue
		}
		igsid := strings.TrimSpace(line[:sep])
		sess := strings.TrimSpace(line[sep+1:])
		if igsid != "" && sess != "" {
			out[igsid] = sess
		}
	}
	return out
}

func formatOwnerMapText(m map[string]string) string {
	if len(m) == 0 {
		return ""
	}
	keys := make([]string, 0, len(m))
	for k := range m {
		keys = append(keys, k)
	}
	for i := 0; i < len(keys); i++ {
		for j := i + 1; j < len(keys); j++ {
			if keys[j] < keys[i] {
				keys[i], keys[j] = keys[j], keys[i]
			}
		}
	}
	var sb strings.Builder
	for _, k := range keys {
		fmt.Fprintf(&sb, "%s = %s\n", k, m[k])
	}
	return strings.TrimRight(sb.String(), "\n")
}

// handleLogsPage surfaces the two log sources the companion has access to:
//   - remote synth container log, pulled via /api/my/logs (polled)
//   - local RPC audit log (in-memory, from /audit)
func (s *UIServer) handleLogsPage(w http.ResponseWriter, r *http.Request) {
	cfg := config.Get()
	paired := cfg != nil && cfg.CompanionToken != ""

	hubURL := ""
	if cfg != nil {
		hubURL = cfg.HubURL
	}
	if hubURL == "" {
		hubURL = "https://hub.snth.ai"
	}

	remoteCard := ""
	if paired && cfg.PairedSynthID != "" {
		lines := 200
		if n, err := fmt.Sscanf(r.URL.Query().Get("lines"), "%d", &lines); err != nil || n == 0 {
			lines = 200
		}
		if lines < 20 {
			lines = 20
		}
		if lines > 2000 {
			lines = 2000
		}
		logText, fetchErr := fetchMyLogs(hubURL, cfg.CompanionToken, lines)
		auto := r.URL.Query().Get("auto") == "1"
		refreshScript := ""
		autoLabel := "Auto-refresh off"
		toggleHref := "/logs?auto=1"
		if auto {
			refreshScript = `<script>setTimeout(function(){location.reload();},2500);</script>`
			autoLabel = "● Auto-refresh on (2.5s)"
			toggleHref = "/logs"
		}

		if fetchErr != nil {
			remoteCard = fmt.Sprintf(`
<div class="card" style="border-color:#ef4444">
  <h2>Synth container log — fetch failed</h2>
  <p class="mono" style="color:#fca5a5;font-size:12px">%s</p>
  <p style="color:#94a3b8;font-size:13px;margin-top:10px">
    Endpoint <code>%s/api/my/logs</code> is unreachable or the bearer token is no longer registered.
  </p>
</div>`, htmlEscape(fetchErr.Error()), htmlEscape(hubURL))
		} else {
			deep := fmt.Sprintf("%s/instances/logs?id=%s", hubURL, cfg.PairedSynthID)
			remoteCard = fmt.Sprintf(`
<div class="card">
  <div style="display:flex;align-items:center;justify-content:space-between;gap:10px;flex-wrap:wrap">
    <h2 style="margin:0">Synth container log · <code>%s</code> · last %d lines</h2>
    <div style="display:flex;gap:8px">
      <a class="btn subtle" href="%s">%s</a>
      <a class="btn subtle" href="/logs?auto=%s&lines=%d">Refresh</a>
      <a class="btn subtle" href="%s" target="_blank" rel="noopener">Open on hub →</a>
    </div>
  </div>
  <pre style="background:#0f172a;border:1px solid #334155;border-radius:6px;padding:12px;margin-top:14px;overflow:auto;max-height:520px;font-size:12px;line-height:1.45;white-space:pre-wrap;word-break:break-word" class="mono">%s</pre>
  <p style="color:#64748b;font-size:12px;margin-top:10px">
    Fetched through <code>/api/my/logs</code> — scoped to your paired
    synth by companion token. True SSE stream is future work.
  </p>
</div>%s`, cfg.PairedSynthID, lines,
				toggleHref, autoLabel,
				map[bool]string{true: "1", false: "0"}[auto], lines,
				deep,
				htmlEscape(logText),
				refreshScript,
			)
		}
	} else {
		remoteCard = `
<div class="card">
  <h2>Synth container log (remote)</h2>
  <p style="color:#94a3b8;font-size:13px">
    Pair the companion first — remote log is scoped to the paired synth.
  </p>
</div>`
	}

	// Local audit snippet: last 30 entries, same shape as /audit.
	entries := RecentAudit(30)
	rows := ""
	for _, e := range entries {
		cls := "ok"
		switch e.Outcome {
		case "error":
			cls = "bad"
		case "denied":
			cls = "mid"
		}
		rows += fmt.Sprintf(
			`<tr><td class="mono">%s</td><td class="mono">%s</td><td class="%s">%s</td></tr>`,
			e.StartedAt.Format("15:04:05"), e.Tool, cls, e.Outcome,
		)
	}
	if rows == "" {
		rows = `<tr><td colspan="3" class="empty">No recent RPCs.</td></tr>`
	}
	localCard := fmt.Sprintf(`
<div class="card">
  <h2>Companion RPC log (local)</h2>
  <p style="color:#94a3b8;font-size:13px">
    Last %d tool calls received from the paired synth. Full table at
    <a href="/audit">/audit</a>. In-memory only.
  </p>
  <table><thead><tr><th>When</th><th>Tool</th><th>Outcome</th></tr></thead>
  <tbody>%s</tbody></table>
</div>`, len(entries), rows)

	s.layout(w, "/logs", "Logs", remoteCard+localCard)
}

func (s *UIServer) handleAudit(w http.ResponseWriter, r *http.Request) {
	entries := RecentAudit(100)
	rows := ""
	for _, e := range entries {
		cls := "ok"
		switch e.Outcome {
		case "error":
			cls = "bad"
		case "denied":
			cls = "mid"
		}
		rows += fmt.Sprintf(
			`<tr><td class="mono">%s</td><td class="mono">%s</td><td class="mono">%dms</td><td class="%s">%s</td><td style="color:#94a3b8" class="mono">%s</td></tr>`,
			e.StartedAt.Format("15:04:05"), e.Tool, e.DurationMs, cls, e.Outcome, e.ArgsSummary,
		)
	}
	if rows == "" {
		rows = `<tr><td colspan="5" class="empty">No recent RPCs.</td></tr>`
	}
	body := fmt.Sprintf(`
<div class="card">
  <h2>Recent RPCs</h2>
  <p style="color:#94a3b8;font-size:13px">Last %d tool calls from the paired synth. In-memory only — cleared on restart.</p>
  <table><thead><tr><th>When</th><th>Tool</th><th>Duration</th><th>Outcome</th><th>Args</th></tr></thead>
  <tbody>%s</tbody></table>
</div>`, len(entries), rows)
	s.layout(w, "/audit", "Audit", body)
}

// --- JSON API ---------------------------------------------------------------

func (s *UIServer) apiStatus(w http.ResponseWriter, r *http.Request) {
	cfg := config.Get()
	st := s.Client.Status()
	resp := map[string]any{
		"status":        st.Status,
		"last_error":    st.LastErr,
		"last_seen":     st.LastSeen,
		"paired":        cfg != nil && cfg.CompanionToken != "",
		"synth_url":     synthField(cfg, "url"),
		"synth_id":      synthField(cfg, "id"),
		"sandbox_roots": sandboxOr(cfg),
		"tools":         tools.Catalog(),
		"version":       Version,
	}
	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(resp)
}

func (s *UIServer) apiAudit(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(map[string]any{
		"entries": RecentAudit(100),
	})
}

// --- helpers ----------------------------------------------------------------

func synthField(c *config.Config, field string) string {
	if c == nil {
		return ""
	}
	switch field {
	case "url":
		return c.PairedSynthURL
	case "id":
		return c.PairedSynthID
	}
	return ""
}

func sandboxOr(c *config.Config) []string {
	if c == nil {
		return nil
	}
	return c.SandboxRoots
}
