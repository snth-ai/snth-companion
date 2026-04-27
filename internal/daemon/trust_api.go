package daemon

// trust_api.go — JSON endpoints powering the Privacy Center UI.
//
// Routes:
//   GET    /api/trust              — current state + tool catalog
//   POST   /api/trust/master       — flip master toggle (with optional expiry)
//   POST   /api/trust/tool         — set per-tool mode
//   POST   /api/trust/revoke-all   — kill switch (everything → prompt)
//   POST   /api/trust/path         — add/remove a write-root scope
//
// All routes are mounted under the existing localhost-only mux, so no
// auth beyond the loopback bind is needed; that mirrors how /api/sandbox
// already works.

import (
	"encoding/json"
	"net/http"
	"sort"
	"strings"
	"time"

	"github.com/snth-ai/snth-companion/internal/trust"
)

// trustStore is the package-global handle injected at boot via
// SetTrustStore from main. nil-safe — endpoints reply 503 when unset
// (e.g. running tests without the real boot wiring).
var pkgTrustStore *trust.Store

func SetTrustStore(s *trust.Store) { pkgTrustStore = s }

func (s *UIServer) registerTrustAPIs(mux *http.ServeMux) {
	mux.HandleFunc("/api/trust", s.apiTrustGet)
	mux.HandleFunc("/api/trust/master", s.apiTrustMaster)
	mux.HandleFunc("/api/trust/tool", s.apiTrustTool)
	mux.HandleFunc("/api/trust/revoke-all", s.apiTrustRevokeAll)
	mux.HandleFunc("/api/trust/path", s.apiTrustPath)
	mux.HandleFunc("/api/trust/audit", s.apiTrustAudit)
}

// trustToolDef is the catalog entry shown to the UI per tool: id,
// human label, danger level, whether it's in the always-prompt set.
type trustToolDef struct {
	ID           string `json:"id"`
	Label        string `json:"label"`
	Danger       string `json:"danger"`         // "safe" | "prompt" | "always-prompt"
	AlwaysPrompt bool   `json:"always_prompt"`  // listed in trust.AlwaysPromptTools
	CurrentMode  string `json:"current_mode"`   // "prompt" | "trusted" | "denied"
	Description  string `json:"description"`
}

// trustCatalog — the canonical list shown in the Privacy UI. We keep it
// here so the SPA never has to second-guess. Order matters: more
// dangerous up top, safer at the bottom.
var trustCatalog = []trustToolDef{
	{ID: "subagent", Label: "Subagent (Claude/Codex CLI)", Danger: "always-prompt", AlwaysPrompt: true, Description: "Spawns Claude Code or Codex CLI on the user's Mac with --dangerously-skip-permissions; can read/write any file in the cwd. Long-running (up to 60 min) and uses CLI subscription."},
	{ID: "messages_send", Label: "iMessage send", Danger: "always-prompt", AlwaysPrompt: true, Description: "Sends a message from the user's iMessage app — visible to recipients as if the user typed it."},
	{ID: "messages_recent", Label: "iMessage history read", Danger: "always-prompt", Description: "Reads recent iMessage chat history. Requires Full Disk Access granted to the companion."},
	{ID: "remote_browser", Label: "Browser (Chrome CDP)", Danger: "always-prompt", Description: "Drives the user's Chrome via CDP — clicks, navigates, extracts DOM. Touches the user's real session and cookies."},
	{ID: "remote_bash", Label: "Bash command", Danger: "prompt", Description: "Runs a shell command in the configured sandbox roots. Out-of-sandbox commands always-prompt regardless of trust mode."},
	{ID: "remote_fs_write", Label: "Filesystem write", Danger: "prompt", Description: "Writes a file. Honors per-path scopes when set. Out-of-sandbox writes always-prompt regardless of trust mode."},
	{ID: "remote_fs_read", Label: "Filesystem read", Danger: "prompt", Description: "Reads a file outside the configured sandbox roots."},
	{ID: "calendar_create", Label: "Calendar event create", Danger: "prompt", Description: "Adds an event to Apple Calendar."},
	{ID: "notes_create", Label: "Notes create", Danger: "prompt", Description: "Creates a new Apple Note."},
	{ID: "notes_read", Label: "Notes read", Danger: "prompt", Description: "Reads an existing Apple Note."},
	{ID: "reminders_create", Label: "Reminder create", Danger: "prompt", Description: "Adds a reminder to Apple Reminders."},
	{ID: "reminders_complete", Label: "Reminder complete", Danger: "prompt", Description: "Marks an existing reminder as completed."},
	{ID: "shortcut_run", Label: "Shortcut run", Danger: "prompt", Description: "Runs an Apple Shortcuts shortcut by name. First run prompts; subsequent runs of the same shortcut are remembered for the session."},
	{ID: "clipboard_read", Label: "Clipboard read", Danger: "prompt", Description: "Reads current clipboard contents."},
	{ID: "clipboard_write", Label: "Clipboard write", Danger: "prompt", Description: "Replaces clipboard contents."},
}

