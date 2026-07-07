package daemon

import (
	"context"
	"encoding/json"
	"fmt"
	"log"
	"net/http"
	"net/url"
	"os"
	"strings"
	"sync"
	"time"

	"github.com/gorilla/websocket"

	"github.com/snth-ai/snth-companion/internal/config"
	"github.com/snth-ai/snth-companion/internal/tools"
)

// Version is stamped at build time via -ldflags, falls back to "dev".
var Version = "dev"

// pingInterval is how often the client sends an application-level ping.
// The read deadline (D4) is set to 2× this so a half-open TCP surfaces
// within ~2 ping cycles instead of relying on a ping WRITE eventually
// failing (which can take ~15 min on a half-open socket).
const pingInterval = 20 * time.Second

// Client is the persistent connection to a paired synth. Zero-value is
// usable after calling Start; it maintains a reconnect loop until Stop is
// called.
type Client struct {
	mu       sync.Mutex
	ws       *websocket.Conn
	status   string // "disconnected" | "connecting" | "connected" | "paused"
	lastErr  error
	lastSeen time.Time
	stopCh   chan struct{}
	doneCh   chan struct{}
	stopping bool // set under mu when Stop() has fired; runOnce treats a
	// conn close as terminal (don't reconnect).

	// writeMu serializes ALL frame writes to c.ws (D1). gorilla/websocket
	// forbids concurrent writers; three goroutines write the same conn
	// (tool_result, ping, read-loop pong). Kept SEPARATE from c.mu, which
	// guards state — a slow write must not block a Status() read.
	writeMu sync.Mutex
}

// writeJSON is the ONE serialized write path for every outbound frame
// (D1). Callers pass the conn they believe is current; if it no longer
// matches c.ws the write is dropped (the conn is being torn down). Holds
// writeMu for the duration of the marshal+write so no two goroutines
// interleave on the same *websocket.Conn.
func (c *Client) writeJSON(conn *websocket.Conn, v any) error {
	c.mu.Lock()
	cur := c.ws
	c.mu.Unlock()
	if conn == nil || cur != conn {
		return fmt.Errorf("connection changed")
	}
	c.writeMu.Lock()
	defer c.writeMu.Unlock()
	return conn.WriteJSON(v)
}

// Status is a read-only snapshot for the UI.
type Status struct {
	Status   string    `json:"status"`
	LastErr  string    `json:"last_error,omitempty"`
	LastSeen time.Time `json:"last_seen,omitempty"`
}

func (c *Client) Status() Status {
	c.mu.Lock()
	defer c.mu.Unlock()
	s := Status{Status: c.status, LastSeen: c.lastSeen}
	if c.lastErr != nil {
		s.LastErr = c.lastErr.Error()
	}
	return s
}

func (c *Client) setStatus(st string, err error) {
	c.mu.Lock()
	c.status = st
	c.lastErr = err
	if st == "connected" {
		c.lastSeen = time.Now()
	}
	c.mu.Unlock()
}

// Start kicks off the reconnect loop. Non-blocking. Idempotent.
func (c *Client) Start() {
	c.mu.Lock()
	if c.stopCh != nil {
		c.mu.Unlock()
		return
	}
	c.stopCh = make(chan struct{})
	c.doneCh = make(chan struct{})
	c.mu.Unlock()
	go c.loop()
}

// Stop closes the connection and prevents further reconnects. It closes
// the active conn so a blocked ReadJSON returns immediately (D2), then
// waits on doneCh with a bounded timeout so a wedged read can never hang
// the caller forever.
func (c *Client) Stop() {
	c.mu.Lock()
	if c.stopCh == nil {
		c.mu.Unlock()
		return
	}
	c.stopping = true
	close(c.stopCh)
	stop := c.doneCh
	ws := c.ws
	c.stopCh = nil
	c.mu.Unlock()

	// Unblock a ReadJSON parked with no deadline: closing the conn makes
	// the read return an error, so runOnce/loop can observe stopCh.
	if ws != nil {
		_ = ws.Close()
	}

	select {
	case <-stop:
	case <-time.After(5 * time.Second):
		log.Printf("[ws] Stop: loop did not exit within 5s, abandoning")
	}

	c.mu.Lock()
	c.stopping = false
	c.mu.Unlock()
}

// healthyResetThreshold — a connection that stayed up at least this long
// before dropping is treated as "healthy": the next reconnect starts from
// the base backoff (D3) instead of continuing to climb toward the 30s cap.
const healthyResetThreshold = 60 * time.Second

