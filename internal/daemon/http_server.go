package daemon

import (
	"encoding/json"
	"fmt"
	"log"
	"net"
	"net/http"
	"strings"
	"time"

	"github.com/snth-ai/snth-companion/internal/config"
)

// UIServer is the local HTTP endpoint listening on 127.0.0.1:<port>. It's
// how the menubar (future) and any ad-hoc curl requests inspect state and
// trigger operations. Never bind to anything other than localhost — the
// handlers have no auth.
type UIServer struct {
	Listener net.Listener
	Client   *Client
}

// Start binds a random high port on 127.0.0.1 and begins serving. Returns
// the URL the user / menubar should hit.
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
	mux.HandleFunc("/", s.handleIndex)
	mux.HandleFunc("/status", s.handleStatus)
	mux.HandleFunc("/pair", s.handlePair)
	mux.HandleFunc("/unpair", s.handleUnpair)
	mux.HandleFunc("/health", func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte("ok"))
	})
	return localhostOnly(mux)
}

// localhostOnly rejects anything that didn't come from 127.0.0.1 / ::1.
// Defense-in-depth: we already bind to 127.0.0.1 only, but if someone's
// running with a weird setup we don't want the UI endpoint exposed.
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

func (s *UIServer) handleIndex(w http.ResponseWriter, r *http.Request) {
	if r.URL.Path != "/" {
		http.NotFound(w, r)
		return
	}
	cfg := config.Get()
	paired := cfg != nil && cfg.CompanionToken != ""
	status := s.Client.Status()

	w.Header().Set("Content-Type", "text/html; charset=utf-8")
	fmt.Fprintf(w, `<!doctype html>
<html><head><title>SNTH Companion</title>
<style>
body { font-family: -apple-system, system-ui, sans-serif; padding: 40px; max-width: 720px; margin: 0 auto; background: #0f172a; color: #e2e8f0; }
h1 { font-size: 22px; }
.card { background: #1e293b; border: 1px solid #334155; border-radius: 8px; padding: 20px; margin-top: 20px; }
.row { display: flex; justify-content: space-between; padding: 6px 0; }
.label { color: #94a3b8; font-size: 13px; }
code { background: #0f172a; padding: 2px 6px; border-radius: 4px; font-size: 13px; }
.status-ok { color: #22c55e; }
.status-bad { color: #ef4444; }
.status-mid { color: #f59e0b; }
.btn { background: #3b82f6; color: white; border: 0; padding: 8px 14px; border-radius: 6px; cursor: pointer; }
input { background: #0f172a; color: #e2e8f0; border: 1px solid #334155; padding: 8px; border-radius: 6px; width: 100%%; margin-bottom: 8px; font-family: monospace; }
</style></head><body>
<h1>SNTH Companion</h1>
<div class="card">
  <div class="row"><span class="label">Status</span><span class="%s">%s</span></div>
  <div class="row"><span class="label">Paired to</span><span>%s</span></div>
  <div class="row"><span class="label">Sandbox roots</span><span>%d</span></div>
  <div class="row"><span class="label">Last error</span><span>%s</span></div>
</div>
`, statusClass(status.Status), status.Status, htmlSynthLabel(cfg), sandboxCount(cfg), htmlStr(status.LastErr))

	if !paired {
		fmt.Fprintf(w, `
<div class="card">
  <h3>Pair this companion to a synth</h3>
  <p>Day-1 manual pair: paste the synth URL and companion token you got out of band.</p>
  <form method="POST" action="/pair">
    <label class="label">Synth URL (e.g. https://hub.snth.ai/instances/mia_snthai_bot)</label>
    <input name="synth_url" placeholder="https://..." />
    <label class="label">Companion token</label>
    <input name="token" placeholder="opaque token" />
    <label class="label">Synth ID (for sandbox folder name)</label>
    <input name="synth_id" placeholder="mia_snthai_bot" />
    <button class="btn" type="submit">Pair</button>
  </form>
</div>`)
	} else {
		fmt.Fprintf(w, `
<div class="card">
  <form method="POST" action="/unpair"><button class="btn" type="submit">Unpair</button></form>
</div>`)
	}
	fmt.Fprintf(w, `</body></html>`)
}

func (s *UIServer) handleStatus(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Content-Type", "application/json")
	st := s.Client.Status()
	cfg := config.Get()
	resp := map[string]any{
		"status":        st.Status,
		"last_error":    st.LastErr,
		"last_seen":     st.LastSeen,
		"paired":        cfg != nil && cfg.CompanionToken != "",
		"synth_url":     redactURL(cfg),
		"synth_id":      synthIDOrEmpty(cfg),
		"sandbox_roots": sandboxRootsOrNil(cfg),
		"version":       Version,
	}
	_ = json.NewEncoder(w).Encode(resp)
}

func (s *UIServer) handlePair(w http.ResponseWriter, r *http.Request) {
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
		http.Error(w, "synth_url, token, synth_id required", 400)
		return
	}
	if err := config.Update(func(c *config.Config) {
		c.PairedSynthURL = synthURL
		c.CompanionToken = token
		c.PairedSynthID = synthID
	}); err != nil {
		http.Error(w, "save config: "+err.Error(), 500)
		return
	}
	// Nudge the client to reconnect with the new creds.
	s.Client.Stop()
	s.Client.Start()
	http.Redirect(w, r, "/", http.StatusSeeOther)
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
	http.Redirect(w, r, "/", http.StatusSeeOther)
}

func statusClass(s string) string {
	switch s {
	case "connected":
		return "status-ok"
	case "connecting", "paused":
		return "status-mid"
	default:
		return "status-bad"
	}
}

func htmlStr(s string) string {
	if s == "" {
		return "—"
	}
	return s
}

func htmlSynthLabel(c *config.Config) string {
	if c == nil || c.PairedSynthID == "" {
		return "—"
	}
	return c.PairedSynthID
}

func sandboxCount(c *config.Config) int {
	if c == nil {
		return 0
	}
	return len(c.SandboxRoots)
}

func sandboxRootsOrNil(c *config.Config) []string {
	if c == nil {
		return nil
	}
	return c.SandboxRoots
}

func synthIDOrEmpty(c *config.Config) string {
	if c == nil {
		return ""
	}
	return c.PairedSynthID
}

// redactURL returns the synth URL (it's not secret) but trims trailing /ws
// etc so the UI shows something clean.
func redactURL(c *config.Config) string {
	if c == nil {
		return ""
	}
	return c.PairedSynthURL
}
