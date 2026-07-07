//go:build darwin

package tools

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"os/exec"
	"runtime"
	"strings"
	"time"
)

// Tiny tools that don't fit elsewhere:
//   remote_clipboard_read / remote_clipboard_write — pbpaste/pbcopy
//   remote_notify — osascript "display notification"
//
// No AppleScript needed for clipboard (plain CLI tools). Notify uses
// osascript but the payload is trivial, no JSON marshaling dance.

func RegisterClipboard() {
	if runtime.GOOS != "darwin" {
		return
	}
	Register(Descriptor{
		Name:            "remote_clipboard_read",
		Description:     "Read the current clipboard contents on the paired Mac. Text only; rich content returns the text representation.",
		DangerLevel:     "prompt",
		ApprovalSummary: clipboardReadSummary,
	}, clipboardReadHandler)
	Register(Descriptor{
		Name:            "remote_clipboard_write",
		Description:     "Write text to the clipboard on the paired Mac. Overwrites whatever is there.",
		DangerLevel:     "prompt",
		ApprovalSummary: clipboardWriteSummary,
	}, clipboardWriteHandler)
}

func RegisterNotify() {
	if runtime.GOOS != "darwin" {
		return
	}
	Register(Descriptor{
		Name:        "remote_notify",
		Description: "Show a native notification on the paired Mac's Notification Center. Quiet — doesn't open any window. Good for proactive pings.",
		DangerLevel: "safe",
	}, notifyHandler)
}

// --- clipboard read ---------------------------------------------------------

type clipboardReadArgs struct{}

type clipboardReadResult struct {
	Content   string `json:"content"`
	Bytes     int    `json:"bytes"`
	Truncated bool   `json:"truncated,omitempty"`
}

const clipboardMaxRead = 512 * 1024

func clipboardReadSummary(_ json.RawMessage) (string, string) {
	return "Read the clipboard contents", ""
}

func clipboardReadHandler(ctx context.Context, _ json.RawMessage) (any, error) {
	cctx, cancel := context.WithTimeout(ctx, 5*time.Second)
	defer cancel()
	var out bytes.Buffer
	cmd := exec.CommandContext(cctx, "pbpaste")
	cmd.Stdout = &out
	if err := cmd.Run(); err != nil {
		return nil, fmt.Errorf("pbpaste: %w", err)
	}
	s := out.String()
	trunc := false
	if len(s) > clipboardMaxRead {
		s = s[:clipboardMaxRead]
		trunc = true
	}
	return clipboardReadResult{Content: s, Bytes: out.Len(), Truncated: trunc}, nil
}

// --- clipboard write --------------------------------------------------------

type clipboardWriteArgs struct {
	Content string `json:"content"`
}

func clipboardWriteHandler(ctx context.Context, raw json.RawMessage) (any, error) {
	var a clipboardWriteArgs
	if err := json.Unmarshal(raw, &a); err != nil {
		return nil, fmt.Errorf("bad args: %w", err)
	}
	cctx, cancel := context.WithTimeout(ctx, 5*time.Second)
	defer cancel()
	cmd := exec.CommandContext(cctx, "pbcopy")
	cmd.Stdin = strings.NewReader(a.Content)
	if err := cmd.Run(); err != nil {
		return nil, fmt.Errorf("pbcopy: %w", err)
	}
	return map[string]any{"bytes": len(a.Content)}, nil
}

// --- notify -----------------------------------------------------------------

type notifyArgs struct {
	Title    string `json:"title"`
	Subtitle string `json:"subtitle,omitempty"`
	Message  string `json:"message,omitempty"`
	Sound    string `json:"sound,omitempty"` // "Glass"|"Ping"|etc; empty = no sound
}

func notifyHandler(ctx context.Context, raw json.RawMessage) (any, error) {
	var a notifyArgs
	if err := json.Unmarshal(raw, &a); err != nil {
		return nil, fmt.Errorf("bad args: %w", err)
	}
	a.Title = strings.TrimSpace(a.Title)
	if a.Title == "" {
		return nil, fmt.Errorf("title is required")
	}
	// No approval prompt — notifications are low-impact and prompting
	// for each one would be absurd UX.

	var parts []string
	parts = append(parts, "display notification "+EscapeAppleScriptString(a.Message))
	parts = append(parts, "with title "+EscapeAppleScriptString(a.Title))
	if a.Subtitle != "" {
		parts = append(parts, "subtitle "+EscapeAppleScriptString(a.Subtitle))
	}
	if a.Sound != "" {
		parts = append(parts, "sound name "+EscapeAppleScriptString(a.Sound))
	}
	src := strings.Join(parts, " ")
	if _, err := RunAppleScript(ctx, src); err != nil {
		return nil, err
	}
	return map[string]any{"ok": true, "title": a.Title}, nil
}

// clipboardWriteSummary renders the approval dialog text for remote_clipboard_write.
func clipboardWriteSummary(raw json.RawMessage) (string, string) {
	var a clipboardWriteArgs
	if err := json.Unmarshal(raw, &a); err != nil {
		return "", ""
	}
	preview := a.Content
	if len(preview) > 120 {
		preview = preview[:120] + "…"
	}
	return "Write to clipboard:\n    " + strings.ReplaceAll(preview, "\n", " "), ""
}
