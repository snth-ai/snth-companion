package browser

import (
	"context"
	"encoding/json"
	"fmt"
	"log"
	"net/http"
	"sync"
	"sync/atomic"
	"time"

	"github.com/gorilla/websocket"
)

// relay.go — Chrome-extension relay server.
//
// When the user has the SNTH Browser Relay extension pinned + clicked
// on a tab, the extension's service worker opens a WS to us at
// ws://127.0.0.1:18792/extension. We speak "JSON-RPC-ish" over that WS:
// the companion sends CDP commands (forwarded by the extension via
// chrome.debugger.sendCommand), and CDP events from the browser
// arrive back as `{event, params, tabId}` frames.
//
// This is the alternative to launching Chrome with
// --remote-debugging-port=9222 — the extension path keeps the
// user's normal profile and per-tab attach UX.
//
// The relay mimics the CDP interface of Session+CDP so the rest of
// the tool code doesn't care which path drove the browser. See
// ExtensionClient.Send — same signature as CDP.Send.

// RelayServer owns the :18792 listener and the single active
// extension connection (only one tab attached at a time).
type RelayServer struct {
	Port int
	ln   http.Handler

	mu         sync.Mutex
	client     *ExtensionClient
	waiters    []chan *ExtensionClient
	serverErrs chan error
}

// NewRelayServer builds the server without starting it. Call Start
// to bind + serve.
func NewRelayServer(port int) *RelayServer {
	if port == 0 {
		port = 18792
	}
	return &RelayServer{Port: port, serverErrs: make(chan error, 1)}
}

// Start binds on 127.0.0.1:<Port> and listens for /extension
// WebSocket upgrades. Non-blocking; server runs in a goroutine.
func (s *RelayServer) Start() error {
	mux := http.NewServeMux()
	mux.HandleFunc("/extension", s.handleExtensionWS)
	mux.HandleFunc("/health", func(w http.ResponseWriter, r *http.Request) {
		w.Write([]byte("ok"))
	})
	addr := fmt.Sprintf("127.0.0.1:%d", s.Port)
	server := &http.Server{Addr: addr, Handler: mux, ReadTimeout: 10 * time.Second}
	go func() {
		log.Printf("[relay] listening on %s", addr)
		if err := server.ListenAndServe(); err != nil && err != http.ErrServerClosed {
			s.serverErrs <- err
		}
	}()
	return nil
}

// WaitForClient blocks until an extension has connected (or ctx
// expires). Returned client is the currently-active one; it's safe
// to use until its Closed() returns.
func (s *RelayServer) WaitForClient(ctx context.Context) (*ExtensionClient, error) {
	s.mu.Lock()
	if s.client != nil && !s.client.Closed() {
		c := s.client
		s.mu.Unlock()
		return c, nil
	}
	ch := make(chan *ExtensionClient, 1)
	s.waiters = append(s.waiters, ch)
	s.mu.Unlock()

	select {
	case <-ctx.Done():
		return nil, ctx.Err()
	case c, ok := <-ch:
		if !ok {
			return nil, fmt.Errorf("relay closed while waiting")
		}
		return c, nil
	}
}

// Client returns the currently-active extension connection, or nil
// if no extension is attached.
func (s *RelayServer) Client() *ExtensionClient {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.client != nil && !s.client.Closed() {
		return s.client
	}
	return nil
}

var relayUpgrader = websocket.Upgrader{
	ReadBufferSize:  16384,
	WriteBufferSize: 16384,
	// The extension connects to us from itself — Origin is the
	// chrome-extension://<id> URL which we can't predict without a
	// store listing. Accept any Origin; we bind to 127.0.0.1 so
	// only local clients can hit us regardless.
	CheckOrigin: func(r *http.Request) bool { return true },
}

func (s *RelayServer) handleExtensionWS(w http.ResponseWriter, r *http.Request) {
	conn, err := relayUpgrader.Upgrade(w, r, nil)
	if err != nil {
		log.Printf("[relay] upgrade: %v", err)
		return
	}
	log.Printf("[relay] extension connected from %s", r.RemoteAddr)

	client := newExtensionClient(conn)

	s.mu.Lock()
	// Displace previous.
	if prev := s.client; prev != nil {
		prev.Close()
	}
	s.client = client
	waiters := s.waiters
	s.waiters = nil
	s.mu.Unlock()

	for _, ch := range waiters {
		select {
		case ch <- client:
		default:
		}
		close(ch)
	}

	client.runLoop()

	s.mu.Lock()
	if s.client == client {
		s.client = nil
	}
	s.mu.Unlock()
	log.Printf("[relay] extension disconnected")
}

