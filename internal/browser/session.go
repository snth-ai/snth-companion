package browser

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"strings"
	"sync"
	"time"
)

// Session is the high-level handle callers use. One Session per
// companion process. It maintains attach-state to the user's Chrome
// at DebuggerPort, re-discovers the current active target on demand,
// and owns a per-target CDP connection cache.
//
// We don't launch Chrome ourselves. The user starts their normal
// Chrome with --remote-debugging-port=9222 (see docs); Session just
// connects. Rationale: attaching to the user's real browser session
// means the synth sees their logins, bookmarks, and what they're
// actually doing, same as OpenClaw's "attach" mode.

// DebuggerPort is the port the companion expects Chrome to expose.
// Hard-coded to 9222 in v1; future config option if users want to
// override.
const DebuggerPort = 9222

// Target describes one tab / worker / browser-level debuggee as
// returned by Chrome's /json/list endpoint.
type Target struct {
	ID                   string `json:"id"`
	Type                 string `json:"type"` // "page" | "background_page" | "service_worker" | "shared_worker" | "browser" | "iframe"
	Title                string `json:"title"`
	URL                  string `json:"url"`
	WebSocketDebuggerURL string `json:"webSocketDebuggerUrl"`
	DevtoolsFrontendURL  string `json:"devtoolsFrontendUrl,omitempty"`
}

// Version is what /json/version returns. Useful for surfacing
// Chrome / Brave / Arc to the user.
type Version struct {
	Browser              string `json:"Browser"`
	ProtocolVersion      string `json:"Protocol-Version"`
	UserAgent            string `json:"User-Agent"`
	V8Version            string `json:"V8-Version"`
	WebKitVersion        string `json:"WebKit-Version"`
	WebSocketDebuggerURL string `json:"webSocketDebuggerUrl"`
}

// Session caches per-target CDP handles and owns the extension
// relay. AttachActive returns the best available transport in
// priority order:
//
//  1. Live extension connection (via Relay, port 18792). Preferred
//     because the user's real Chrome profile is used.
//  2. Direct --remote-debugging-port=9222 attach. Fallback for when
//     the user launched Chrome manually with the debug flag.
type Session struct {
	Host string // default "127.0.0.1"
	Port int    // default DebuggerPort (9222)

	Relay *RelayServer // non-nil → prefer extension over port

	mu   sync.Mutex
	cdps map[string]*CDP // targetID → connected cdp
}

func NewSession() *Session {
	return &Session{
		Host: "127.0.0.1",
		Port: DebuggerPort,
		cdps: map[string]*CDP{},
	}
}

// WithRelay attaches an already-started RelayServer so Conn-returning
// methods can prefer it over the debug-port path.
func (s *Session) WithRelay(r *RelayServer) *Session {
	s.Relay = r
	return s
}

// baseURL returns http://host:port (no trailing slash).
func (s *Session) baseURL() string {
	return fmt.Sprintf("http://%s:%d", s.Host, s.Port)
}

// Version hits /json/version; returns a friendly error when Chrome
// isn't reachable so the tool can tell the LLM what's up.
func (s *Session) Version(ctx context.Context) (*Version, error) {
	resp, err := httpGet(ctx, s.baseURL()+"/json/version")
	if err != nil {
		return nil, fmt.Errorf("chrome not reachable at %s — launch Chrome with --remote-debugging-port=%d (see snth-companion README). Raw: %w",
			s.baseURL(), s.Port, err)
	}
	defer resp.Body.Close()
	body, _ := io.ReadAll(resp.Body)
	if resp.StatusCode != 200 {
		return nil, fmt.Errorf("chrome /json/version: %d %s", resp.StatusCode, string(body))
	}
	var v Version
	if err := json.Unmarshal(body, &v); err != nil {
		return nil, fmt.Errorf("decode /json/version: %w", err)
	}
	return &v, nil
}

