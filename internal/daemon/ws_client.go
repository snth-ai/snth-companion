package daemon

import (
	"context"
	"encoding/json"
	"fmt"
	"log"
	"net/http"
	"net/url"
	"strings"
	"sync"
	"time"

	"github.com/gorilla/websocket"

	"github.com/snth-ai/snth-companion/internal/config"
	"github.com/snth-ai/snth-companion/internal/tools"
)

// Version is stamped at build time via -ldflags, falls back to "dev".
var Version = "dev"

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

// Stop closes the connection and prevents further reconnects.
func (c *Client) Stop() {
	c.mu.Lock()
	if c.stopCh == nil {
		c.mu.Unlock()
		return
	}
	close(c.stopCh)
	stop := c.doneCh
	c.stopCh = nil
	c.mu.Unlock()
	<-stop
}

func (c *Client) loop() {
	defer close(c.doneCh)
	backoff := time.Second
	for {
		select {
		case <-c.stopCh:
			return
		default:
		}

		cfg := config.Get()
		if cfg == nil || cfg.PairedSynthURL == "" || cfg.CompanionToken == "" {
			c.setStatus("paused", fmt.Errorf("not paired"))
			select {
			case <-c.stopCh:
				return
			case <-time.After(5 * time.Second):
			}
			continue
		}

		c.setStatus("connecting", nil)
		if err := c.runOnce(cfg.PairedSynthURL, cfg.CompanionToken); err != nil {
			log.Printf("[ws] disconnected: %v", err)
			c.setStatus("disconnected", err)
			select {
			case <-c.stopCh:
				return
			case <-time.After(backoff):
			}
			backoff *= 2
			if backoff > 30*time.Second {
				backoff = 30 * time.Second
			}
			continue
		}
		backoff = time.Second
	}
}

func (c *Client) runOnce(synthURL, token string) error {
	wsURL, err := toWSURL(synthURL, "/api/companion/ws")
	if err != nil {
		return err
	}
	hdr := http.Header{}
	hdr.Set("Authorization", "Bearer "+token)
	hdr.Set("User-Agent", "snth-companion/"+Version)

	dialer := *websocket.DefaultDialer
	dialer.HandshakeTimeout = 15 * time.Second
	conn, _, err := dialer.Dial(wsURL, hdr)
	if err != nil {
		return fmt.Errorf("dial %s: %w", wsURL, err)
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

	// Send hello with our capabilities.
	catalog := tools.Catalog()
	caps := make([]ToolDesc, len(catalog))
	for i, d := range catalog {
		caps[i] = ToolDesc{Name: d.Name, Description: d.Description, DangerLevel: d.DangerLevel}
	}
	if err := conn.WriteJSON(Frame{
		Type:             FrameHello,
		CompanionVersion: Version,
		Capabilities:     caps,
	}); err != nil {
		return fmt.Errorf("send hello: %w", err)
	}

	// Expect welcome.
	var welcome Frame
	_ = conn.SetReadDeadline(time.Now().Add(10 * time.Second))
	if err := conn.ReadJSON(&welcome); err != nil {
		return fmt.Errorf("read welcome: %w", err)
	}
	if welcome.Type != FrameWelcome {
		return fmt.Errorf("expected welcome, got %q", welcome.Type)
	}
	_ = conn.SetReadDeadline(time.Time{})

	c.setStatus("connected", nil)
	log.Printf("[ws] connected to %s (synth=%s v=%s, tools=%d)",
		wsURL, welcome.SynthID, welcome.SynthVersion, len(caps))

	// Ping loop keeps idle connections alive and surfaces dead ones.
	pingDone := make(chan struct{})
	go c.pingLoop(conn, pingDone)
	defer close(pingDone)

	// Read loop dispatches tool calls.
	for {
		var frame Frame
		if err := conn.ReadJSON(&frame); err != nil {
			return fmt.Errorf("read: %w", err)
		}
		switch frame.Type {
		case FrameToolCall:
			go c.handleToolCall(frame)
		case FramePing:
			_ = conn.WriteJSON(Frame{Type: FramePong})
		case FramePong:
			// noop — just means the remote is alive
		case FrameError:
			return fmt.Errorf("server error: %s", frame.Error)
		default:
			log.Printf("[ws] unknown frame type: %q", frame.Type)
		}
	}
}

func (c *Client) pingLoop(conn *websocket.Conn, done chan struct{}) {
	t := time.NewTicker(20 * time.Second)
	defer t.Stop()
	for {
		select {
		case <-done:
			return
		case <-t.C:
			c.mu.Lock()
			ws := c.ws
			c.mu.Unlock()
			if ws != conn {
				return
			}
			if err := conn.WriteJSON(Frame{Type: FramePing}); err != nil {
				return
			}
		}
	}
}

func (c *Client) handleToolCall(frame Frame) {
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Minute)
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
	if err := ws.WriteJSON(resp); err != nil {
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
