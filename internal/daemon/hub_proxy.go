package daemon

// hub_proxy.go — thin JSON proxies that bridge the React SPA to the
// hub's pair-token-scoped /api/my/* endpoints. The SPA runs inside
// the WebView on the same origin as the Go server; the CompanionToken
// is stored in config.json and MUST NOT land in client JS. These
// handlers attach it server-side so the browser never sees it.
//
// Every proxy follows the same shape: read `{method}` + `{path}`,
// copy the request body (if POST), add Authorization header, forward,
// stream response back. Matches the hub URL we persisted at
// pair-claim time.

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"strings"
	"time"

	"github.com/snth-ai/snth-companion/internal/config"
)

// registerHubProxies wires every hub-side /api/my/* endpoint we want
// the SPA to reach, under /api/hub/*. Pattern keeps the set mapping
// obvious + easy to audit.
//
// React hits:        /api/hub/channel-settings
// Proxy forwards to: $HUB_URL/api/my/channel-settings
func (s *UIServer) registerHubProxies(mux *http.ServeMux) {
	proxy := func(pattern, hubPath string) {
		mux.HandleFunc(pattern, func(w http.ResponseWriter, r *http.Request) {
			s.proxyToHub(w, r, hubPath)
		})
	}
	proxy("/api/hub/whoami", "/api/my/whoami")
	proxy("/api/hub/channel-settings", "/api/my/channel-settings")
	proxy("/api/hub/llm-config", "/api/my/llm-config")
	proxy("/api/hub/provider-catalog", "/api/my/provider-catalog")
	proxy("/api/hub/provider-key", "/api/my/provider-key")
	proxy("/api/hub/codex-creds", "/api/my/codex-creds")
	proxy("/api/hub/logs-remote", "/api/my/logs")
	proxy("/api/hub/synth-tools", "/api/my/tools")
	proxy("/api/hub/synth-tools/toggle", "/api/my/tools/toggle")
	// Mini-apps surface (Wave 9, v0.4.39+ companion). The bound synth
	// serves the actual content; hub bridges, companion sandboxes the
	// iframe + handles the $mini postMessage bridge in the UI shell.
	proxy("/api/hub/mini-apps", "/api/my/mini-apps")
	proxy("/api/hub/mini-apps/ask", "/api/my/mini-apps/ask")
	proxy("/api/hub/synth-fetch", "/api/my/synth-fetch")
	// /api/hub/mini-app/<slug>[/<path>] is registered separately
	// because path-prefix proxy semantics differ — see
	// registerHubProxyPrefix below.
	s.registerHubProxyPrefix(mux, "/api/hub/mini-app/", "/api/my/mini-app/")
}

// registerHubProxyPrefix wires a directory-style proxy that preserves
// the path tail (everything after `prefix` is appended to `hubPrefix`).
// Used by mini-app asset serving where the iframe references nested
// paths like /api/hub/mini-app/<slug>/foo.png.
func (s *UIServer) registerHubProxyPrefix(mux *http.ServeMux, prefix, hubPrefix string) {
	mux.HandleFunc(prefix, func(w http.ResponseWriter, r *http.Request) {
		tail := strings.TrimPrefix(r.URL.Path, prefix)
		s.proxyToHub(w, r, hubPrefix+tail)
	})
}

// proxyToHub forwards r to $HUB_URL+hubPath with companion token
// injected. On config misses (no paired synth or no HubURL) returns
// a structured JSON error so the SPA can show it nicely without
// triggering a generic 502.
func (s *UIServer) proxyToHub(w http.ResponseWriter, r *http.Request, hubPath string) {
	cfg := config.Get()
	if cfg == nil || cfg.CompanionToken == "" {
		writeJSONErr(w, 400, "not paired — pair the companion first")
		return
	}
	hubURL := cfg.HubURL
	if hubURL == "" {
		hubURL = "https://hub.snth.ai"
	}
	target := strings.TrimRight(hubURL, "/") + hubPath
	// Preserve query string (lines=N etc.).
	if r.URL.RawQuery != "" {
		target += "?" + r.URL.RawQuery
	}

	var body io.Reader
	if r.Method != http.MethodGet && r.ContentLength != 0 {
		buf, _ := io.ReadAll(r.Body)
		body = bytes.NewReader(buf)
	}
	req, err := http.NewRequest(r.Method, target, body)
	if err != nil {
		writeJSONErr(w, 500, "build request: "+err.Error())
		return
	}
	req.Header.Set("Authorization", "Bearer "+cfg.CompanionToken)
	// Forward Content-Type so hub sees JSON bodies correctly.
	if ct := r.Header.Get("Content-Type"); ct != "" {
		req.Header.Set("Content-Type", ct)
	}
	client := &http.Client{Timeout: 60 * time.Second}
	resp, err := client.Do(req)
	if err != nil {
		writeJSONErr(w, 502, "hub unreachable: "+err.Error())
		return
	}
	defer resp.Body.Close()
	// Mirror status + content-type; stream body.
	if ct := resp.Header.Get("Content-Type"); ct != "" {
		w.Header().Set("Content-Type", ct)
	} else {
		w.Header().Set("Content-Type", "application/json")
	}
	w.WriteHeader(resp.StatusCode)
	_, _ = io.Copy(w, resp.Body)
}

// writeJSONErr writes a short JSON envelope with the standard shape
// the SPA's error toasts look for.
func writeJSONErr(w http.ResponseWriter, status int, msg string) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(map[string]any{
		"ok":    false,
		"error": msg,
	})
	_ = fmt.Sprint // keep fmt if error-building grows later
}
