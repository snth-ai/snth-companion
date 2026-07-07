package tools

// These tests exercise the REAL fs tools through the REAL Dispatch gate
// wired to the REAL approval.Request + REAL trust.Store — NOT fake tools.
// The existing dispatch_gate_test.go used throwaway fakes and was therefore
// blind to F1 (remote_fs_list dropped all enforcement) and F2 (the gate's
// always-prompt escalation was silently defeated by master-trust). These
// tests reproduce both holes end to end.
//
// Wiring mirrors cmd/companion/main.go exactly: tools.SetApprovalFn forwards
// tool/summary/danger/path into the REAL approval.Request, which consults
// the REAL trust.Store. To keep the dialog out of CI we pass a context with
// an already-expired deadline: approval.Request auto-approves/denies via
// trust WITHOUT ever consulting the deadline, but if trust returns Prompt it
// hits the osascript path where the expired deadline makes it return
// immediately (audit source "prompt-timeout") instead of hanging on a GUI
// dialog. An audit hook records the decision source so a test can tell
// "auto-approved by trust" (the F2 hole) apart from "prompted".

import (
	"context"
	"encoding/json"
	"os"
	"sync"
	"testing"
	"time"

	"github.com/snth-ai/snth-companion/internal/approval"
	"github.com/snth-ai/snth-companion/internal/config"
	"github.com/snth-ai/snth-companion/internal/trust"
)

func writeFileForTest(path, content string) error {
	return os.WriteFile(path, []byte(content), 0o600)
}

// auditRec captures approval.Request's resolved decision + source.
type auditRec struct {
	mu       sync.Mutex
	seen     bool
	tool     string
	decision string // "approved" | "denied"
	source   string // "trusted" | "prompt" | "prompt-timeout" | "non-darwin" | ...
	danger   string // effective danger the gate passed
}

func (a *auditRec) snapshot() auditRec {
	a.mu.Lock()
	defer a.mu.Unlock()
	return auditRec{seen: a.seen, tool: a.tool, decision: a.decision, source: a.source, danger: a.danger}
}

// realApprovalHarness installs a real trust.Store (master ON, empty write
// roots) behind approval.Request, wires the production tools approval fn,
// and records every resolved decision via the audit hook. Returns the
// recorder + a context whose deadline is already expired so the osascript
// dialog path returns instantly instead of blocking.
func realApprovalHarness(t *testing.T) (*auditRec, context.Context) {
	t.Helper()

	ts, err := trust.NewStore(t.TempDir())
	if err != nil {
		t.Fatalf("new trust store: %v", err)
	}
	if err := ts.SetMaster(true, nil); err != nil {
		t.Fatalf("set master: %v", err)
	}
	approval.SetTrustStore(ts)
	t.Cleanup(func() { approval.SetTrustStore(nil) })

	rec := &auditRec{}
	approval.SetAuditHook(func(tool, summary, decision, source, errMsg string) {
		rec.mu.Lock()
		rec.seen = true
		rec.tool = tool
		rec.decision = decision
		rec.source = source
		rec.mu.Unlock()
	})
	t.Cleanup(func() { approval.SetAuditHook(nil) })

	// Production wiring, with the effective danger also stamped on rec so a
	// test can assert the gate escalated to always-prompt.
	SetApprovalFn(func(ctx context.Context, tool, summary, danger, path string) (bool, error) {
		rec.mu.Lock()
		rec.danger = danger
		rec.mu.Unlock()
		return approval.Request(ctx, approval.Request_{
			Tool:    tool,
			Summary: summary,
			Danger:  danger,
			Path:    path,
		})
	})
	t.Cleanup(func() { SetApprovalFn(nil) })

	// Expired-deadline context: trust auto-decisions ignore it; the dialog
	// path returns immediately (DeadlineExceeded → denied) with no GUI.
	ctx, cancel := context.WithDeadline(context.Background(), time.Now().Add(-time.Second))
	t.Cleanup(cancel)
	return rec, ctx
}

