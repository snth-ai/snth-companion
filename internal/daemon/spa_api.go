package daemon

// spa_api.go — JSON endpoints the React SPA consumes directly (no
// hub involvement). Mirrors the existing HTML form handlers 1:1 but
// returns structured JSON responses the new UI expects.
//
// Legacy HTML handlers (handlePairClaim, handleChannelsPage, etc.)
// stay intact at their old paths for the transition. Once every
// React page is live the legacy set will be deleted in one sweep.

import (
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"strings"
	"time"

	"github.com/snth-ai/snth-companion/internal/config"
	"github.com/snth-ai/snth-companion/internal/tools"
)

func (s *UIServer) registerSPAAPIs(mux *http.ServeMux) {
	mux.HandleFunc("/api/pair/claim", s.apiPairClaim)
	mux.HandleFunc("/api/pair/save", s.apiPairSave)
	mux.HandleFunc("/api/unpair", s.apiUnpair)
	mux.HandleFunc("/api/sandbox", s.apiSandbox)
	mux.HandleFunc("/api/sandbox/add", s.apiSandboxAdd)
	mux.HandleFunc("/api/sandbox/remove", s.apiSandboxRemove)
	mux.HandleFunc("/api/codex-login/state", s.apiCodexLoginState)
	mux.HandleFunc("/api/codex-login/start", s.apiCodexLoginStart)
	mux.HandleFunc("/api/codex-login/upload", s.apiCodexLoginUpload)
	mux.HandleFunc("/api/codex-login/clear", s.apiCodexLoginClear)
	mux.HandleFunc("/api/tools", s.apiTools)
}

// --- Pair -----------------------------------------------------------

type pairClaimRequest struct {
	Code   string `json:"code"`
	HubURL string `json:"hub_url"`
}

func (s *UIServer) apiPairClaim(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		writeJSONErr(w, 405, "POST only")
		return
	}
	var req pairClaimRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeJSONErr(w, 400, "bad json: "+err.Error())
		return
	}
	var digits strings.Builder
	for _, ch := range req.Code {
		if ch >= '0' && ch <= '9' {
			digits.WriteRune(ch)
		}
	}
	code := digits.String()
	if len(code) != 6 {
		writeJSONErr(w, 400, fmt.Sprintf("Code must be 6 digits (got %d).", len(code)))
		return
	}
	hubURL := strings.TrimRight(strings.TrimSpace(req.HubURL), "/")
	if hubURL == "" {
		hubURL = "https://hub.snth.ai"
	}

	hubBody, _ := json.Marshal(map[string]string{"code": code})
	hubReq, _ := http.NewRequestWithContext(r.Context(), "POST",
		hubURL+"/api/companion/claim-pair", strings.NewReader(string(hubBody)))
	hubReq.Header.Set("Content-Type", "application/json")
	client := &http.Client{Timeout: 15 * time.Second}
	resp, err := client.Do(hubReq)
	if err != nil {
		writeJSONErr(w, 502, "hub unreachable: "+err.Error())
		return
	}
	defer resp.Body.Close()
	body, _ := io.ReadAll(resp.Body)
	if resp.StatusCode == 404 {
		writeJSONErr(w, 404, "Code not found — expired, used, or wrong.")
		return
	}
	if resp.StatusCode == 410 {
		writeJSONErr(w, 410, "Code expired (5-minute window).")
		return
	}
	if resp.StatusCode/100 != 2 {
		writeJSONErr(w, resp.StatusCode, fmt.Sprintf("hub %d: %s", resp.StatusCode, strings.TrimSpace(string(body))))
		return
	}
	var hubResp struct {
		SynthURL       string `json:"synth_url"`
		SynthID        string `json:"synth_id"`
		CompanionToken string `json:"companion_token"`
	}
	if err := json.Unmarshal(body, &hubResp); err != nil {
		writeJSONErr(w, 500, "parse hub response: "+err.Error())
		return
	}
	if err := config.Update(func(c *config.Config) {
		c.PairedSynthURL = hubResp.SynthURL
		c.PairedSynthID = hubResp.SynthID
		c.CompanionToken = hubResp.CompanionToken
		c.HubURL = hubURL
		c.SandboxRoots = append(c.SandboxRoots[:0], c.SandboxRoots...)
	}); err != nil {
		writeJSONErr(w, 500, "save config: "+err.Error())
		return
	}
	s.Client.Stop()
	s.Client.Start()
	writeJSON(w, 200, map[string]any{
		"ok":        true,
		"synth_id":  hubResp.SynthID,
		"synth_url": hubResp.SynthURL,
	})
}

type pairSaveRequest struct {
	SynthURL string `json:"synth_url"`
	Token    string `json:"token"`
	SynthID  string `json:"synth_id"`
}

func (s *UIServer) apiPairSave(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		writeJSONErr(w, 405, "POST only")
		return
	}
	var req pairSaveRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeJSONErr(w, 400, "bad json: "+err.Error())
		return
	}
	req.SynthURL = strings.TrimSpace(req.SynthURL)
	req.Token = strings.TrimSpace(req.Token)
	req.SynthID = strings.TrimSpace(req.SynthID)
	if req.SynthURL == "" || req.Token == "" || req.SynthID == "" {
		writeJSONErr(w, 400, "synth_url, token, synth_id all required")
		return
	}
	if err := config.Update(func(c *config.Config) {
		c.PairedSynthURL = req.SynthURL
		c.CompanionToken = req.Token
		c.PairedSynthID = req.SynthID
		c.SandboxRoots = append(c.SandboxRoots[:0], c.SandboxRoots...)
	}); err != nil {
		writeJSONErr(w, 500, "save config: "+err.Error())
		return
	}
	s.Client.Stop()
	s.Client.Start()
	writeJSON(w, 200, map[string]any{"ok": true})
}

