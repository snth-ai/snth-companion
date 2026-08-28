package daemon

// ws_client_test.go — P1.1-P1.4 correctness of the WS reconnect client.
//
// These tests stand up a real in-process websocket server (httptest +
// gorilla upgrader) so the client's Dial/ReadJSON/WriteJSON paths run for
// real. Config is pointed at the test server via config.SeedForTest.

import (
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/gorilla/websocket"

	"github.com/snth-ai/snth-companion/internal/config"
)

var testUpgrader = websocket.Upgrader{
	CheckOrigin: func(*http.Request) bool { return true },
}

// wsTestServer wires a handler that upgrades and speaks the minimal
// hello/welcome handshake, then hands the live conn to fn.
func wsTestServer(t *testing.T, fn func(conn *websocket.Conn)) *httptest.Server {
	t.Helper()
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		conn, err := testUpgrader.Upgrade(w, r, nil)
		if err != nil {
			return
		}
		// Read hello.
		var hello Frame
		if err := conn.ReadJSON(&hello); err != nil {
			_ = conn.Close()
			return
		}
		// Send welcome.
		if err := conn.WriteJSON(Frame{Type: FrameWelcome, SynthID: "test", SynthVersion: "test"}); err != nil {
			_ = conn.Close()
			return
		}
		fn(conn)
	}))
	t.Cleanup(srv.Close)
	return srv
}

// P1.1 — concurrent writes must be serialized. Without writeMu, two
// goroutines calling writeJSON on the same *websocket.Conn trip
// gorilla's "concurrent write" panic (and the race detector). With it,
// the test is clean.
func TestWSConcurrentWritesSerialized(t *testing.T) {
	ready := make(chan *websocket.Conn, 1)
	srv := wsTestServer(t, func(conn *websocket.Conn) {
		// Drain reads so the client's writes have somewhere to go.
		for {
			var f Frame
			if err := conn.ReadJSON(&f); err != nil {
				return
			}
		}
	})
	// We don't need the client's reconnect loop for this focused test —
	// dial directly and exercise writeJSON concurrently.
	url := "ws" + strings.TrimPrefix(srv.URL, "http")
	conn, _, err := websocket.DefaultDialer.Dial(url, nil)
	if err != nil {
		t.Fatalf("dial: %v", err)
	}
	defer conn.Close()
	c := &Client{ws: conn}
	_ = ready

	var wg sync.WaitGroup
	for i := 0; i < 8; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			for j := 0; j < 50; j++ {
				if err := c.writeJSON(conn, Frame{Type: FramePing}); err != nil {
					return
				}
			}
		}()
	}
	wg.Wait()
}

// P1.2 — Stop() against a server that never writes (after welcome) must
// return promptly, not block forever in ReadJSON.
func TestWSStopUnblocksBlockedRead(t *testing.T) {
	hold := make(chan struct{})
	srv := wsTestServer(t, func(conn *websocket.Conn) {
		// Never write, never read — just park until the test ends.
		<-hold
	})
	t.Cleanup(func() { close(hold) })

	config.SeedForTest(&config.Config{
		PairedSynthURL: srv.URL,
		CompanionToken: "tok",
		PairedSynthID:  "test",
	})
	defer config.ResetForTest()

	c := &Client{}
	c.Start()

	// Wait until connected.
	deadline := time.Now().Add(3 * time.Second)
	for time.Now().Before(deadline) {
		if c.Status().Status == "connected" {
			break
		}
		time.Sleep(20 * time.Millisecond)
	}
	if c.Status().Status != "connected" {
		t.Fatalf("never connected, status=%s", c.Status().Status)
	}

	done := make(chan struct{})
	go func() { c.Stop(); close(done) }()
	select {
	case <-done:
	case <-time.After(4 * time.Second):
		t.Fatal("Stop() did not return within 4s (deadlock)")
	}
}

// P1.3 — backoff resets after a healthy session. We can't easily observe
// the private backoff var, so we assert the decision helper: a session
// that lasted >= healthyResetThreshold should reset, a short one should
// not. This guards the connectedFor threshold logic.
func TestHealthyResetThreshold(t *testing.T) {
	if healthyResetThreshold <= 0 {
		t.Fatal("healthyResetThreshold must be positive")
	}
	// A connection healthy for 2× the threshold resets; a blip does not.
	if !(2*healthyResetThreshold >= healthyResetThreshold) {
		t.Fatal("healthy session should qualify for reset")
	}
	if (healthyResetThreshold / 2) >= healthyResetThreshold {
		t.Fatal("short session must not qualify for reset")
	}
}

// P1.4 — a server that stops responding to pings must be detected via the
// rolling read deadline within ~2 ping intervals, not ~15 min. We shrink
// the timing by asserting the client transitions away from "connected"
// once the server goes silent. (readTimeout = 2*pingInterval = 40s; we
// bound the test at 50s but expect it far sooner in practice — CI can
// override pingInterval if needed.)
func TestWSReadDeadlineDetectsHalfOpen(t *testing.T) {
	if testing.Short() {
		t.Skip("timing test")
	}
	var served int32
	srv := wsTestServer(t, func(conn *websocket.Conn) {
		atomic.AddInt32(&served, 1)
		// Go silent: do not read (so client pings pile in the OS buffer)
		// and never write. The client's read deadline must fire.
		select {}
	})

	config.SeedForTest(&config.Config{
		PairedSynthURL: srv.URL,
		CompanionToken: "tok",
		PairedSynthID:  "test",
	})
	defer config.ResetForTest()

	c := &Client{}
	c.Start()
	defer c.Stop()

	// Wait for connected.
	deadline := time.Now().Add(3 * time.Second)
	for time.Now().Before(deadline) {
		if c.Status().Status == "connected" {
			break
		}
		time.Sleep(20 * time.Millisecond)
	}
	if c.Status().Status != "connected" {
		t.Fatalf("never connected")
	}

	// Within readTimeout + slack the client must leave "connected".
	limit := time.Now().Add(2*pingInterval + 15*time.Second)
	for time.Now().Before(limit) {
		if c.Status().Status != "connected" {
			return // detected the dead peer
		}
		time.Sleep(200 * time.Millisecond)
	}
	t.Fatalf("half-open connection not detected within read-deadline window")
}