// trustGetResponse is the shape consumed by the SPA Privacy page.
type trustGetResponse struct {
	State trust.State    `json:"state"`
	Tools []trustToolDef `json:"tools"`
}

func (s *UIServer) apiTrustGet(w http.ResponseWriter, r *http.Request) {
	if pkgTrustStore == nil {
		writeJSONErr(w, 503, "trust store not initialized")
		return
	}
	st := pkgTrustStore.Snapshot()
	cat := make([]trustToolDef, len(trustCatalog))
	copy(cat, trustCatalog)
	for i, t := range cat {
		mode := string(trust.ModePrompt)
		if m, ok := st.Tools[t.ID]; ok {
			mode = string(m)
		}
		cat[i].CurrentMode = mode
	}
	sort.SliceStable(cat, func(i, j int) bool {
		if cat[i].AlwaysPrompt != cat[j].AlwaysPrompt {
			return cat[i].AlwaysPrompt
		}
		return cat[i].ID < cat[j].ID
	})
	writeJSON(w, 200, trustGetResponse{State: st, Tools: cat})
}

type trustMasterRequest struct {
	On      bool   `json:"on"`
	Expires string `json:"expires,omitempty"` // RFC3339 absolute, or "" = no expiry
}

func (s *UIServer) apiTrustMaster(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		writeJSONErr(w, 405, "POST only")
		return
	}
	if pkgTrustStore == nil {
		writeJSONErr(w, 503, "trust store not initialized")
		return
	}
	var req trustMasterRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeJSONErr(w, 400, "bad json: "+err.Error())
		return
	}
	var exp *time.Time
	if s := strings.TrimSpace(req.Expires); s != "" {
		t, err := time.Parse(time.RFC3339, s)
		if err != nil {
			writeJSONErr(w, 400, "expires must be RFC3339: "+err.Error())
			return
		}
		exp = &t
	}
	if err := pkgTrustStore.SetMaster(req.On, exp); err != nil {
		writeJSONErr(w, 500, err.Error())
		return
	}
	writeJSON(w, 200, map[string]any{"ok": true})
}

type trustToolRequest struct {
	Tool string `json:"tool"`
	Mode string `json:"mode"` // "prompt" | "trusted" | "denied"
}

func (s *UIServer) apiTrustTool(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		writeJSONErr(w, 405, "POST only")
		return
	}
	if pkgTrustStore == nil {
		writeJSONErr(w, 503, "trust store not initialized")
		return
	}
	var req trustToolRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeJSONErr(w, 400, "bad json: "+err.Error())
		return
	}
	mode := trust.ToolMode(req.Mode)
	switch mode {
	case trust.ModePrompt, trust.ModeTrusted, trust.ModeDenied:
	default:
		writeJSONErr(w, 400, "mode must be prompt|trusted|denied")
		return
	}
	if req.Tool == "" {
		writeJSONErr(w, 400, "tool required")
		return
	}
	if err := pkgTrustStore.SetTool(req.Tool, mode); err != nil {
		writeJSONErr(w, 500, err.Error())
		return
	}
	writeJSON(w, 200, map[string]any{"ok": true})
}

func (s *UIServer) apiTrustRevokeAll(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		writeJSONErr(w, 405, "POST only")
		return
	}
	if pkgTrustStore == nil {
		writeJSONErr(w, 503, "trust store not initialized")
		return
	}
	if err := pkgTrustStore.RevokeAll(); err != nil {
		writeJSONErr(w, 500, err.Error())
		return
	}
	writeJSON(w, 200, map[string]any{"ok": true})
}

type trustPathRequest struct {
	Op   string `json:"op"`   // "add" | "remove"
	Path string `json:"path"`
}

func (s *UIServer) apiTrustPath(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		writeJSONErr(w, 405, "POST only")
		return
	}
	if pkgTrustStore == nil {
		writeJSONErr(w, 503, "trust store not initialized")
		return
	}
	var req trustPathRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeJSONErr(w, 400, "bad json: "+err.Error())
		return
	}
	switch req.Op {
	case "add":
		if err := pkgTrustStore.AddWriteRoot(req.Path); err != nil {
			writeJSONErr(w, 500, err.Error())
			return
		}
	case "remove":
		if err := pkgTrustStore.RemoveWriteRoot(req.Path); err != nil {
			writeJSONErr(w, 500, err.Error())
			return
		}
	default:
		writeJSONErr(w, 400, "op must be add|remove")
		return
	}
	writeJSON(w, 200, map[string]any{"ok": true})
}

// apiTrustAudit returns the most recent decisions filtered to those
// emitted by the approval layer (Outcome="approval"). Pure passthrough
// to the existing audit ring buffer — this endpoint just gives the
// SPA a typed slice to render in the Privacy page audit feed.
func (s *UIServer) apiTrustAudit(w http.ResponseWriter, r *http.Request) {
	all := RecentAudit(200)
	out := make([]AuditEntry, 0, len(all))
	for _, e := range all {
		if e.Outcome == "approval" {
			out = append(out, e)
		}
	}
	writeJSON(w, 200, map[string]any{"entries": out})
}