func (c *Client) loop() {
	defer close(c.doneCh)
	// Capture stopCh once: Stop() sets c.stopCh=nil after closing it, and a
	// receive on a nil channel blocks forever. Reading the field directly in
	// the selects below would deadlock the reconnect wait after Stop nils it.
	c.mu.Lock()
	stopCh := c.stopCh
	c.mu.Unlock()

	backoff := time.Second
	for {
		select {
		case <-stopCh:
			return
		default:
		}

		cfg := config.Get()
		if cfg == nil || cfg.PairedSynthURL == "" || cfg.CompanionToken == "" {
			c.setStatus("paused", fmt.Errorf("not paired"))
			select {
			case <-stopCh:
				return
			case <-time.After(5 * time.Second):
			}
			continue
		}

		c.setStatus("connecting", nil)
		connectedFor, err := c.runOnce(cfg.PairedSynthURL, cfg.CompanionToken)

		// A Stop-initiated close is terminal — do not reconnect (D2).
		select {
		case <-stopCh:
			return
		default:
		}

		if err != nil {
			log.Printf("[ws] disconnected: %v", err)
			c.setStatus("disconnected", err)
		}

		// D3: reset backoff after a healthy session so a single blip
		// doesn't pin the reconnect delay at the 30s cap for life.
		if connectedFor >= healthyResetThreshold {
			backoff = time.Second
		}

		select {
		case <-stopCh:
			return
		case <-time.After(backoff):
		}
		backoff *= 2
		if backoff > 30*time.Second {
			backoff = 30 * time.Second
		}
	}
}

// runOnce dials, handshakes, and services the connection until it drops.
// It returns how long the connection was fully established (used by loop
// to decide whether to reset the reconnect backoff — D3) and the error
// that ended it (nil on a clean Stop-initiated close).
func (c *Client) runOnce(synthURL, token string) (time.Duration, error) {
	wsURL, err := toWSURL(synthURL, "/api/companion/ws")
	if err != nil {
		return 0, err
	}
	hdr := http.Header{}
	hdr.Set("Authorization", "Bearer "+token)
	hdr.Set("User-Agent", "snth-companion/"+Version)

	dialer := *websocket.DefaultDialer
	dialer.HandshakeTimeout = 15 * time.Second
	conn, _, err := dialer.Dial(wsURL, hdr)
	if err != nil {
		return 0, fmt.Errorf("dial %s: %w", wsURL, err)
	}
	c.mu.Lock()
	c.ws = conn
	c.mu.Unlock()
	defer func() {
		c.mu.Lock()
		c.ws = nil
		c.mu.Unlock()
		conn.Close()
	}()

	// Send hello with our capabilities + multi-companion identity.
	catalog := tools.Catalog()
	caps := make([]ToolDesc, len(catalog))
	for i, d := range catalog {
		caps[i] = ToolDesc{Name: d.Name, Description: d.Description, DangerLevel: d.DangerLevel}
	}
	cfg := config.Get()
	role := ""
	tags := []string(nil)
	deviceID := ""
	if cfg != nil {
		role = string(cfg.CompanionRole)
		tags = append(tags, cfg.CompanionTags...)
	}
	if h, err := os.Hostname(); err == nil {
		deviceID = h
	}
	if err := c.writeJSON(conn, Frame{
		Type:              FrameHello,
		CompanionVersion:  Version,
		Capabilities:      caps,
		CompanionRole:     role,
		CompanionTags:     tags,
		CompanionDeviceID: deviceID,
	}); err != nil {
		return 0, fmt.Errorf("send hello: %w", err)
	}

	// Expect welcome.
	var welcome Frame
	_ = conn.SetReadDeadline(time.Now().Add(10 * time.Second))
	if err := conn.ReadJSON(&welcome); err != nil {
		return 0, fmt.Errorf("read welcome: %w", err)
	}
	if welcome.Type != FrameWelcome {
		return 0, fmt.Errorf("expected welcome, got %q", welcome.Type)
	}

	connectedAt := time.Now()
	c.setStatus("connected", nil)
	log.Printf("[ws] connected to %s (synth=%s v=%s, tools=%d)",
		wsURL, welcome.SynthID, welcome.SynthVersion, len(caps))

	// Ping loop keeps idle connections alive and surfaces dead ones.
	pingDone := make(chan struct{})
	go c.pingLoop(conn, pingDone)
	defer close(pingDone)

	// D4: rolling read deadline. Every received frame (any type — a pong,
	// a tool_call, a server ping) proves the peer is alive, so we refresh
	// the deadline on each read. A silent half-open connection then trips
	// the deadline within ~2 ping intervals instead of ~15 min.
	readTimeout := 2 * pingInterval
	_ = conn.SetReadDeadline(time.Now().Add(readTimeout))

	// Read loop dispatches tool calls.
	for {
		var frame Frame
		if err := conn.ReadJSON(&frame); err != nil {
			// A Stop-initiated close surfaces here as a read error; report
			// it as a clean (nil) end so loop doesn't log a scary line and
			// treats it as terminal via the stopCh check.
			c.mu.Lock()
			stopping := c.stopping
			c.mu.Unlock()
			if stopping {
				return time.Since(connectedAt), nil
			}
			return time.Since(connectedAt), fmt.Errorf("read: %w", err)
		}
		_ = conn.SetReadDeadline(time.Now().Add(readTimeout))
		switch frame.Type {
		case FrameToolCall:
			go c.handleToolCall(frame)
		case FramePing:
			_ = c.writeJSON(conn, Frame{Type: FramePong})
		case FramePong:
			// noop — the deadline refresh above already registered liveness
		case FrameError:
			return time.Since(connectedAt), fmt.Errorf("server error: %s", frame.Error)
		default:
			log.Printf("[ws] unknown frame type: %q", frame.Type)
		}
	}
}

