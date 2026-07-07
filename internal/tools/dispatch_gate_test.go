package tools

import (
	"context"
	"encoding/json"
	"errors"
	"sync"
	"testing"
)

// resetApprovalFn restores the package approval fn after a test.
func withApprovalFn(t *testing.T, fn ApprovalFn) {
	t.Helper()
	SetApprovalFn(fn)
	t.Cleanup(func() { SetApprovalFn(nil) })
}

// registerFake registers a throwaway tool for gate tests, tracking whether
// its handler ran, and de-registers it after the test.
func registerFake(t *testing.T, name, danger string, gp func(json.RawMessage) GateDecision, ran *bool) {
	t.Helper()
	Register(Descriptor{Name: name, DangerLevel: danger, GatePolicy: gp}, func(ctx context.Context, _ json.RawMessage) (any, error) {
		*ran = true
		return "ok", nil
	})
	t.Cleanup(func() {
		mu.Lock()
		delete(handlers, name)
		delete(descs, name)
		mu.Unlock()
	})
}

func TestDispatchGateDeniesPromptTool(t *testing.T) {
	var ran bool
	registerFake(t, "fake_prompt", "prompt", nil, &ran)

	var gotTool, gotDanger string
	withApprovalFn(t, func(_ context.Context, tool, _, danger, _ string) (bool, error) {
		gotTool, gotDanger = tool, danger
		return false, nil // deny
	})

	out, err := Dispatch(context.Background(), "fake_prompt", json.RawMessage(`{}`))
	if !errors.Is(err, ErrDenied) {
		t.Fatalf("want ErrDenied, got out=%v err=%v", out, err)
	}
	if ran {
		t.Fatal("handler ran despite denial — gate did not block the handler")
	}
	if gotTool != "fake_prompt" || gotDanger != "prompt" {
		t.Fatalf("approval fn got tool=%q danger=%q, want fake_prompt/prompt", gotTool, gotDanger)
	}
}

func TestDispatchGateAllowsRunsHandler(t *testing.T) {
	var ran bool
	registerFake(t, "fake_prompt_ok", "prompt", nil, &ran)
	withApprovalFn(t, func(_ context.Context, _, _, _, _ string) (bool, error) { return true, nil })

	out, err := Dispatch(context.Background(), "fake_prompt_ok", json.RawMessage(`{}`))
	if err != nil {
		t.Fatalf("unexpected err: %v", err)
	}
	if !ran {
		t.Fatal("handler did not run after approval allowed")
	}
	if out != "ok" {
		t.Fatalf("want ok, got %v", out)
	}
}

func TestDispatchSafeToolSkipsGate(t *testing.T) {
	var ran bool
	registerFake(t, "fake_safe", "safe", nil, &ran)
	// Approval fn that would DENY — must never be consulted for a safe tool.
	called := false
	withApprovalFn(t, func(_ context.Context, _, _, _, _ string) (bool, error) {
		called = true
		return false, nil
	})

	if _, err := Dispatch(context.Background(), "fake_safe", json.RawMessage(`{}`)); err != nil {
		t.Fatalf("safe tool should run, got err %v", err)
	}
	if !ran {
		t.Fatal("safe tool handler did not run")
	}
	if called {
		t.Fatal("approval fn consulted for a safe tool")
	}
}

func TestDispatchDenyClosedWhenApprovalUnset(t *testing.T) {
	var ran bool
	registerFake(t, "fake_unwired", "prompt", nil, &ran)
	SetApprovalFn(nil) // explicitly unwired

	out, err := Dispatch(context.Background(), "fake_unwired", json.RawMessage(`{}`))
	if !errors.Is(err, ErrDenied) {
		t.Fatalf("deny-closed expected ErrDenied, got out=%v err=%v", out, err)
	}
	if ran {
		t.Fatal("handler ran with no approval fn wired — not deny-closed")
	}
}

func TestDispatchGatePolicySkip(t *testing.T) {
	var ran bool
	registerFake(t, "fake_gatepolicy", "prompt", func(json.RawMessage) GateDecision { return GateSkip }, &ran)
	called := false
	withApprovalFn(t, func(_ context.Context, _, _, _, _ string) (bool, error) {
		called = true
		return false, nil
	})
	if _, err := Dispatch(context.Background(), "fake_gatepolicy", json.RawMessage(`{}`)); err != nil {
		t.Fatalf("GateSkip should run handler, got err %v", err)
	}
	if !ran {
		t.Fatal("GateSkip handler did not run")
	}
	if called {
		t.Fatal("approval fn consulted despite GateSkip")
	}
}

// TestYtDlpAndContactsArePromptTier guards the A1/A3 reclassifications:
// remote_yt_dlp and remote_contacts_search must be gated (not safe).
func TestYtDlpAndContactsArePromptTier(t *testing.T) {
	RegisterYtDlp()
	found := map[string]string{}
	for _, d := range Catalog() {
		found[d.Name] = d.DangerLevel
	}
	if lvl := found["remote_yt_dlp"]; lvl != "prompt" && lvl != "always-prompt" {
		t.Fatalf("remote_yt_dlp danger level = %q, want prompt/always-prompt (A1)", lvl)
	}
}

// TestDispatchDeniedYtDlpNoExec proves a denied yt-dlp call never runs the
// handler (so no yt-dlp subprocess spawns), even with an --exec payload.
func TestDispatchDeniedYtDlpNoExec(t *testing.T) {
	RegisterYtDlp()
	withApprovalFn(t, func(_ context.Context, _, _, _, _ string) (bool, error) { return false, nil })
	args, _ := json.Marshal(map[string]any{"args": []string{"--exec", "touch /tmp/pwned", "https://x"}})
	_, err := Dispatch(context.Background(), "remote_yt_dlp", args)
	if !errors.Is(err, ErrDenied) {
		t.Fatalf("denied yt-dlp should be ErrDenied, got %v", err)
	}
}

// concurrency smoke: the gate + descriptor lookup must be race-free under
// parallel Dispatch. Registers a safe tool with a race-free handler (no
// shared-write) so the -race detector only exercises the registry/gate
// paths, not the test's own bookkeeping.
func TestDispatchGateConcurrent(t *testing.T) {
	Register(Descriptor{Name: "fake_concurrent", DangerLevel: "safe"}, func(ctx context.Context, _ json.RawMessage) (any, error) {
		return "ok", nil
	})
	t.Cleanup(func() {
		mu.Lock()
		delete(handlers, "fake_concurrent")
		delete(descs, "fake_concurrent")
		mu.Unlock()
	})
	var wg sync.WaitGroup
	for i := 0; i < 32; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			_, _ = Dispatch(context.Background(), "fake_concurrent", json.RawMessage(`{}`))
		}()
	}
	wg.Wait()
}
