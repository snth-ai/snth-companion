package tools

import (
	"bytes"
	"context"
	"fmt"
	"os/exec"
	"runtime"
	"strings"
	"time"
)

// applescript.go — shared helper for the Apple-app wave of tools
// (calendar, notes, reminders, contacts). All of them shell out to
// `osascript` and parse the text back; this file centralises the
// plumbing: context-scoped timeouts, stderr capture + truncation,
// non-darwin fallback, and a tiny AppleScript string-escape helper.
//
// Why AppleScript and not EventKit/NotesKit via a Swift helper? Three
// reasons: (1) no binary dependencies beyond osascript, which ships
// with macOS; (2) no entitlements beyond the standard Automation
// permission the user grants interactively; (3) keeps the companion
// a single static Go binary so notarization is straightforward. The
// tradeoff is perf — osascript is ~150ms warm, so chatty workflows
// suffer. Fine for v1; if we hit pain we'll ship a Swift helper later.

const (
	osascriptMaxStderr = 4 * 1024
	osascriptDefaultTimeout = 20 * time.Second
)

// RunAppleScript executes src via `osascript -` and returns stdout.
// The context bound on a timeout bounds how long we'll wait; on
// cancel, osascript is killed via CommandContext.
func RunAppleScript(ctx context.Context, src string) (string, error) {
	if runtime.GOOS != "darwin" {
		return "", fmt.Errorf("AppleScript is only available on macOS")
	}

	cctx, cancel := context.WithTimeout(ctx, osascriptDefaultTimeout)
	defer cancel()

	cmd := exec.CommandContext(cctx, "osascript", "-")
	cmd.Stdin = strings.NewReader(src)
	var stdout, stderr bytes.Buffer
	cmd.Stdout = &stdout
	cmd.Stderr = &stderr
	err := cmd.Run()

	if cctx.Err() == context.DeadlineExceeded {
		return "", fmt.Errorf("osascript timeout after %s", osascriptDefaultTimeout)
	}
	if err != nil {
		errMsg := strings.TrimSpace(stderr.String())
		if len(errMsg) > osascriptMaxStderr {
			errMsg = errMsg[:osascriptMaxStderr-1] + "…"
		}
		return "", fmt.Errorf("osascript: %w (stderr: %s)", err, errMsg)
	}
	return strings.TrimRight(stdout.String(), "\n"), nil
}

// EscapeAppleScriptString escapes a Go string for AppleScript literal
// context. AppleScript doubles quotes for literals but has no general
// backslash-escape mechanism inside strings; the safe pattern is to
// break the string at every double-quote and concatenate with &
// character classes:
//
//     original: hello "world"\n
//     escaped:  "hello " & quote & "world" & quote & linefeed
//
// We only emit the simplest case (doubles + newlines) since arbitrary
// binary input isn't expected. Bad input fails loudly at the parse
// step rather than silently corrupting the AS source.
func EscapeAppleScriptString(s string) string {
	if s == "" {
		return `""`
	}
	var b strings.Builder
	b.WriteString(`"`)
	first := true
	chunks := strings.Split(s, "\n")
	for i, line := range chunks {
		if i > 0 {
			if !first {
				b.WriteString(`"`)
			}
			b.WriteString(` & linefeed & `)
			b.WriteString(`"`)
			first = false
		}
		// Inside a chunk, escape " by breaking and concatenating
		// with `quote`.
		parts := strings.Split(line, `"`)
		for j, p := range parts {
			if j > 0 {
				b.WriteString(`" & quote & "`)
			}
			b.WriteString(p)
		}
		first = false
	}
	b.WriteString(`"`)
	return b.String()
}