// ExtensionClient is a CDP-shaped wrapper over the extension WS.
// Semantics match CDP: Send(method, params, result) → synchronous
// JSON-RPC call; On(fn) → subscribe to events.
type ExtensionClient struct {
	conn   *websocket.Conn
	mu     sync.Mutex
	nextID int64
	pend   map[int64]chan relayResp

	evMu     sync.Mutex
	evHandlers []func(method string, params json.RawMessage)

	writeMu sync.Mutex
	closed  atomic.Bool
}

type relayResp struct {
	ID     int64           `json:"id,omitempty"`
	Result json.RawMessage `json:"result,omitempty"`
	Error  *relayErr       `json:"error,omitempty"`

	// Event shape (from chrome.debugger.onEvent).
	Event  string          `json:"event,omitempty"`
	Params json.RawMessage `json:"params,omitempty"`
	TabID  int             `json:"tabId,omitempty"`

	// Hello from extension.
	Type             string `json:"type,omitempty"`
	ExtensionVersion string `json:"extensionVersion,omitempty"`
}

type relayErr struct {
	Code    int    `json:"code"`
	Message string `json:"message"`
}

func (e *relayErr) Error() string { return fmt.Sprintf("%d %s", e.Code, e.Message) }

func newExtensionClient(conn *websocket.Conn) *ExtensionClient {
	return &ExtensionClient{conn: conn, pend: map[int64]chan relayResp{}}
}

func (c *ExtensionClient) runLoop() {
	for {
		var msg relayResp
		if err := c.conn.ReadJSON(&msg); err != nil {
			c.markClosed()
			return
		}
		if msg.Event != "" {
			c.evMu.Lock()
			var fns []func(string, json.RawMessage)
			fns = append(fns, c.evHandlers...)
			c.evMu.Unlock()
			for _, fn := range fns {
				fn(msg.Event, msg.Params)
			}
			continue
		}
		if msg.ID != 0 {
			c.mu.Lock()
			ch, ok := c.pend[msg.ID]
			delete(c.pend, msg.ID)
			c.mu.Unlock()
			if ok {
				select {
				case ch <- msg:
				default:
				}
			}
		}
	}
}

func (c *ExtensionClient) markClosed() {
	if c.closed.Swap(true) {
		return
	}
	c.conn.Close()
	c.mu.Lock()
	for id, ch := range c.pend {
		close(ch)
		delete(c.pend, id)
	}
	c.mu.Unlock()
}

// Closed reports whether the WS has been torn down.
func (c *ExtensionClient) Closed() bool { return c.closed.Load() }

// Close tears the WS down. Safe to call multiple times.
func (c *ExtensionClient) Close() {
	c.markClosed()
}

// Send has the same signature as CDP.Send so actions.go and
// snapshot.go can work against either a --remote-debugging-port CDP
// or an extension-relay ExtensionClient.
func (c *ExtensionClient) Send(ctx context.Context, method string, params any, result any) error {
	if c.closed.Load() {
		return fmt.Errorf("extension relay closed")
	}
	id := atomic.AddInt64(&c.nextID, 1)
	ch := make(chan relayResp, 1)
	c.mu.Lock()
	c.pend[id] = ch
	c.mu.Unlock()

	req := map[string]any{
		"id":     id,
		"method": method,
		"params": params,
	}

	c.writeMu.Lock()
	err := c.conn.WriteJSON(req)
	c.writeMu.Unlock()
	if err != nil {
		c.mu.Lock()
		delete(c.pend, id)
		c.mu.Unlock()
		return fmt.Errorf("write %s: %w", method, err)
	}

	select {
	case <-ctx.Done():
		c.mu.Lock()
		delete(c.pend, id)
		c.mu.Unlock()
		return ctx.Err()
	case resp, ok := <-ch:
		if !ok {
			return fmt.Errorf("relay closed while waiting for %s", method)
		}
		if resp.Error != nil {
			return fmt.Errorf("%s: %w", method, resp.Error)
		}
		if result != nil && len(resp.Result) > 0 {
			if err := json.Unmarshal(resp.Result, result); err != nil {
				return fmt.Errorf("%s: decode: %w", method, err)
			}
		}
		return nil
	}
}

// On registers an event handler.
func (c *ExtensionClient) On(fn func(method string, params json.RawMessage)) {
	c.evMu.Lock()
	c.evHandlers = append(c.evHandlers, fn)
	c.evMu.Unlock()
}
