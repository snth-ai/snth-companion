package daemon

// subagent_timeout_test.go — G1/P1.10: remote_subagent must run under its
// own long context, not the 5-min dispatcher ceiling that would SIGKILL a
// 30-60 min delegation at ~5 min.

import (
	"context"
	"encoding/json"
	"testing"
	"time"

	"github.com/snth-ai/snth-companion/internal/tools"
)

func TestSubagentGetsLongToolCallTimeout(t *testing.T) {
	// The subagent handler derives its own WithTimeout from the ctx
	// handleToolCall passes. That ctx ceiling must comfortably exceed the
	// subagent's 60-min internal hard cap, else the parent deadline wins
	// and kills it early.
	const subagentInternalMax = 60 * time.Minute

	got := toolCallTimeout("remote_subagent")
	if got <= subagentInternalMax {
		t.Fatalf("remote_subagent tool-call timeout = %s, must exceed the 60-min internal cap so the dispatcher ctx never wins", got)
	}

	// Every other tool keeps the short 5-min ceiling.
	if d := toolCallTimeout("remote_fs_read"); d != 5*time.Minute {
		t.Fatalf("default tool-call timeout = %s, want 5m", d)
	}
}

// TestHandleToolCallAppliesSubagentTimeout exercises the REAL dispatch
// path: handleToolCall -> toolCallTimeout -> Dispatch -> handler. The
// handler inspects its ctx deadline. Pre-fix (a flat 5-min ctx) the
// deadline would be ~5 min from now; the fix gives remote_subagent >60min.
func TestHandleToolCallAppliesSubagentTimeout(t *testing.T) {
	gotDeadline := make(chan time.Time, 1)
	// Register a fake "remote_subagent" (safe, so no approval gate) that
	// records the ctx deadline it was dispatched with. Register is
	// last-wins, so this shadows the real one for the test.
	tools.Register(tools.Descriptor{Name: "remote_subagent", DangerLevel: "safe"},
		func(ctx context.Context, _ json.RawMessage) (any, error) {
			dl, _ := ctx.Deadline()
			gotDeadline <- dl
			return "ok", nil
		})

	c := &Client{} // no ws — handleToolCall just logs the dropped result
	c.handleToolCall(Frame{Type: FrameToolCall, Tool: "remote_subagent", CallID: "x"})

	select {
	case dl := <-gotDeadline:
		remaining := time.Until(dl)
		if remaining <= 60*time.Minute {
			t.Fatalf("remote_subagent dispatched with only %s of ctx budget; must exceed 60m", remaining)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("handler never ran")
	}
}
