package tools

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"runtime"
	"strings"
	"sync"
	"time"

	"github.com/snth-ai/snth-companion/internal/approval"
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

	if !shortcutApproved(a.Name) {
		summary := fmt.Sprintf("Run shortcut %q", a.Name)
		if a.Input != "" {
			summary += "\nwith input:\n    " + truncate(a.Input, 120)
		}
		ok, err := approval.Request(ctx, approval.Request_{Summary: summary, Danger: "prompt"})
		if err != nil {
			return nil, fmt.Errorf("approval: %w", err)
		}
		if !ok {
			return nil, fmt.Errorf("user denied")
		}
		rememberShortcutApproval(a.Name)
	}

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

func shortcutApproved(name string) bool {
	shortcutApprovalsMu.Lock()
	defer shortcutApprovalsMu.Unlock()
	_, ok := shortcutApprovals[name]
	return ok
}

func rememberShortcutApproval(name string) {
	shortcutApprovalsMu.Lock()
	defer shortcutApprovalsMu.Unlock()
	shortcutApprovals[name] = struct{}{}
}
