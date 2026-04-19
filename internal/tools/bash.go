package tools

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"os/exec"
	"strings"
	"time"

	"github.com/snth-ai/snth-companion/internal/approval"
	"github.com/snth-ai/snth-companion/internal/config"
	"github.com/snth-ai/snth-companion/internal/sandbox"
)

// BashArgs is the RPC arg shape for remote_bash. "cmd" is canonical;
// "command" is accepted as an alias because LLMs habitually emit it
// (matching Anthropic's built-in bash tool). Same for "dir" → "cwd".
type BashArgs struct {
	Cmd       string `json:"cmd"`
	CmdAlias  string `json:"command,omitempty"`
	Cwd       string `json:"cwd,omitempty"`
	CwdAlias  string `json:"dir,omitempty"`
	TimeoutMs int    `json:"timeout_ms,omitempty"`
}

func (a *BashArgs) normalize() {
	if a.Cmd == "" && a.CmdAlias != "" {
		a.Cmd = a.CmdAlias
	}
	if a.Cwd == "" && a.CwdAlias != "" {
		a.Cwd = a.CwdAlias
	}
}

// BashResult is what we return to the synth.
type BashResult struct {
	ExitCode  int    `json:"exit_code"`
	Stdout    string `json:"stdout"`
	Stderr    string `json:"stderr"`
	DurationMs int64 `json:"duration_ms"`
	Truncated bool   `json:"truncated,omitempty"`
}

const (
	bashMaxOutputBytes = 64 * 1024
	bashDefaultTimeout = 30 * time.Second
	bashMaxTimeout     = 5 * time.Minute
)

// RegisterBash wires remote_bash into the tool registry.
func RegisterBash() {
	Register(Descriptor{
		Name:        "remote_bash",
		Description: "Run a bash command on the paired Mac. Default cwd is the synth's sandbox root. Out-of-sandbox cwd requires user approval.",
		DangerLevel: "prompt",
	}, bashHandler)
}

func bashHandler(ctx context.Context, raw json.RawMessage) (any, error) {
	var args BashArgs
	if err := json.Unmarshal(raw, &args); err != nil {
		return nil, fmt.Errorf("bad args: %w", err)
	}
	args.normalize()
	args.Cmd = strings.TrimSpace(args.Cmd)
	if args.Cmd == "" {
		return nil, fmt.Errorf("cmd is required")
	}

	cfg := config.Get()

	// Pick cwd: user-provided, or default sandbox root.
	cwd := args.Cwd
	if cwd == "" {
		cwd = config.DefaultSandboxRoot(cfg)
		if cwd == "" {
			return nil, fmt.Errorf("no sandbox root — companion not paired")
		}
		_ = sandbox.EnsureDir(cwd)
	}
	resolvedCwd, err := sandbox.Resolve(cwd)
	if err != nil {
		return nil, fmt.Errorf("resolve cwd: %w", err)
	}

	inside := sandbox.InsideAny(cfg.SandboxRoots, resolvedCwd)
	autoApproved := isAutoApprovedCmd(cfg.AutoApproveBashPatterns, args.Cmd)
	if !inside || !autoApproved {
		summary := fmt.Sprintf("Run bash command in %s:\n    %s", resolvedCwd, truncate(args.Cmd, 200))
		danger := "prompt"
		if !inside {
			danger = "always-prompt"
		}
		ok, err := approval.Request(ctx, approval.Request_{
			Summary: summary,
			Danger:  danger,
		})
		if err != nil {
			return nil, fmt.Errorf("approval: %w", err)
		}
		if !ok {
			return nil, fmt.Errorf("user denied")
		}
	}

	// Build command with timeout.
	timeout := bashDefaultTimeout
	if args.TimeoutMs > 0 {
		t := time.Duration(args.TimeoutMs) * time.Millisecond
		if t > bashMaxTimeout {
			t = bashMaxTimeout
		}
		timeout = t
	}
	cctx, cancel := context.WithTimeout(ctx, timeout)
	defer cancel()

	cmd := exec.CommandContext(cctx, "bash", "-c", args.Cmd)
	cmd.Dir = resolvedCwd

	var stdout, stderr bytes.Buffer
	cmd.Stdout = &capBuffer{buf: &stdout, max: bashMaxOutputBytes}
	cmd.Stderr = &capBuffer{buf: &stderr, max: bashMaxOutputBytes}

	start := time.Now()
	runErr := cmd.Run()
	duration := time.Since(start)

	exitCode := 0
	if runErr != nil {
		if ee, ok := runErr.(*exec.ExitError); ok {
			exitCode = ee.ExitCode()
		} else if cctx.Err() == context.DeadlineExceeded {
			exitCode = 124 // coreutils `timeout` convention
			stderr.WriteString("\n[companion] timeout after ")
			stderr.WriteString(timeout.String())
		} else {
			return nil, fmt.Errorf("exec: %w", runErr)
		}
	}

	return BashResult{
		ExitCode:   exitCode,
		Stdout:     stdout.String(),
		Stderr:     stderr.String(),
		DurationMs: duration.Milliseconds(),
		Truncated:  stdout.Len() >= bashMaxOutputBytes || stderr.Len() >= bashMaxOutputBytes,
	}, nil
}

func isAutoApprovedCmd(patterns []string, cmd string) bool {
	trimmed := strings.TrimSpace(cmd)
	for _, p := range patterns {
		if strings.HasPrefix(trimmed, p) {
			return true
		}
	}
	return false
}

func truncate(s string, n int) string {
	if len(s) <= n {
		return s
	}
	return s[:n-1] + "…"
}

// capBuffer is an io.Writer that caps the total bytes written into buf.
// Once the cap is reached, further writes are silently dropped.
type capBuffer struct {
	buf *bytes.Buffer
	max int
}

func (c *capBuffer) Write(p []byte) (int, error) {
	remaining := c.max - c.buf.Len()
	if remaining <= 0 {
		return len(p), nil
	}
	if len(p) > remaining {
		c.buf.Write(p[:remaining])
		return len(p), nil
	}
	return c.buf.Write(p)
}
