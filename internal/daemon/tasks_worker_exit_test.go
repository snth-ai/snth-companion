package daemon

// tasks_worker_exit_test.go — C1/C2/C3/C4 exit-detection correctness.

import (
	"encoding/json"
	"errors"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"os/exec"
	"path/filepath"
	"sync/atomic"
	"syscall"
	"testing"
	"time"
)

// terminalEvent finds the first captured subagent_exited/subagent_failed
// event and returns its new_state + exit_code (if present).
func (h *fakeHub) terminalEvent() (kind, newState string, exitCode int, found bool) {
	h.mu.Lock()
	defer h.mu.Unlock()
	for _, e := range h.events {
		k, _ := e["kind"].(string)
		if k != "subagent_exited" && k != "subagent_failed" {
			continue
		}
		ns, _ := e["new_state"].(string)
		code := -999
		if p, ok := e["payload"].(map[string]any); ok {
			if v, ok := p["exit_code"].(float64); ok {
				code = int(v)
			}
		}
		return k, ns, code, true
	}
	return "", "", 0, false
}

func (h *fakeHub) eventCount() int {
	h.mu.Lock()
	defer h.mu.Unlock()
	return len(h.events)
}

// spawnTestProcess mirrors the exec + reaper wiring in spawn() for a given
// shell body, returning the pid, cmd, and exitCh the monitor consumes.
func spawnTestProcess(t *testing.T, ws, body string) (int, *exec.Cmd, chan int) {
	t.Helper()
	if err := os.MkdirAll(ws, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(ws, "transcript.log"), nil, 0o644); err != nil {
		t.Fatal(err)
	}
	wrapped := "{ " + body + " ; } >> transcript.log 2>&1 ; " +
		"__rc=$? ; printf '%s' \"$__rc\" > " + exitCodeFile + ".tmp && mv " + exitCodeFile + ".tmp " + exitCodeFile
	cmd := exec.Command("bash", "-lc", wrapped)
	cmd.Dir = ws
	tlog, _ := os.OpenFile(filepath.Join(ws, "transcript.log"), os.O_APPEND|os.O_WRONLY, 0o644)
	cmd.Stdout = tlog
	cmd.Stderr = tlog
	cmd.SysProcAttr = &syscall.SysProcAttr{Setsid: true}
	_ = os.Remove(filepath.Join(ws, exitCodeFile))
	if err := cmd.Start(); err != nil {
		t.Fatal(err)
	}
	pid := cmd.Process.Pid
	exitCh := make(chan int, 1)
	go func() {
		werr := cmd.Wait()
		code := 0
		if cmd.ProcessState != nil {
			code = cmd.ProcessState.ExitCode()
		} else if werr != nil {
			code = -1
		}
		exitCh <- code
	}()
	return pid, cmd, exitCh
}

// C1 — a fast-exiting sub-agent must reach a terminal `done`/code-0 event.
// Pre-fix (no cmd.Wait) ProcessState stayed nil so the monitor never saw
// the fresh run finish; it would only fail via the stall watchdog.
func TestMonitorFreshRunReachesDone(t *testing.T) {
	hub := newFakeHub(t)
	w := setupWorkerEnv(t, hub)

	ws := workspaceFor("task_fresh")
	pid, cmd, exitCh := spawnTestProcess(t, ws, `echo hello`)
	w.mu.Lock()
	w.running["task_fresh"] = &localRun{taskID: "task_fresh", workspace: ws, pid: pid, cmd: cmd, exitCh: exitCh}
	w.mu.Unlock()

	done := make(chan struct{})
	go func() { w.monitor("task_fresh", ws, pid, cmd, exitCh, defaultBudget(), hooksConfig{}); close(done) }()
	select {
	case <-done:
	case <-time.After(10 * time.Second):
		t.Fatal("monitor did not return within 10s (fresh run exit not detected)")
	}

	kind, ns, code, ok := hub.terminalEvent()
	if !ok {
		t.Fatal("no terminal event posted for fresh run")
	}
	if kind != "subagent_exited" || ns != "done" || code != 0 {
		t.Fatalf("fresh run terminal = kind=%s state=%s code=%d, want subagent_exited/done/0", kind, ns, code)
	}
}

// C2 — a REATTACHED run (cmd==nil, exitCh==nil) recovers the true exit code
// from the rc file the wrapper wrote, instead of defaulting to -1/error.
func TestMonitorReattachRecoversExitCode(t *testing.T) {
	hub := newFakeHub(t)
	w := setupWorkerEnv(t, hub)

	ws := workspaceFor("task_reattach")
	// Simulate a run that already finished with code 0: process is gone,
	// rc file present. Use a pid that is definitely dead.
	pid, cmd, exitCh := spawnTestProcess(t, ws, `echo done0`)
	<-exitCh // let it finish + write the rc file
	_ = cmd
	// Confirm the rc file says 0.
	if code, ok := readExitCodeFile(ws); !ok || code != 0 {
		t.Fatalf("rc file not written correctly: code=%d ok=%v", code, ok)
	}

	w.mu.Lock()
	w.running["task_reattach"] = &localRun{taskID: "task_reattach", workspace: ws, pid: pid}
	w.mu.Unlock()

	done := make(chan struct{})
	// Reattach path: cmd==nil, exitCh==nil.
	go func() { w.monitor("task_reattach", ws, pid, nil, nil, defaultBudget(), hooksConfig{}); close(done) }()
	select {
	case <-done:
	case <-time.After(10 * time.Second):
		t.Fatal("reattach monitor did not return within 10s")
	}

	kind, ns, code, ok := hub.terminalEvent()
	if !ok {
		t.Fatal("no terminal event for reattached run")
	}
	if kind != "subagent_exited" || ns != "done" || code != 0 {
		t.Fatalf("reattach terminal = kind=%s state=%s code=%d, want subagent_exited/done/0", kind, ns, code)
	}
}

