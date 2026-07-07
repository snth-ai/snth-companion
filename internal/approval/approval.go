// Package approval shows the user a native macOS dialog asking whether
// to permit a potentially-risky operation, OR auto-approves/auto-denies
// based on the user's trust profile.
//
// The dialog still uses osascript "display dialog" — same as before.
// The new layer in front consults the trust package: each tool now
// passes its `Tool` name (e.g. "remote_bash", "fs_write"), and the
// trust store decides {Trusted, Denied, Prompt} from per-tool overrides
// + master toggle + write-root scopes + critical-tool safety net.
//
// Every call lands in the audit ring buffer so the user can inspect
// "what did the synth just try and what did the system decide" in the
// Privacy / Audit UI.
package approval

import (
	"context"
	"fmt"
	"os/exec"
	"runtime"
	"strings"
	"sync"
	"time"

	"github.com/snth-ai/snth-companion/internal/trust"
)

// Request_ is the payload passed into Request (trailing underscore
// avoids colliding with the method name on the package).
type Request_ struct {
	// Tool is the canonical short id used in the trust store key
	// (e.g. "remote_bash", "fs_write", "notes_create",
	// "messages_send", "subagent"). MUST be set by callers — empty
	// Tool falls back to legacy "always-prompt" behavior so old
	// call sites keep working until updated.
	Tool string

	Summary string
	Danger  string // "safe" | "prompt" | "always-prompt"
	Details string

	// Path, when set, gates fs_write-style tools: trust store checks
	// it against AllowedWriteRoots before honoring trusted mode.
	Path string
}

const (
	defaultTimeout = 30 * time.Second
)

var (
	// mu serializes prompts so only one dialog is on screen at once;
	// concurrent RPCs queue behind each other.
	mu sync.Mutex

	// bypass — the legacy dev escape hatch. Its VALUE is decided at
	// COMPILE time: in a release build (no `dev` build tag) bypassValue is
	// the constant false, so SNTH_COMPANION_BYPASS_APPROVAL is inert even
	// if it leaks into a shipped plist / inherited env (A7). Only a
	// dev-tagged build reads the env. See bypass_release.go / bypass_dev.go.
	bypass = bypassValue

	// trustStore is set once at boot via SetTrustStore. nil-safe — when
	// nil, all decisions fall through to the legacy prompt path (no
	// trust evaluation, no auto-approve).
	trustStore *trust.Store
	trustMu    sync.RWMutex
)

// BypassActive reports whether the compiled-in approval bypass is on.
// Always false in release builds (built without `-tags dev`).
func BypassActive() bool { return bypassActive() }

// SetTrustStore wires the package-global trust store. Called once at
// boot from cmd/companion/main. Subsequent calls replace.
func SetTrustStore(s *trust.Store) {
	trustMu.Lock()
	defer trustMu.Unlock()
	trustStore = s
}

func currentTrust() *trust.Store {
	trustMu.RLock()
	defer trustMu.RUnlock()
	return trustStore
}

// AuditHook is called once per resolved Request. The companion's
// daemon.RecordAudit is wired here at boot; package approval avoids
// importing daemon to prevent an import cycle.
type AuditHook func(tool, summary, decision, source, errMsg string)

var (
	auditHook   AuditHook
	auditHookMu sync.RWMutex
)

func SetAuditHook(h AuditHook) {
	auditHookMu.Lock()
	defer auditHookMu.Unlock()
	auditHook = h
}

func emitAudit(tool, summary, decision, source, errMsg string) {
	auditHookMu.RLock()
	h := auditHook
	auditHookMu.RUnlock()
	if h != nil {
		h(tool, summary, decision, source, errMsg)
	}
}

// Request evaluates the trust profile, then either auto-approves,
// auto-denies, or pops a dialog. Returns (allowed, error). Error is
// reserved for unexpected failures (osascript missing, etc.); a regular
// "user clicked Deny" is (false, nil).
func Request(ctx context.Context, r Request_) (bool, error) {
	if bypass {
		emitAudit(r.Tool, r.Summary, "approved", "env-bypass", "")
		return true, nil
	}

	// Trust evaluation. Empty Tool → legacy path (treats every call
	// as prompt-required) preserving the old behavior for tools that
	// haven't yet been updated to pass Tool.
	//
	// alwaysPrompt: the CALLER/gate declared this specific invocation
	// always-prompt (e.g. the Dispatch gate escalated an out-of-sandbox
	// remote_fs_read/write/bash/browser to GateAlwaysPrompt). That must
	// defeat the master auto-approve exactly as if the tool NAME were in
	// trust.AlwaysPromptTools — otherwise a conditional tool whose name is
	// not hardcoded in that set (remote_fs_read out-of-sandbox, etc.)
	// would be silently auto-approved under master-trust, defeating the
	// gate's escalation. Only an EXPLICIT per-tool ModeTrusted override
	// (the user opting in for that tool) may still auto-approve silently;
	// trust.Get already honors that ahead of the master path.
	alwaysPrompt := r.Danger == "always-prompt"
	if r.Tool != "" {
		if ts := currentTrust(); ts != nil {
			switch ts.GetDanger(r.Tool, r.Path, alwaysPrompt) {
			case trust.DecisionTrusted:
				emitAudit(r.Tool, r.Summary, "approved", "trusted", "")
				return true, nil
			case trust.DecisionDenied:
				emitAudit(r.Tool, r.Summary, "denied", "trusted-deny", "")
				return false, nil
			}
		}
	}

	mu.Lock()
	defer mu.Unlock()

	cctx, cancel := context.WithTimeout(ctx, defaultTimeout)
	defer cancel()

	if runtime.GOOS != "darwin" {
		emitAudit(r.Tool, r.Summary, "denied", "non-darwin", "")
		return false, nil
	}

	script := fmt.Sprintf(
		`display dialog %s with title "SNTH Companion" buttons {"Deny", "Approve"} default button "Deny" with icon caution giving up after 25`,
		quote(r.Summary),
	)
	cmd := exec.CommandContext(cctx, "osascript", "-e", script)
	out, err := cmd.CombinedOutput()
	if cctx.Err() == context.DeadlineExceeded {
		emitAudit(r.Tool, r.Summary, "denied", "prompt-timeout", "")
		return false, nil
	}
	if err != nil {
		if ee, ok := err.(*exec.ExitError); ok && ee.ExitCode() == 1 {
			emitAudit(r.Tool, r.Summary, "denied", "prompt", "")
			return false, nil
		}
		emitAudit(r.Tool, r.Summary, "denied", "prompt-error", err.Error())
		return false, fmt.Errorf("osascript: %w (out: %s)", err, string(out))
	}
	if strings.Contains(string(out), "Approve") {
		emitAudit(r.Tool, r.Summary, "approved", "prompt", "")
		return true, nil
	}
	emitAudit(r.Tool, r.Summary, "denied", "prompt", "")
	return false, nil
}

func quote(s string) string {
	s = strings.ReplaceAll(s, "\\", "\\\\")
	s = strings.ReplaceAll(s, "\"", "\\\"")
	return "\"" + s + "\""
}
