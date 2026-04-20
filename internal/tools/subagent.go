package tools

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"os/exec"
	"path/filepath"
	"strings"
	"time"

	"github.com/snth-ai/snth-companion/internal/approval"
)

// subagent.go — delegate a concrete coding mission to a local AI CLI
// (Claude Code or Codex). The synth calls this tool, we prompt the
// user once, then block up to ~60 minutes while the sub-agent runs
// on the user's Mac. Stdout + stderr + exit code + git-diff-stat
// come back to the synth so it can summarise what changed.
//
// Think "fork/join" — synth forks, subagent works, synth joins.
//
// Not for chat. The sub-agent CLI spins up, executes the task to
// completion (or timeout / error), exits. Each call is a fresh
// process. Sub-agents inherit the user's env — their own keys,
// auth, tooling all work.
//
// Approval: always-prompt on first call, because the sub-agent will
// read and modify files in cwd (and possibly run shell commands).
// We do NOT auto-approve even if cwd is inside the sandbox — the
// blast radius is too wide for the sandbox heuristic to cover.

const (
	subagentDefaultTimeout = 30 * time.Minute
	subagentMaxTimeout     = 60 * time.Minute
	subagentMaxStdout      = 1 * 1024 * 1024 // 1 MiB
	subagentMaxStderr      = 256 * 1024
)

func RegisterSubagent() {
	Register(Descriptor{
		Name:        "remote_subagent",
		Description: "Delegate a coding mission to Claude Code (claude) or Codex (codex) CLI on the user's Mac. Blocks synchronously until the sub-agent completes the task, then returns full transcript + git diff stat. Intended for concrete missions (e.g. 'refactor package X', 'implement feature Y', 'fix bug Z'), not for chat. Long-running — up to 60 minutes. ALWAYS prompts user approval before spawning.",
		DangerLevel: "always-prompt",
	}, subagentHandler)
}

type subagentArgs struct {
	Agent     string `json:"agent"`              // "claude" | "codex"
	Task      string `json:"task"`               // the prompt / mission
	Cwd       string `json:"cwd"`                // working directory (absolute)
	Model     string `json:"model,omitempty"`    // optional model hint
	MaxTurns  int    `json:"max_turns,omitempty"` // Claude only; default 40
	TimeoutMs int    `json:"timeout_ms,omitempty"`
}

type subagentResult struct {
	Agent       string `json:"agent"`
	Command     string `json:"command"` // the actual CLI invocation
	ExitCode    int    `json:"exit_code"`
	Stdout      string `json:"stdout"`
	Stderr      string `json:"stderr"`
	DurationMs  int64  `json:"duration_ms"`
	StdoutTrunc bool   `json:"stdout_truncated,omitempty"`
	StderrTrunc bool   `json:"stderr_truncated,omitempty"`
	GitDiffStat string `json:"git_diff_stat,omitempty"` // "3 files changed, 42 insertions(+), 5 deletions(-)" style
}