// Targets lists all debuggable targets.
func (s *Session) Targets(ctx context.Context) ([]Target, error) {
	resp, err := httpGet(ctx, s.baseURL()+"/json/list")
	if err != nil {
		return nil, fmt.Errorf("chrome /json/list unreachable: %w", err)
	}
	defer resp.Body.Close()
	body, _ := io.ReadAll(resp.Body)
	if resp.StatusCode != 200 {
		return nil, fmt.Errorf("chrome /json/list: %d %s", resp.StatusCode, string(body))
	}
	var targets []Target
	if err := json.Unmarshal(body, &targets); err != nil {
		return nil, fmt.Errorf("decode /json/list: %w", err)
	}
	return targets, nil
}

// ActivePage returns the most-recently-focused page target, or the
// first page target if none is clearly active. Errors when no
// pages exist.
//
// /json/list sorts by recent activity, so the first type="page"
// entry is usually the active tab.
func (s *Session) ActivePage(ctx context.Context) (*Target, error) {
	ts, err := s.Targets(ctx)
	if err != nil {
		return nil, err
	}
	for _, t := range ts {
		if t.Type == "page" && !strings.HasPrefix(t.URL, "devtools://") {
			tt := t
			return &tt, nil
		}
	}
	return nil, fmt.Errorf("no page targets — open a tab in Chrome first")
}

// Attach opens (or returns cached) CDP connection for the given
// target ID.
func (s *Session) Attach(ctx context.Context, t *Target) (*CDP, error) {
	s.mu.Lock()
	if c, ok := s.cdps[t.ID]; ok && !c.closed.Load() {
		s.mu.Unlock()
		return c, nil
	}
	delete(s.cdps, t.ID)
	s.mu.Unlock()

	c, err := Dial(ctx, t.WebSocketDebuggerURL)
	if err != nil {
		return nil, err
	}

	s.mu.Lock()
	s.cdps[t.ID] = c
	s.mu.Unlock()
	return c, nil
}

// AttachActive returns a ready-to-use Conn. Prefers the extension
// relay when connected; falls back to direct debug-port CDP.
// The Target is nil when we're talking through the relay (the
// extension chose which tab when the user clicked its icon).
func (s *Session) AttachActive(ctx context.Context) (*Target, Conn, error) {
	if s.Relay != nil {
		if client := s.Relay.Client(); client != nil {
			return nil, client, nil
		}
	}
	t, err := s.ActivePage(ctx)
	if err != nil {
		if s.Relay != nil {
			return nil, nil, fmt.Errorf("%w — or click the SNTH Companion Relay extension icon in Chrome to attach a tab", err)
		}
		return nil, nil, err
	}
	c, err := s.Attach(ctx, t)
	if err != nil {
		return nil, nil, err
	}
	return t, c, nil
}

// DropTarget closes and drops the cached CDP for targetID. Called
// after a Page.navigate in case we want a fresh attach, or on error.
func (s *Session) DropTarget(targetID string) {
	s.mu.Lock()
	c, ok := s.cdps[targetID]
	delete(s.cdps, targetID)
	s.mu.Unlock()
	if ok {
		c.Close()
	}
}

// CloseAll closes every cached CDP. Called during companion
// shutdown.
func (s *Session) CloseAll() {
	s.mu.Lock()
	for id, c := range s.cdps {
		c.Close()
		delete(s.cdps, id)
	}
	s.mu.Unlock()
}

// httpGet is a context-scoped GET with a 5s default timeout if ctx
// carries none.
func httpGet(ctx context.Context, url string) (*http.Response, error) {
	if _, ok := ctx.Deadline(); !ok {
		var cancel context.CancelFunc
		ctx, cancel = context.WithTimeout(ctx, 5*time.Second)
		defer cancel()
	}
	req, err := http.NewRequestWithContext(ctx, "GET", url, nil)
	if err != nil {
		return nil, err
	}
	return http.DefaultClient.Do(req)
}