func (c *Client) pingLoop(conn *websocket.Conn, done chan struct{}) {
	t := time.NewTicker(pingInterval)
	defer t.Stop()
	for {
		select {
		case <-done:
			return
		case <-t.C:
			if err := c.writeJSON(conn, Frame{Type: FramePing}); err != nil {
				return
			}
		}
	}
}

// toolCallTimeout is the per-tool ceiling handleToolCall applies to a
// dispatched call. Most tools finish in seconds; the default cap is 5 min.
// remote_subagent (G1) drives a 30-60 min CLI delegation and derives its
// OWN ctx.WithTimeout from the ctx we pass — so a 5-min cap here would
// SIGKILL it at ~5 min. It gets a long ceiling that comfortably exceeds
// its internal 60-min hard cap. Cancellation on Stop/disconnect is
// preserved: the parent ctx is still cancelled when the client tears down.
func toolCallTimeout(tool string) time.Duration {
	switch tool {
	case "remote_subagent":
		return 65 * time.Minute
	default:
		return 5 * time.Minute
	}
}

func (c *Client) handleToolCall(frame Frame) {
	ctx, cancel := context.WithTimeout(context.Background(), toolCallTimeout(frame.Tool))
	defer cancel()

	start := time.Now()
	resp := Frame{
		Type:   FrameToolResult,
		CallID: frame.CallID,
	}
	data, err := tools.Dispatch(ctx, frame.Tool, frame.Args)
	duration := time.Since(start)

	entry := AuditEntry{
		StartedAt:   start,
		DurationMs:  duration.Milliseconds(),
		Tool:        frame.Tool,
		ArgsSummary: summarizeJSON(frame.Args, 200),
	}
	if err != nil {
		resp.Error = err.Error()
		entry.Outcome = "error"
		entry.Error = err.Error()
		if err.Error() == "user denied" {
			entry.Outcome = "denied"
		}
	} else {
		raw, mErr := json.Marshal(data)
		if mErr != nil {
			resp.Error = fmt.Sprintf("marshal result: %s", mErr)
			entry.Outcome = "error"
			entry.Error = resp.Error
		} else {
			resp.Data = raw
			entry.Outcome = "ok"
		}
	}
	RecordAudit(entry)

	c.mu.Lock()
	ws := c.ws
	c.mu.Unlock()
	if ws == nil {
		log.Printf("[ws] dropped tool_result for %s: no connection", frame.CallID)
		return
	}
	if err := c.writeJSON(ws, resp); err != nil {
		log.Printf("[ws] write tool_result %s: %v", frame.CallID, err)
	}
}

// summarizeJSON returns the raw JSON truncated to n runes (inclusive
// of the ellipsis). Used for the audit log's args preview — we don't
// want to store full 4 MiB file contents in an in-memory ring.
func summarizeJSON(raw json.RawMessage, n int) string {
	s := string(raw)
	if len(s) <= n {
		return s
	}
	return s[:n-1] + "…"
}

// toWSURL converts an https://… synth URL to wss://…/path, or http→ws.
func toWSURL(base, path string) (string, error) {
	u, err := url.Parse(base)
	if err != nil {
		return "", err
	}
	switch strings.ToLower(u.Scheme) {
	case "https":
		u.Scheme = "wss"
	case "http":
		u.Scheme = "ws"
	case "wss", "ws":
		// already good
	default:
		return "", fmt.Errorf("unsupported scheme %q", u.Scheme)
	}
	u.Path = strings.TrimRight(u.Path, "/") + path
	return u.String(), nil
}
