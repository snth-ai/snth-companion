// Command fake-synth is a minimal stand-in for an openpaw synth, used for
// integration smoke tests of the companion WS protocol without pulling in
// the full synth runtime (LanceDB + all).
//
// It serves /api/companion/ws with the same token-auth + hello/welcome
// handshake as companion_ws.go in openpaw_server, then invokes one RPC
// call per --invoke flag and prints the result.
//
// Usage:
//
//	SNTH_COMPANION_TOKEN=devtoken go run ./cmd/fake-synth \
//	    --port 9999 --invoke remote_bash --args '{"cmd":"echo hi"}'
package main

import (
	"encoding/json"
	"flag"
	"fmt"
	"log"
	"net/http"
	"os"
	"sync"
	"sync/atomic"
	"time"

	"github.com/gorilla/websocket"
)

type frame struct {
	Type string `json:"type"`

	CompanionVersion string            `json:"companion_version,omitempty"`
	Capabilities     []json.RawMessage `json:"capabilities,omitempty"`
	SynthVersion     string            `json:"synth_version,omitempty"`
	SynthID          string            `json:"synth_id,omitempty"`

	CallID string          `json:"call_id,omitempty"`
	Tool   string          `json:"tool,omitempty"`
	Args   json.RawMessage `json:"args,omitempty"`
	Data   json.RawMessage `json:"data,omitempty"`
	Error  string          `json:"error,omitempty"`
}

var (
	upgrader = websocket.Upgrader{CheckOrigin: func(r *http.Request) bool { return true }}

	mu         sync.Mutex
	activeConn *websocket.Conn
	connected  chan struct{}

	pendingMu sync.Mutex
	pending   = map[string]chan frame{}

	nextID uint64
)

func main() {
	port := flag.Int("port", 9999, "listen port")
	invoke := flag.String("invoke", "", "tool name to invoke once companion connects")
	argsRaw := flag.String("args", "{}", "JSON args for the invoke")
	timeout := flag.Duration("timeout", 2*time.Minute, "overall timeout")
	flag.Parse()

	token := os.Getenv("SNTH_COMPANION_TOKEN")
	if token == "" {
		token = "devtoken"
		log.Printf("SNTH_COMPANION_TOKEN not set, using %q", token)
	}

	connected = make(chan struct{})

	http.HandleFunc("/api/companion/ws", func(w http.ResponseWriter, r *http.Request) {
		if r.Header.Get("Authorization") != "Bearer "+token {
			http.Error(w, "unauthorized", http.StatusUnauthorized)
			return
		}
		conn, err := upgrader.Upgrade(w, r, nil)
		if err != nil {
			log.Printf("upgrade: %v", err)
			return
		}
		var hello frame
		if err := conn.ReadJSON(&hello); err != nil {
			log.Printf("hello: %v", err)
			conn.Close()
			return
		}
		if hello.Type != "hello" {
			log.Printf("expected hello, got %q", hello.Type)
			conn.Close()
			return
		}
		log.Printf("companion connected (version=%s, tools=%d)", hello.CompanionVersion, len(hello.Capabilities))

		if err := conn.WriteJSON(frame{Type: "welcome", SynthVersion: "fake-synth", SynthID: "smoke_test"}); err != nil {
			log.Printf("welcome: %v", err)
			conn.Close()
			return
		}

		mu.Lock()
		activeConn = conn
		mu.Unlock()
		select {
		case <-connected:
		default:
			close(connected)
		}

		for {
			var f frame
			if err := conn.ReadJSON(&f); err != nil {
				log.Printf("read: %v", err)
				return
			}
			if f.Type == "tool_result" {
				pendingMu.Lock()
				ch, ok := pending[f.CallID]
				pendingMu.Unlock()
				if ok {
					ch <- f
				}
			}
		}
	})

	server := &http.Server{Addr: fmt.Sprintf("127.0.0.1:%d", *port)}
	go func() {
		log.Printf("fake-synth listening on %s", server.Addr)
		if err := server.ListenAndServe(); err != nil {
			log.Fatal(err)
		}
	}()

	if *invoke == "" {
		select {}
	}

	// Wait for companion.
	select {
	case <-connected:
	case <-time.After(*timeout):
		log.Fatal("timed out waiting for companion to connect")
	}

	// Invoke the tool.
	id := fmt.Sprintf("fs%d", atomic.AddUint64(&nextID, 1))
	ch := make(chan frame, 1)
	pendingMu.Lock()
	pending[id] = ch
	pendingMu.Unlock()

	mu.Lock()
	conn := activeConn
	mu.Unlock()

	if err := conn.WriteJSON(frame{Type: "tool_call", CallID: id, Tool: *invoke, Args: json.RawMessage(*argsRaw)}); err != nil {
		log.Fatalf("write tool_call: %v", err)
	}

	select {
	case resp := <-ch:
		if resp.Error != "" {
			log.Fatalf("tool error: %s", resp.Error)
		}
		fmt.Println(string(resp.Data))
	case <-time.After(*timeout):
		log.Fatal("timed out waiting for tool_result")
	}
}
