package daemon

import (
	"context"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"sync"
	"testing"
	"time"

	"github.com/snth-ai/snth-companion/internal/approval"
	"github.com/snth-ai/snth-companion/internal/config"
)

// fakeHub captures the task events the worker posts.
type fakeHub struct {
	mu     sync.Mutex
	events []map[string]any
	srv    *httptest.Server
}

func newFakeHub(t *testing.T) *fakeHub {
	t.Helper()
	h := &fakeHub{}
	h.srv = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method == http.MethodPost {
			body, _ := io.ReadAll(r.Body)
			var m map[string]any
			_ = json.Unmarshal(body, &m)
			m["__path"] = r.URL.Path
			h.mu.Lock()
			h.events = append(h.events, m)
			h.mu.Unlock()
		}
		w.WriteHeader(200)
		_, _ = w.Write([]byte(`{}`))
	}))
	t.Cleanup(h.srv.Close)
	return h
}

func (h *fakeHub) sawDeclined() bool {
	h.mu.Lock()
	defer h.mu.Unlock()
	for _, e := range h.events {
		if r, _ := e["reason"].(string); r == "declined" {
			return true
		}
		// runtime payload carries error_category too
		if rt, ok := e["runtime"].(map[string]any); ok {
			if c, _ := rt["error_category"].(string); c == "declined" {
				return true
			}
		}
	}
	return false
}

// setupWorkerEnv points HOME + config at a temp dir with an active pair
// whose hub URL is the fake hub, and restores the approval stub.
func setupWorkerEnv(t *testing.T, hub *fakeHub) *TasksWorker {
	t.Helper()
	tmp := t.TempDir()
	t.Setenv("HOME", tmp)

	// Ensure a config is loaded (cached pointer is fine — we inject the
	// active pair below via Update, which is what spawn() reads).
	if _, err := config.Load(); err != nil {
		t.Fatalf("config load: %v", err)
	}
	if err := config.Update(func(c *config.Config) {
		c.AddOrUpdatePair(config.SynthPair{
			ID:     "synth_test",
			URL:    "wss://x/api/companion/ws",
			Token:  "tok",
			HubURL: hub.srv.URL,
		})
		_ = c.SetActive("synth_test")
	}); err != nil {
		t.Fatalf("config update: %v", err)
	}

	w := &TasksWorker{
		HTTP:     &http.Client{Timeout: 5 * time.Second},
		running:  make(map[string]*localRun),
		approved: make(map[string]bool),
		stopCh:   make(chan struct{}),
	}
	return w
}

func taskRowFixture() taskRow {
	return taskRow{
		ID:           "task_abc",
		Title:        "do a thing",
		SubAgentKind: "claude",
	}
}

// TestTaskSpawnDeniedDoesNotExecute proves P0.7/A2: a denied task never
// runs a hook or the sub-agent, and reports declined to the hub.
func TestTaskSpawnDeniedDoesNotExecute(t *testing.T) {
	hub := newFakeHub(t)
	w := setupWorkerEnv(t, hub)

	// Stub approval to DENY and record that it was consulted.
	consulted := false
	orig := taskApprovalRequest
	taskApprovalRequest = func(_ context.Context, r approval.Request_) (bool, error) {
		consulted = true
		if r.Tool != "task_run" {
			t.Errorf("gate tool = %q, want task_run", r.Tool)
		}
		return false, nil
	}
	t.Cleanup(func() { taskApprovalRequest = orig })

	w.spawn(context.Background(), taskRowFixture())

	if !consulted {
		t.Fatal("approval gate was not consulted before running the task")
	}
	// No sub-agent process should be tracked.
	w.mu.Lock()
	_, running := w.running["task_abc"]
	w.mu.Unlock()
	if running {
		t.Fatal("denied task left a run in flight — sub-agent executed despite denial")
	}
	if !hub.sawDeclined() {
		t.Fatal("denied task did not report declined to the hub")
	}
}

// TestTaskSpawnApprovedProceeds proves the gate lets an approved task
// through (it advances past the gate to command build/exec instead of
// reporting declined).
func TestTaskSpawnApprovedProceeds(t *testing.T) {
	hub := newFakeHub(t)
	w := setupWorkerEnv(t, hub)

	orig := taskApprovalRequest
	taskApprovalRequest = func(_ context.Context, _ approval.Request_) (bool, error) {
		return true, nil // approve
	}
	t.Cleanup(func() { taskApprovalRequest = orig })

	// Use an unknown sub-agent kind so buildCommand fails cleanly right
	// AFTER the gate — proving the gate passed without launching a real
	// claude/codex process.
	tr := taskRowFixture()
	tr.SubAgentKind = "definitely-not-an-agent"
	w.spawn(context.Background(), tr)

	if hub.sawDeclined() {
		t.Fatal("approved task incorrectly reported declined")
	}
	if !w.taskApproved("task_abc") {
		t.Fatal("approved task not remembered")
	}
}