// C3 — a terminal postEvent that fails is retried; if it keeps failing it
// is persisted and delivered on the next flush. We fail the hub for the
// first N POSTs, then let it succeed, and assert the event lands + the
// pending file is cleaned.
func TestTerminalEventRetriedAndPersisted(t *testing.T) {
	var failing int32 = 1 // 1 => fail POSTs; 0 => accept
	fh := &fakeHub{}
	fh.srv = httptest.NewServer(http.HandlerFunc(func(rw http.ResponseWriter, r *http.Request) {
		if r.Method == http.MethodPost {
			if atomic.LoadInt32(&failing) != 0 {
				rw.WriteHeader(503)
				_, _ = rw.Write([]byte(`{"error":"down"}`))
				return
			}
			body, _ := io.ReadAll(r.Body)
			var m map[string]any
			_ = json.Unmarshal(body, &m)
			fh.mu.Lock()
			fh.events = append(fh.events, m)
			fh.mu.Unlock()
		}
		rw.WriteHeader(200)
		_, _ = rw.Write([]byte(`{}`))
	}))
	t.Cleanup(fh.srv.Close)
	w := setupWorkerEnv(t, fh)

	ws := workspaceFor("task_c3")
	if err := os.MkdirAll(ws, 0o755); err != nil {
		t.Fatal(err)
	}

	// All 3 inline retries fail -> event persisted.
	w.postTerminal("task_c3", ws, "done", "subagent_exited",
		map[string]any{"exit_code": 0}, map[string]any{})

	if _, err := os.Stat(filepath.Join(ws, pendingTerminalFile)); errors.Is(err, os.ErrNotExist) {
		t.Fatal("terminal event was not persisted after retries failed")
	}

	// Now let the hub accept POSTs and flush.
	atomic.StoreInt32(&failing, 0)
	w.flushPendingTerminals()

	if _, err := os.Stat(filepath.Join(ws, pendingTerminalFile)); !errors.Is(err, os.ErrNotExist) {
		t.Fatal("pending terminal file not cleared after successful flush")
	}
	if !fh.hasTerminalDone() {
		t.Fatal("terminal event never delivered to hub")
	}
}

func (h *fakeHub) hasTerminalDone() bool {
	h.mu.Lock()
	defer h.mu.Unlock()
	for _, e := range h.events {
		if ns, _ := e["new_state"].(string); ns == "done" {
			return true
		}
	}
	return false
}

// C4 — a run parked in awaiting_input must NOT be killed by the stall
// watchdog. We use a tiny stall timeout and an awaiting marker; the monitor
// must keep running past the stall window until the process actually exits.
func TestMonitorAwaitingInputExemptFromStall(t *testing.T) {
	hub := newFakeHub(t)
	w := setupWorkerEnv(t, hub)

	ws := workspaceFor("task_await")
	// The sub-agent immediately emits an INPUT REQUIRED marker, then sleeps
	// well past the (tiny) stall timeout, then exits 0. If awaiting is NOT
	// exempt, the stall watchdog kills it and reports an error/stall.
	pid, cmd, exitCh := spawnTestProcess(t, ws, `echo "INPUT REQUIRED: your name?"; sleep 3; echo bye`)
	w.mu.Lock()
	w.running["task_await"] = &localRun{taskID: "task_await", workspace: ws, pid: pid, cmd: cmd, exitCh: exitCh}
	w.mu.Unlock()

	budget := defaultBudget()
	budget.StallTimeout = 1 * time.Second // shorter than the 3s sleep

	done := make(chan struct{})
	go func() { w.monitor("task_await", ws, pid, cmd, exitCh, budget, hooksConfig{}); close(done) }()
	select {
	case <-done:
	case <-time.After(12 * time.Second):
		t.Fatal("awaiting monitor did not return")
	}

	// Must have posted awaiting_input and NOT a stall failure.
	hub.mu.Lock()
	defer hub.mu.Unlock()
	sawAwaiting, sawStall := false, false
	for _, e := range hub.events {
		if k, _ := e["kind"].(string); k == "subagent_awaiting_input" {
			sawAwaiting = true
		}
		if rt, ok := e["runtime"].(map[string]any); ok {
			if c, _ := rt["error_category"].(string); c == "stall_timeout" {
				sawStall = true
			}
		}
	}
	if !sawAwaiting {
		t.Fatal("awaiting_input event never posted")
	}
	if sawStall {
		t.Fatal("awaiting run was killed by the stall watchdog (C4 regression)")
	}
}