// TestFsListOutOfSandboxIsGated is the F1 regression: remote_fs_list used to
// be DangerLevel "safe" with no GatePolicy, so Dispatch NEVER called the
// approval fn — arbitrary directory listing (~/.ssh, /etc) with zero
// approval and zero sandbox check. With the fix it behaves like fs_read.
//
// RED PROOF: revert remote_fs_list to DangerLevel:"safe" (no GatePolicy) and
// this fails — the audit hook is never invoked (gate GateSkip'd) and the
// handler lists /etc unapproved (err == nil).
func TestFsListOutOfSandboxIsGated(t *testing.T) {
	config.Load()
	if err := config.Update(func(c *config.Config) { c.SandboxRoots = []string{} }); err != nil {
		t.Fatalf("config update: %v", err)
	}

	RegisterFS()
	rec, ctx := realApprovalHarness(t)

	out, err := Dispatch(ctx, "remote_fs_list", json.RawMessage(`{"path":"/etc"}`))

	snap := rec.snapshot()
	if !snap.seen {
		t.Fatalf("F1 REGRESSION: remote_fs_list ran WITHOUT any approval (audit never fired); out=%v err=%v", out, err)
	}
	if snap.danger != "always-prompt" {
		t.Fatalf("F1: remote_fs_list gated at danger=%q, want always-prompt for out-of-sandbox", snap.danger)
	}
	if snap.source == "trusted" || snap.decision == "approved" {
		t.Fatalf("F1: out-of-sandbox list was auto-approved (source=%q decision=%q), must prompt", snap.source, snap.decision)
	}
	if err == nil {
		t.Fatalf("F1: expected denial after prompt, handler appears to have run (out=%v)", out)
	}
}

// TestFsReadOutOfSandboxNotAutoApprovedUnderMaster is the F2 regression:
// with master-trust ON and empty AllowedWriteRoots, an out-of-sandbox
// remote_fs_read (gate → GateAlwaysPrompt) must be PROMPTED, not
// auto-approved. Pre-fix approval.Request ignored r.Danger and trust.Get
// returned Trusted under master (remote_fs_read was not in AlwaysPromptTools)
// → silent auto-approve of ~/.ssh/id_rsa reads.
//
// RED PROOF: in approval.Request, replace ts.GetDanger(...) with ts.Get(...)
// (danger-blind) AND remove remote_fs_read from trust.AlwaysPromptTools —
// then the audit source is "trusted" / decision "approved" and this fails.
func TestFsReadOutOfSandboxNotAutoApprovedUnderMaster(t *testing.T) {
	config.Load()
	if err := config.Update(func(c *config.Config) { c.SandboxRoots = []string{} }); err != nil {
		t.Fatalf("config update: %v", err)
	}

	RegisterFS()
	rec, ctx := realApprovalHarness(t)

	out, err := Dispatch(ctx, "remote_fs_read", json.RawMessage(`{"path":"/etc/passwd"}`))

	snap := rec.snapshot()
	if !snap.seen {
		t.Fatalf("F2: remote_fs_read never reached approval (gate GateSkip'd an out-of-sandbox read)")
	}
	if snap.danger != "always-prompt" {
		t.Fatalf("F2: gate passed danger=%q, want always-prompt for out-of-sandbox read", snap.danger)
	}
	if snap.source == "trusted" || snap.decision == "approved" {
		t.Fatalf("F2 REGRESSION: out-of-sandbox remote_fs_read AUTO-APPROVED under master-trust (source=%q decision=%q); must prompt", snap.source, snap.decision)
	}
	if err == nil {
		t.Fatalf("F2: expected denial after prompt, handler appears to have run (out=%v)", out)
	}
}

// TestFsReadInsideSandboxSkipsGate proves the fix does NOT over-prompt:
// inside the sandbox the gate returns GateSkip and Dispatch never calls the
// approval fn (unchanged; no double-prompt).
func TestFsReadInsideSandboxSkipsGate(t *testing.T) {
	config.Load()
	root := t.TempDir()
	if err := config.Update(func(c *config.Config) { c.SandboxRoots = []string{root} }); err != nil {
		t.Fatalf("config update: %v", err)
	}

	inside := root + "/hello.txt"
	if err := writeFileForTest(inside, "hi"); err != nil {
		t.Fatalf("seed file: %v", err)
	}

	RegisterFS()
	rec, ctx := realApprovalHarness(t)

	args, _ := json.Marshal(map[string]string{"path": inside})
	if _, err := Dispatch(ctx, "remote_fs_read", json.RawMessage(args)); err != nil {
		t.Fatalf("inside-sandbox read should succeed, got err=%v", err)
	}
	if rec.snapshot().seen {
		t.Fatalf("inside-sandbox read prompted — must GateSkip, no dialog/audit")
	}
}
