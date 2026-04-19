// Package approval shows the user a native macOS dialog asking whether to
// permit a potentially-risky operation (bash command outside sandbox, write
// to a path not in the granted roots, etc).
//
// Day-1 impl: osascript shells out to "display dialog". Blocks for up to
// 30s. If the user ignores the prompt, the request is denied on timeout.
//
// Later: nicer native bridge via a tiny Swift helper with structured arg
// handling, and a bulk-approve UI that remembers "Always allow git in ~/".
package approval

import (
	"context"
	"fmt"
	"os"
	"os/exec"
	"runtime"
	"strings"
	"sync"
	"time"
)

// Request_ is the payload passed into Request (trailing underscore avoids
// colliding with the method name on the package).
type Request_ struct {
	Summary string
	Danger  string // "safe" | "prompt" | "always-prompt"
	Details string // optional longer context (not shown in Day-1 impl)
}

const (
	defaultTimeout = 30 * time.Second
)

var (
	// mu serializes prompts so only one dialog is on screen at once;
	// multiple concurrent RPCs queue behind each other.
	mu sync.Mutex

	// bypass, when true, auto-approves everything. Set via SNTH_COMPANION_BYPASS_APPROVAL=1
	// for local development and CI. Never enable in shipped builds.
	bypass = os.Getenv("SNTH_COMPANION_BYPASS_APPROVAL") == "1"
)

// Request pops up a native approval dialog and returns whether the user
// pressed "Approve". An error is returned only on unexpected failures
// (osascript not available, dialog was killed abnormally, etc).
func Request(ctx context.Context, r Request_) (bool, error) {
	if bypass {
		return true, nil
	}
	mu.Lock()
	defer mu.Unlock()

	cctx, cancel := context.WithTimeout(ctx, defaultTimeout)
	defer cancel()

	if runtime.GOOS != "darwin" {
		// Non-mac fallback: deny by default. We'll add real Windows/Linux
		// prompts when those targets ship.
		return false, nil
	}

	// osascript returns "button returned:Approve" on approve, or errors
	// (non-zero exit) on deny/cancel/timeout.
	script := fmt.Sprintf(
		`display dialog %s with title "SNTH Companion" buttons {"Deny", "Approve"} default button "Deny" with icon caution giving up after 25`,
		quote(r.Summary),
	)
	cmd := exec.CommandContext(cctx, "osascript", "-e", script)
	out, err := cmd.CombinedOutput()
	if cctx.Err() == context.DeadlineExceeded {
		return false, nil
	}
	if err != nil {
		// osascript returns code 1 on Deny; treat that as !ok, not error.
		if ee, ok := err.(*exec.ExitError); ok && ee.ExitCode() == 1 {
			return false, nil
		}
		return false, fmt.Errorf("osascript: %w (out: %s)", err, string(out))
	}
	return strings.Contains(string(out), "Approve"), nil
}

// quote wraps a string in AppleScript-safe double quotes with escapes.
func quote(s string) string {
	s = strings.ReplaceAll(s, "\\", "\\\\")
	s = strings.ReplaceAll(s, "\"", "\\\"")
	return "\"" + s + "\""
}
