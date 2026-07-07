//go:build darwin

package tools

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"runtime"
	"strings"
	"sync"
	"time"
)

// shortcut.go — macOS-only bridge to Apple Shortcuts.
//
// The user configures Shortcuts in the Shortcuts app (calendar add, notes
// create, whatever). The synth calls remote_shortcut with the shortcut's
// name and optional text input; we invoke `shortcuts run "<name>"
// --input-path <tmp> --output-path <tmp>` and return the output as a
// string.
//
// Approval: shortcuts are intrinsically user-gated (the user wrote/
// installed them), so we DON'T prompt on every invocation — that would
// be prompt fatigue. Instead we prompt only the first time a given
// shortcut is invoked in a session and remember approvals in-memory.
// Future: persist "always allow" per shortcut name in config.

const (
	shortcutDefaultTimeout = 60 * time.Second
	shortcutMaxTimeout     = 5 * time.Minute
	shortcutMaxInput       = 64 * 1024
	shortcutMaxOutput      = 256 * 1024
)

func RegisterShortcut() {
	if runtime.GOOS != "darwin" {
		// On non-darwin the tool would always error — skip registration.
		return
	}
	Register(Descriptor{
		Name:        "remote_shortcut",
		Description: "Run a user-configured Apple Shortcut on the paired Mac. The user sets up Shortcuts in Apple's Shortcuts app; `name` is the shortcut's exact title. Optional `input` is piped as stdin and forwarded with --input-path. Output is returned as a string (up to 256 KiB).",
		DangerLevel: "prompt",
		// Session-cache: the central gate prompts once per shortcut name
		// per session, then GateSkip on subsequent calls (approval is
		// recorded by the handler after a successful gated run).
		GatePolicy:      shortcutGatePolicy,
		ApprovalSummary: shortcutSummary,
	}, shortcutHandler)
}

type shortcutArgs struct {
	Name      string `json:"name"`
	Input     string `json:"input,omitempty"`
	TimeoutMs int    `json:"timeout_ms,omitempty"`
}

type shortcutResult struct {
	Name       string `json:"name"`
	Output     string `json:"output"`
	DurationMs int64  `json:"duration_ms"`
	Truncated  bool   `json:"truncated,omitempty"`
}

func shortcutHandler(ctx context.Context, raw json.RawMessage) (any, error) {
	var a shortcutArgs
	if err := json.Unmarshal(raw, &a); err != nil {
		return nil, fmt.Errorf("bad args: %w", err)
	}
	a.Name = strings.TrimSpace(a.Name)
	if a.Name == "" {
		return nil, fmt.Errorf("name is required")
	}
	if len(a.Input) > shortcutMaxInput {
		return nil, fmt.Errorf("input too large: %d bytes (max %d)", len(a.Input), shortcutMaxInput)
	}

	// Reaching the handler means the central gate already approved (or
	// GateSkip'd) this call. Record the approval so subsequent calls to
	// the SAME shortcut WITH THE SAME INPUT this session skip the prompt.
	// The cache is keyed on name + input hash, NOT name alone: a
	// Run-Shell-Script shortcut executes its stdin, so approving "X" once
	// must NOT bless "X" with arbitrary different attacker input (F3).
	rememberShortcutApproval(a.Name, a.Input)

	// Materialize input as a temp file (shortcuts run prefers file input
	// over stdin).
	inPath := ""
	if a.Input != "" {
		f, err := os.CreateTemp("", "snth-shortcut-in-*")
		if err != nil {
			return nil, fmt.Errorf("tmp input: %w", err)
		}
		defer os.Remove(f.Name())
		if _, err := f.WriteString(a.Input); err != nil {
			f.Close()
			return nil, fmt.Errorf("write input: %w", err)
		}
		f.Close()
		inPath = f.Name()
	}

	outFile, err := os.CreateTemp("", "snth-shortcut-out-*")
	if err != nil {
		return nil, fmt.Errorf("tmp output: %w", err)
	}
	outFile.Close()
	defer os.Remove(outFile.Name())

	timeout := shortcutDefaultTimeout
	if a.TimeoutMs > 0 {
		t := time.Duration(a.TimeoutMs) * time.Millisecond
		if t > shortcutMaxTimeout {
			t = shortcutMaxTimeout
		}
		timeout = t
	}
	cctx, cancel := context.WithTimeout(ctx, timeout)
	defer cancel()

	args := []string{"run", a.Name, "--output-path", outFile.Name()}
	if inPath != "" {
		args = append(args, "--input-path", inPath)
	}
	var stderr bytes.Buffer
	cmd := exec.CommandContext(cctx, "shortcuts", args...)
	cmd.Stderr = &stderr

	start := time.Now()
	if err := cmd.Run(); err != nil {
		if cctx.Err() == context.DeadlineExceeded {
			return nil, fmt.Errorf("timeout after %s", timeout)
		}
		return nil, fmt.Errorf("shortcuts run: %w (stderr: %s)", err, strings.TrimSpace(stderr.String()))
	}
	duration := time.Since(start)

	raw2, err := os.ReadFile(outFile.Name())
	if err != nil {
		return nil, fmt.Errorf("read output: %w", err)
	}
	truncated := false
	if len(raw2) > shortcutMaxOutput {
		raw2 = raw2[:shortcutMaxOutput]
		truncated = true
	}
	return shortcutResult{
		Name:       a.Name,
		Output:     string(raw2),
		DurationMs: duration.Milliseconds(),
		Truncated:  truncated,
	}, nil
}

// --- session approval cache -------------------------------------------------

var (
	shortcutApprovalsMu sync.Mutex
	shortcutApprovals   = map[string]struct{}{}
)

// shortcutCacheKey is the session-cache key: shortcut name + a hash of the
// exact input. Two calls to the same shortcut with different input produce
// different keys, so an approval for one input never blesses another (F3).
func shortcutCacheKey(name, input string) string {
	sum := sha256.Sum256([]byte(input))
	return name + "\x00" + hex.EncodeToString(sum[:])
}

func shortcutApproved(name, input string) bool {
	shortcutApprovalsMu.Lock()
	defer shortcutApprovalsMu.Unlock()
	_, ok := shortcutApprovals[shortcutCacheKey(name, input)]
	return ok
}

func rememberShortcutApproval(name, input string) {
	shortcutApprovalsMu.Lock()
	defer shortcutApprovalsMu.Unlock()
	shortcutApprovals[shortcutCacheKey(name, input)] = struct{}{}
}

// shortcutGatePolicy skips the prompt when this shortcut name was already
// approved earlier in the session WITH THE SAME INPUT; otherwise it
// requires a prompt. Keying on name+input means re-invoking an approved
// shortcut with different (attacker-chosen) input re-prompts.
func shortcutGatePolicy(raw json.RawMessage) GateDecision {
	var a shortcutArgs
	if err := json.Unmarshal(raw, &a); err != nil {
		return GatePrompt
	}
	if shortcutApproved(strings.TrimSpace(a.Name), a.Input) {
		return GateSkip
	}
	return GatePrompt
}

// shortcutSummary renders the approval dialog text for remote_shortcut.
func shortcutSummary(raw json.RawMessage) (string, string) {
	var a shortcutArgs
	if err := json.Unmarshal(raw, &a); err != nil {
		return "", ""
	}
	summary := fmt.Sprintf("Run shortcut %q", strings.TrimSpace(a.Name))
	if a.Input != "" {
		summary += "\nwith input:\n    " + truncate(a.Input, 120)
	}
	return summary, ""
}