func subagentHandler(ctx context.Context, raw json.RawMessage) (any, error) {
	var a subagentArgs
	if err := json.Unmarshal(raw, &a); err != nil {
		return nil, fmt.Errorf("bad args: %w", err)
	}
	a.Agent = strings.ToLower(strings.TrimSpace(a.Agent))
	a.Task = strings.TrimSpace(a.Task)
	a.Cwd = strings.TrimSpace(a.Cwd)

	if a.Agent == "" {
		a.Agent = "claude"
	}
	if a.Task == "" {
		return nil, fmt.Errorf("task is required")
	}
	if a.Cwd == "" {
		return nil, fmt.Errorf("cwd is required — absolute path to the directory the subagent should operate in")
	}
	if !filepath.IsAbs(a.Cwd) {
		return nil, fmt.Errorf("cwd must be absolute; got %q", a.Cwd)
	}

	// Locate the binary up-front so we can surface a cleaner error
	// than whatever exec.Command would say.
	var bin string
	switch a.Agent {
	case "claude":
		bin = "claude"
	case "codex":
		bin = "codex"
	default:
		return nil, fmt.Errorf("unknown agent %q (valid: claude, codex)", a.Agent)
	}
	binPath, err := exec.LookPath(bin)
	if err != nil {
		return nil, fmt.Errorf("%s CLI not found in PATH on the paired Mac — install it first (claude: `brew install claude-code` or npm; codex: `brew install codex`)", bin)
	}

	// Approval is always required — blast radius is too big.
	preview := a.Task
	if len(preview) > 300 {
		preview = preview[:300] + "…"
	}
	summary := fmt.Sprintf(
		"Spawn %s sub-agent in %s\n\nTask:\n    %s\n\nThe sub-agent runs with --dangerously-skip-permissions and can read/write any file inside cwd (and possibly outside). Timeout ~%d min.",
		a.Agent, a.Cwd, strings.ReplaceAll(preview, "\n", "\n    "),
		int(subagentDefaultTimeout/time.Minute),
	)
	ok, err := approval.Request(ctx, approval.Request_{Summary: summary, Danger: "always-prompt"})
	if err != nil {
		return nil, fmt.Errorf("approval: %w", err)
	}
	if !ok {
		return nil, fmt.Errorf("user denied")
	}

	// Build the command. Claude Code and Codex have different flag shapes.
	var cmd *exec.Cmd
	cctx, cancel := context.WithTimeout(ctx, clampDuration(a.TimeoutMs, subagentDefaultTimeout, subagentMaxTimeout))
	defer cancel()

	switch a.Agent {
	case "claude":
		args := []string{
			"-p", a.Task,
			"--output-format", "text",
			"--dangerously-skip-permissions",
		}
		if a.Model != "" {
			args = append(args, "--model", a.Model)
		}
		if a.MaxTurns > 0 {
			args = append(args, "--max-turns", fmt.Sprintf("%d", a.MaxTurns))
		}
		cmd = exec.CommandContext(cctx, binPath, args...)
	case "codex":
		args := []string{"exec", a.Task}
		if a.Model != "" {
			args = append(args, "--model", a.Model)
		}
		cmd = exec.CommandContext(cctx, binPath, args...)
	}
	cmd.Dir = a.Cwd

	var stdout, stderr bytes.Buffer
	cmd.Stdout = &capBuffer{buf: &stdout, max: subagentMaxStdout}
	cmd.Stderr = &capBuffer{buf: &stderr, max: subagentMaxStderr}

	start := time.Now()
	runErr := cmd.Run()
	duration := time.Since(start)

	res := subagentResult{
		Agent:       a.Agent,
		Command:     binPath + " " + strings.Join(cmd.Args[1:], " "),
		Stdout:      stdout.String(),
		Stderr:      stderr.String(),
		DurationMs:  duration.Milliseconds(),
		StdoutTrunc: stdout.Len() >= subagentMaxStdout,
		StderrTrunc: stderr.Len() >= subagentMaxStderr,
	}
	if cctx.Err() == context.DeadlineExceeded {
		res.ExitCode = 124 // coreutils `timeout` convention
		res.Stderr += "\n[companion] subagent timeout"
	} else if runErr != nil {
		if ee, ok := runErr.(*exec.ExitError); ok {
			res.ExitCode = ee.ExitCode()
		} else {
			return nil, fmt.Errorf("subagent exec: %w", runErr)
		}
	}

	// If cwd is inside a git repo, attach a diff stat so the synth can
	// relay "what changed" without shipping the full patch over WS.
	res.GitDiffStat = collectGitDiffStat(a.Cwd)

	return res, nil
}

func collectGitDiffStat(cwd string) string {
	git, err := exec.LookPath("git")
	if err != nil {
		return ""
	}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, git, "diff", "--stat", "HEAD")
	cmd.Dir = cwd
	out, err := cmd.Output()
	if err != nil {
		// Not a git repo, or nothing committed yet — quiet fallback.
		return ""
	}
	return strings.TrimSpace(string(out))
}

// clampDuration converts a caller-specified timeout in ms to a time.Duration
// clamped to [min ÷ 2 … max], with def as the fallback on zero/negative.
func clampDuration(ms int, def, max time.Duration) time.Duration {
	if ms <= 0 {
		return def
	}
	d := time.Duration(ms) * time.Millisecond
	if d < def/2 {
		return def / 2
	}
	if d > max {
		return max
	}
	return d
}
