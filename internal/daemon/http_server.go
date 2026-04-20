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
	mux.HandleFunc("/sandbox", s.handleSandboxPage)
	mux.HandleFunc("/sandbox/add", s.handleSandboxAdd)
	mux.HandleFunc("/sandbox/remove", s.handleSandboxRemove)
	mux.HandleFunc("/audit", s.handleAudit)
	mux.HandleFunc("/api/status", s.apiStatus)
	mux.HandleFunc("/api/audit", s.apiAudit)
	mux.HandleFunc("/health", func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte("ok"))
	})
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
<nav>%s %s %s %s %s</nav>
%s
</div></body></html>`,
		title, layoutCSS, Version,
		nav("/", "Status"),
		nav("/pair", "Pair"),
		nav("/tools", "Tools"),
		nav("/sandbox", "Sandbox"),
		nav("/audit", "Audit"),
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
  <h2>Re-pair with a different synth</h2>
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

func (s *UIServer) handleTools(w http.ResponseWriter, r *http.Request) {
	rows := ""
	for _, d := range tools.Catalog() {
		badge := "#334155"
		switch d.DangerLevel {
		case "safe":
			badge = "#22c55e"
		case "prompt":
			badge = "#f59e0b"
		case "always-prompt":
			badge = "#ef4444"
		}
		rows += fmt.Sprintf(
			`<tr><td class="mono">%s</td><td><span style="background:%s;padding:2px 8px;border-radius:10px;font-size:11px;color:#0f172a">%s</span></td><td style="color:#94a3b8">%s</td></tr>`,
			d.Name, badge, d.DangerLevel, d.Description,
		)
	}
	if rows == "" {
		rows = `<tr><td colspan="3" class="empty">No tools registered.</td></tr>`
	}
	body := fmt.Sprintf(`
<div class="card">
  <h2>Registered tools</h2>
  <p style="color:#94a3b8;font-size:13px">These are advertised to the paired synth in the <code>hello</code> frame on connect. Only connect-time changes apply — to enable a new tool, restart the companion.</p>
  <table><thead><tr><th>Name</th><th>Danger</th><th>Description</th></tr></thead>
  <tbody>%s</tbody></table>
</div>`, rows)
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
