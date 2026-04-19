// Package browser talks to Chrome over the Chrome DevTools Protocol
// (CDP). We implement CDP directly on top of gorilla/websocket instead
// of pulling in chromedp — the tool we expose to synths needs
// LLM-friendly snapshot output, which means custom tree formatting
// that chromedp's high-level helpers don't provide. Raw CDP is also
// smaller (no headless browser library), easier to reason about, and
// lets us evolve the wire shapes as our snapshot format matures.
//
// The layering:
//
//   cdp.go      — raw JSON-RPC 2.0 client: one WS per target, id-
//                 correlated send/recv, event subscriptions.
//   session.go  — high-level Chrome session: attach to localhost:9222,
//                 pick a target, reconnect on disconnect.
//   snapshot.go — DOM → numbered tree via Runtime.evaluate.
//   actions.go  — click / type / navigate / wait / screenshot.
//
// Chrome must be launched with --remote-debugging-port=9222 for us to
// attach. The companion doesn't bundle Chrome — we attach to the
// user's already-running browser (after they relaunch it with the
// flag) so they see exactly what the synth is doing in their own
// session.
package browser

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"net/url"
	"sync"
	"sync/atomic"
	"time"

	"github.com/gorilla/websocket"
)

// Conn is the minimal surface our higher-level browser code needs. Both
// *CDP (direct --remote-debugging-port attach) and *ExtensionClient
// (via the relay/extension bridge) implement it, so snapshot.go and
// actions.go take Conn, not a concrete *CDP.
type Conn interface {
	Send(ctx context.Context, method string, params any, result any) error
	On(fn func(method string, params json.RawMessage))
}

// CDP is a single CDP WebSocket client attached to one target (tab or
// browser endpoint). Safe for concurrent Send calls from many
// goroutines. Not safe to reuse after Close — create a new CDP.
type CDP struct {
	url  string // ws://...
	conn *websocket.Conn

	mu        sync.Mutex
	nextID    int64
	pending   map[int64]chan *cdpResp   // id → waiter
	events    []func(method string, params json.RawMessage)
	closed    atomic.Bool
	closeCh   chan struct{}

	writeMu sync.Mutex // serialize WriteJSON
}

type cdpReq struct {
	ID         int64       `json:"id"`
	Method     string      `json:"method"`
	Params     any         `json:"params,omitempty"`
	SessionID  string      `json:"sessionId,omitempty"`
}

type cdpResp struct {
	ID        int64           `json:"id"`
	Result    json.RawMessage `json:"result,omitempty"`
	Error     *cdpError       `json:"error,omitempty"`
	Method    string          `json:"method,omitempty"`
	Params    json.RawMessage `json:"params,omitempty"`
	SessionID string          `json:"sessionId,omitempty"`
}

type cdpError struct {
	Code    int    `json:"code"`
	Message string `json:"message"`
	Data    string `json:"data,omitempty"`
}

func (e *cdpError) Error() string {
	if e.Data != "" {
		return fmt.Sprintf("%d %s: %s", e.Code, e.Message, e.Data)
	}
	return fmt.Sprintf("%d %s", e.Code, e.Message)
}

// Dial opens a CDP WebSocket. wsURL is the target's
// webSocketDebuggerUrl from /json/list or the top-level browser URL
// from /json/version.
func Dial(ctx context.Context, wsURL string) (*CDP, error) {
	if _, err := url.Parse(wsURL); err != nil {
		return nil, fmt.Errorf("parse ws url: %w", err)
	}
	dialer := websocket.DefaultDialer
	dctx, cancel := context.WithTimeout(ctx, 10*time.Second)
	defer cancel()
	conn, _, err := dialer.DialContext(dctx, wsURL, http.Header{})
	if err != nil {
		return nil, fmt.Errorf("ws dial %s: %w", wsURL, err)
	}
	c := &CDP{
		url:     wsURL,
		conn:    conn,
		pending: map[int64]chan *cdpResp{},
		closeCh: make(chan struct{}),
	}
	go c.readLoop()
	return c, nil
}

func (c *CDP) readLoop() {
	for {
		if c.closed.Load() {
			return
		}
		var msg cdpResp
		if err := c.conn.ReadJSON(&msg); err != nil {
			c.closeAndDrain()
			return
		}
		if msg.Method != "" {
			// Server-initiated event.
			c.mu.Lock()
			var fns []func(string, json.RawMessage)
			fns = append(fns, c.events...)
			c.mu.Unlock()
			for _, fn := range fns {
				fn(msg.Method, msg.Params)
			}
			continue
		}
		// Response to one of our requests.
		c.mu.Lock()
		ch, ok := c.pending[msg.ID]
		delete(c.pending, msg.ID)
		c.mu.Unlock()
		if ok {
			select {
			case ch <- &msg:
			default:
			}
		}
	}
}

func (c *CDP) closeAndDrain() {
	if c.closed.Swap(true) {
		return
	}
	close(c.closeCh)
	_ = c.conn.Close()
	c.mu.Lock()
	for id, ch := range c.pending {
		close(ch)
		delete(c.pending, id)
	}
	c.mu.Unlock()
}

// Send issues a CDP command and blocks until the matching response
// arrives (or ctx cancels). result, if non-nil, is JSON-decoded from
// the response's `result` field.
func (c *CDP) Send(ctx context.Context, method string, params any, result any) error {
	return c.SendSession(ctx, "", method, params, result)
}

// SendSession is Send for session-scoped commands (Target.sendMessageToTarget
// forwarding). Leave sessionID empty for the root (browser) session.
func (c *CDP) SendSession(ctx context.Context, sessionID, method string, params any, result any) error {
	if c.closed.Load() {
		return fmt.Errorf("cdp: connection closed")
	}
	id := atomic.AddInt64(&c.nextID, 1)
	ch := make(chan *cdpResp, 1)
	c.mu.Lock()
	c.pending[id] = ch
	c.mu.Unlock()

	req := cdpReq{ID: id, Method: method, Params: params, SessionID: sessionID}

	c.writeMu.Lock()
	err := c.conn.WriteJSON(req)
	c.writeMu.Unlock()
	if err != nil {
		c.mu.Lock()
		delete(c.pending, id)
		c.mu.Unlock()
		return fmt.Errorf("write %s: %w", method, err)
	}

	select {
	case <-ctx.Done():
		c.mu.Lock()
		delete(c.pending, id)
		c.mu.Unlock()
		return ctx.Err()
	case resp, ok := <-ch:
		if !ok {
			return fmt.Errorf("cdp: connection closed while waiting for %s", method)
		}
		if resp.Error != nil {
			return fmt.Errorf("%s: %w", method, resp.Error)
		}
		if result != nil && len(resp.Result) > 0 {
			if err := json.Unmarshal(resp.Result, result); err != nil {
				return fmt.Errorf("%s: decode result: %w", method, err)
			}
		}
		return nil
	}
}

// On registers an event handler. Handlers are called in order for
// every server-initiated method frame. Not removable — CDP sessions
// are short-lived in our usage (per snapshot / per action).
func (c *CDP) On(fn func(method string, params json.RawMessage)) {
	c.mu.Lock()
	c.events = append(c.events, fn)
	c.mu.Unlock()
}

// Close terminates the WS.
func (c *CDP) Close() {
	c.closeAndDrain()
}
