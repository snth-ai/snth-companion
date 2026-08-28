package tools

// F5 regression: upload_to_synth used to be DangerLevel "safe" with no
// GatePolicy, so Dispatch NEVER consulted the approval fn — a
// compromised/prompt-injected synth could call
// upload_to_synth{path:"/Users/<u>/.ssh/id_rsa"} and stream the file's
// BYTES off the Mac with zero approval. "read-only on the Mac" was the
// exfiltration primitive. These tests exercise the REAL Dispatch gate +
// REAL approval.Request + REAL trust.Store (same harness as the fs gate
// tests), proving out-of-sandbox uploads now prompt (never auto-approved
// under master trust) and inside-sandbox uploads still GateSkip.

import (
	"encoding/json"
	"os"
	"testing"

	"github.com/snth-ai/snth-companion/internal/config"
)

// TestUploadOutOfSandboxIsGated is the F5 regression proper: an
// out-of-sandbox upload_to_synth must reach the approval gate at
// always-prompt danger and NOT be auto-approved under master trust.
//
// RED PROOF: revert the descriptor to DangerLevel:"safe" (drop
// GatePolicy/ApprovalSummary) and this fails — the audit hook never fires
// (Dispatch GateSkip's the "safe" tool) so snap.seen is false and the
// arbitrary path would be shipped off the Mac unapproved.
func TestUploadOutOfSandboxIsGated(t *testing.T) {
	config.Load()
	if err := config.Update(func(c *config.Config) { c.SandboxRoots = []string{} }); err != nil {
		t.Fatalf("config update: %v", err)
	}

	RegisterUpload()
	rec, ctx := realApprovalHarness(t)

	args, _ := json.Marshal(map[string]any{
		"path":      "/etc/passwd",
		"upload_id": "u-f5",
	})
	out, err := Dispatch(ctx, "upload_to_synth", json.RawMessage(args))

	snap := rec.snapshot()
	if !snap.seen {
		t.Fatalf("F5 REGRESSION: upload_to_synth ran WITHOUT any approval (audit never fired) — arbitrary file shipped off the Mac; out=%v err=%v", out, err)
	}
	if snap.danger != "always-prompt" {
		t.Fatalf("F5: upload_to_synth gated at danger=%q, want always-prompt for out-of-sandbox", snap.danger)
	}
	if snap.source == "trusted" || snap.decision == "approved" {
		t.Fatalf("F5: out-of-sandbox upload was auto-approved (source=%q decision=%q), must prompt", snap.source, snap.decision)
	}
	if err == nil {
		t.Fatalf("F5: expected denial after prompt, handler appears to have run (out=%v)", out)
	}
}

// TestUploadInsideSandboxSkipsGate proves the fix does NOT add friction to
// the normal #130 companion:// flow: an upload of a file inside the
// sandbox returns GateSkip and Dispatch never calls the approval fn. The
// handler then early-returns ("not paired") because no PairedSynthURL is
// set — no network, no goroutine — which is irrelevant to the gate
// assertion (rec must not have been consulted).
func TestUploadInsideSandboxSkipsGate(t *testing.T) {
	config.Load()
	root := t.TempDir()
	if err := config.Update(func(c *config.Config) {
		c.SandboxRoots = []string{root}
		// Force the early "not paired" return so the handler never opens a
		// socket if it ran past the gate.
		c.PairedSynthURL = ""
		c.CompanionToken = ""
	}); err != nil {
		t.Fatalf("config update: %v", err)
	}

	inside := root + "/media/inbound/clip.mp4"
	if err := os.MkdirAll(root+"/media/inbound", 0o755); err != nil {
		t.Fatalf("mkdir: %v", err)
	}
	if err := writeFileForTest(inside, "video-bytes"); err != nil {
		t.Fatalf("seed file: %v", err)
	}

	RegisterUpload()
	rec, ctx := realApprovalHarness(t)

	args, _ := json.Marshal(map[string]any{
		"path":      inside,
		"upload_id": "u-inside",
	})
	_, _ = Dispatch(ctx, "upload_to_synth", json.RawMessage(args))

	if rec.snapshot().seen {
		t.Fatalf("inside-sandbox upload prompted — must GateSkip, no dialog/audit")
	}
}