func (s *UIServer) apiUnpair(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		writeJSONErr(w, 405, "POST only")
		return
	}
	if err := config.Update(func(c *config.Config) {
		c.PairedSynthURL = ""
		c.CompanionToken = ""
		c.PairedSynthID = ""
	}); err != nil {
		writeJSONErr(w, 500, "save config: "+err.Error())
		return
	}
	s.Client.Stop()
	s.Client.Start()
	writeJSON(w, 200, map[string]any{"ok": true})
}

// --- Sandbox --------------------------------------------------------

func (s *UIServer) apiSandbox(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodGet {
		writeJSONErr(w, 405, "GET only")
		return
	}
	cfg := config.Get()
	roots := []string{}
	if cfg != nil {
		roots = append(roots, cfg.SandboxRoots...)
	}
	writeJSON(w, 200, map[string]any{"roots": roots})
}

func (s *UIServer) apiSandboxAdd(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		writeJSONErr(w, 405, "POST only")
		return
	}
	var req struct {
		Path string `json:"path"`
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeJSONErr(w, 400, "bad json: "+err.Error())
		return
	}
	p := strings.TrimSpace(req.Path)
	if p == "" {
		writeJSONErr(w, 400, "path required")
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
	writeJSON(w, 200, map[string]any{"ok": true})
}

func (s *UIServer) apiSandboxRemove(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		writeJSONErr(w, 405, "POST only")
		return
	}
	var req struct {
		Path string `json:"path"`
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeJSONErr(w, 400, "bad json: "+err.Error())
		return
	}
	p := strings.TrimSpace(req.Path)
	_ = config.Update(func(c *config.Config) {
		filtered := c.SandboxRoots[:0]
		for _, root := range c.SandboxRoots {
			if root != p {
				filtered = append(filtered, root)
			}
		}
		c.SandboxRoots = filtered
	})
	writeJSON(w, 200, map[string]any{"ok": true})
}

// --- Codex login ----------------------------------------------------

func (s *UIServer) apiCodexLoginState(w http.ResponseWriter, r *http.Request) {
	codexLogin.mu.Lock()
	defer codexLogin.mu.Unlock()
	out := map[string]any{
		"has_flow":    codexLogin.flow != nil,
		"done":        codexLogin.done,
		"auth_url":    codexLogin.authURL,
		"err":         codexLogin.err,
		"has_result":  codexLogin.result != nil,
		"uploaded_at": nil,
		"upload_err":  codexLogin.uploadErr,
	}
	if codexLogin.result != nil {
		out["account_id"] = codexLogin.result.AccountID
	}
	if !codexLogin.uploadedAt.IsZero() {
		out["uploaded_at"] = codexLogin.uploadedAt.Format(time.RFC3339)
	}
	if !codexLogin.started.IsZero() {
		out["started_at"] = codexLogin.started.Format(time.RFC3339)
	}
	writeJSON(w, 200, out)
}

func (s *UIServer) apiCodexLoginStart(w http.ResponseWriter, r *http.Request) {
	// Re-use the existing handler — it already wraps start + browser open
	// + background Finish() + auto-upload. The HTML it returns is a
	// 303 redirect; the SPA treats 200/303 as "kicked off" and polls
	// /api/codex-login/state.
	s.handleCodexLoginStart(w, r)
}

func (s *UIServer) apiCodexLoginUpload(w http.ResponseWriter, r *http.Request) {
	s.handleCodexLoginUpload(w, r)
}

func (s *UIServer) apiCodexLoginClear(w http.ResponseWriter, r *http.Request) {
	s.handleCodexLoginClear(w, r)
}

// --- Tools catalog (separate from /api/status.tools so UI can show
// it independently of the WS state) ---------------------------------

func (s *UIServer) apiTools(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodGet {
		writeJSONErr(w, 405, "GET only")
		return
	}
	cat := tools.Catalog()
	// Cross-reference the audit log so the SPA can render per-tool
	// last-invocation outcome + counts.
	audit := RecentAudit(500)
	type toolStat struct {
		Last     *AuditEntry `json:"last,omitempty"`
		Calls    int         `json:"calls"`
		Errors   int         `json:"errors"`
	}
	stats := map[string]toolStat{}
	for _, e := range audit {
		st := stats[e.Tool]
		st.Calls++
		if e.Outcome == "error" {
			st.Errors++
		}
		if st.Last == nil || e.StartedAt.After(st.Last.StartedAt) {
			entry := e
			st.Last = &entry
		}
		stats[e.Tool] = st
	}
	type toolOut struct {
		Name        string   `json:"name"`
		Description string   `json:"description"`
		DangerLevel string   `json:"danger_level"`
		Stat        toolStat `json:"stat"`
	}
	out := make([]toolOut, 0, len(cat))
	for _, d := range cat {
		out = append(out, toolOut{
			Name:        d.Name,
			Description: d.Description,
			DangerLevel: d.DangerLevel,
			Stat:        stats[d.Name],
		})
	}
	writeJSON(w, 200, map[string]any{"tools": out})
}

// writeJSON is a small helper — sets header, writes status, encodes.
func writeJSON(w http.ResponseWriter, status int, v any) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(v)
}
