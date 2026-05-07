package daemon

// agent_runner.go — drives one streaming turn for the synth's
// experimental companion-claude-max provider.
//
// Architecture: synth sends `agent.run_turn` over WS → we spawn `claude
// -p` as a subprocess with the requested model + system prompt + a
// pre-flattened history+user prompt. claude CLI authenticates against
// the local OAuth credentials in ~/.claude/ (Max subscription). We
// stream stdout (--output-format stream-json + --include-partial-
// messages) and forward content_block_delta events as
// `agent.text_delta` frames. On terminal `result` event, we send
// `agent.done` with usage. On synth-initiated `agent.cancel` matching
// our turn_id, we send SIGTERM to the subprocess.
//
// PHASE 1 (this file): no MCP bridge. claude CLI runs with whatever
// built-in tools it normally has access to (Bash/Read/Write/Edit/
// WebSearch/WebFetch/Glob/Grep/Skill/Agent). The synth's tool catalog
// is NOT mirrored back — synth-bridged tool execution is reserved for
// PHASE 2 and stays dormant. The synth-side disallowed_tools list is
// passed through to --disallowed-tools so the operator can still gate
// per-turn what claude is allowed to do.
//
// Auth: --bare is intentionally NOT used. Bare mode forces strict
// ANTHROPIC_API_KEY-only auth and ignores OAuth + keychain. We need
// the opposite — OAuth from ~/.claude/ tied to the user's Max sub.

import (
	"bufio"
	"context"
	"encoding/json"
	"io"
	"log"
	"os/exec"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	"github.com/gorilla/websocket"
)

// claudeCLIPath holds the path to the `claude` binary. Empty when the
// CLI isn't installed. Set once at startup by DetectClaudeCLI().
var claudeCLIPath atomic.Value // string

// claudeCLIVersion is the output of `claude --version` (best-effort
// trim). Used in startup logs only.
var claudeCLIVersion atomic.Value // string

// DetectClaudeCLI runs `claude --version` and stashes the path +
// version when successful. Called once during companion boot. Safe to
// call concurrently — uses atomic stores. Returns the resolved path
// (empty when CLI not found).
func DetectClaudeCLI() string {
	path, err := exec.LookPath("claude")
	if err != nil {
		log.Printf("[agent-runner] claude CLI not found in PATH — companion-claude-max disabled")
		return ""
	}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	out, err := exec.CommandContext(ctx, path, "--version").Output()
	if err != nil {
		log.Printf("[agent-runner] claude --version failed: %v — companion-claude-max disabled", err)
		return ""
	}
	version := strings.TrimSpace(string(out))
	claudeCLIPath.Store(path)
	claudeCLIVersion.Store(version)
	log.Printf("[agent-runner] claude CLI ready: %s (%s)", path, version)
	return path
}

// IsClaudeCLIReady returns true when DetectClaudeCLI succeeded.
// ws_client.go consults this to decide whether to advertise the
// CapAgentSdkMaxReady capability.
func IsClaudeCLIReady() bool {
	v, _ := claudeCLIPath.Load().(string)
	return v != ""
}

// agentTurnState tracks one in-flight subprocess. Used to honor
// agent.cancel frames mid-turn.
type agentTurnState struct {
	turnID string
	cmd    *exec.Cmd
	cancel context.CancelFunc
	doneCh chan struct{}
}

var (
	agentTurnsMu sync.Mutex
	agentTurns   = map[string]*agentTurnState{}
)

// HandleAgentRunTurn runs one streaming turn end-to-end and writes
// frames back through the provided write func. Blocks until the
// subprocess exits OR cancellation. Safe for the WS read loop to
// dispatch in a goroutine — we serialize WS writes through writeFrame
// (callers should provide a write that's safe to call concurrently
// with their own writes; the existing Client uses gorilla's writer
// which is single-threaded so they need a per-turn lock or a
// dedicated writer goroutine; see `runOnceWithMutex` below).
func HandleAgentRunTurn(parent context.Context, frame Frame, writeFrame func(Frame) error) {
	turnID := frame.TurnID
	if turnID == "" {
		_ = writeFrame(Frame{
			Type:        FrameAgentError,
			TurnID:      turnID,
			Error:       "agent.run_turn missing turn_id",
			Recoverable: boolPtr(false),
		})
		return
	}

	cliPath, _ := claudeCLIPath.Load().(string)
	if cliPath == "" {
		_ = writeFrame(Frame{
			Type:        FrameAgentError,
			TurnID:      turnID,
			Error:       "claude CLI not available on this companion",
			Recoverable: boolPtr(false),
		})
		return
	}

	model := frame.Model
	if model == "" {
		model = "claude-opus-4-7"
	}

	// Build the user-facing prompt: history block (boop pattern) +
	// current message. claude CLI accepts this as the positional
	// argument when --input-format=text (default).
	var userPrompt strings.Builder
	if strings.TrimSpace(frame.HistoryText) != "" {
		userPrompt.WriteString("Prior turns:\n")
		userPrompt.WriteString(frame.HistoryText)
		userPrompt.WriteString("\n\nCurrent message:\n")
	}
	userPrompt.WriteString(frame.Prompt)

	args := []string{
		"-p",
		"--print",
		"--output-format", "stream-json",
		"--input-format", "text",
		"--include-partial-messages",
		"--verbose", // stream-json requires verbose
		"--no-session-persistence",
		"--dangerously-skip-permissions",
		"--model", model,
	}
	if frame.SystemPrompt != "" {
		// REPLACE (not append) so the synth's identity is the only
		// system prompt the model sees. Claude Code's default system
		// prompt would otherwise dominate ("You are Claude Code,
		// Anthropic's official CLI…") and Mia would answer in that
		// voice instead of her own. Tool-use machinery in claude CLI
		// isn't carried by the system prompt — it stays functional.
		args = append(args, "--system-prompt", frame.SystemPrompt)
	}
	if frame.Effort != "" {
		args = append(args, "--effort", frame.Effort)
	}
	if len(frame.Disallowed) > 0 {
		args = append(args, "--disallowed-tools")
		args = append(args, frame.Disallowed...)
	}
	args = append(args, userPrompt.String())

	turnCtx, cancelTurn := context.WithCancel(parent)
	defer cancelTurn()

	cmd := exec.CommandContext(turnCtx, cliPath, args...)
	stdout, err := cmd.StdoutPipe()
	if err != nil {
		_ = writeFrame(Frame{Type: FrameAgentError, TurnID: turnID, Error: "stdout pipe: " + err.Error(), Recoverable: boolPtr(false)})
		return
	}
	stderr, err := cmd.StderrPipe()
	if err != nil {
		_ = writeFrame(Frame{Type: FrameAgentError, TurnID: turnID, Error: "stderr pipe: " + err.Error(), Recoverable: boolPtr(false)})
		return
	}

	if err := cmd.Start(); err != nil {
		_ = writeFrame(Frame{Type: FrameAgentError, TurnID: turnID, Error: "start claude: " + err.Error(), Recoverable: boolPtr(false)})
		return
	}

	state := &agentTurnState{
		turnID: turnID,
		cmd:    cmd,
		cancel: cancelTurn,
		doneCh: make(chan struct{}),
	}
	agentTurnsMu.Lock()
	agentTurns[turnID] = state
	agentTurnsMu.Unlock()
	defer func() {
		agentTurnsMu.Lock()
		delete(agentTurns, turnID)
		agentTurnsMu.Unlock()
		close(state.doneCh)
	}()

	// Drain stderr in a goroutine — log it, don't forward (Claude CLI
	// sometimes emits noise that isn't useful to the synth).
	go func() {
		buf := bufio.NewScanner(stderr)
		buf.Buffer(make([]byte, 64*1024), 1024*1024)
		for buf.Scan() {
			line := buf.Text()
			if strings.TrimSpace(line) != "" {
				log.Printf("[agent-runner %s] stderr: %s", turnID[:min(8, len(turnID))], line)
			}
		}
	}()

	parseStreamJSON(stdout, turnID, writeFrame)

	// Wait for the subprocess to exit so we can read the exit status.
	// stream-json's "result" event already gave the synth what it
	// needs; this Wait is for cleanup + logging only.
	if err := cmd.Wait(); err != nil {
		// If we already sent agent.done from the result event, this
		// non-zero exit is just lifecycle noise. If we DIDN'T send done
		// (e.g. claude crashed before producing result), surface as
		// agent.error.
		log.Printf("[agent-runner %s] claude exited: %v", turnID[:min(8, len(turnID))], err)
	}
}

// parseStreamJSON reads claude's stream-json stdout line-by-line and
// translates events into wire frames sent through writeFrame. Each
// stream-json event is a JSON object on its own line.
//
// We care about three event types:
//   - stream_event with content_block_delta type=text_delta → emit
//     agent.text_delta with the delta text
//   - assistant message (full) — used as a fallback if partial-messages
//     wasn't reliable; we accumulate the text but DON'T emit it as
//     delta to avoid double-counting (synth aggregates deltas).
//   - result (terminal) → emit agent.done with usage
//
// All other event types (system init, hook events, tool_use_summary,
// etc.) are ignored for phase 1.
func parseStreamJSON(r io.Reader, turnID string, writeFrame func(Frame) error) {
	scanner := bufio.NewScanner(r)
	// Lines from claude can be large (full assistant message blocks);
	// bump the scanner buffer.
	scanner.Buffer(make([]byte, 64*1024), 4*1024*1024)

	doneSent := false
	var aggregatedText strings.Builder
	var servedModel string // captured from claude.ai's actual served-model field

	for scanner.Scan() {
		raw := scanner.Bytes()
		if len(raw) == 0 {
			continue
		}
		var ev streamEvent
		if err := json.Unmarshal(raw, &ev); err != nil {
			log.Printf("[agent-runner %s] parse: %v (line=%q)", turnID[:min(8, len(turnID))], err, truncate(string(raw), 200))
			continue
		}

		// Capture the served-model id whenever claude.ai exposes it.
		// This is the GROUND TRUTH for "what model actually answered" —
		// model field in init is what we asked for, but the model
		// inside assistant.message.model is what claude.ai actually
		// routed to (would differ if our requested model was unavailable
		// and the SDK fell back).
		if ev.Type == "assistant" && ev.Message != nil && ev.Message.Model != "" {
			servedModel = ev.Message.Model
		}

		switch ev.Type {
		case "stream_event":
			// Token-level streaming. Inside .event we have shape like:
			//   {type:"content_block_delta", delta:{type:"text_delta", text:"Hello"}}
			if ev.Event == nil {
				continue
			}
			if ev.Event.Type != "content_block_delta" || ev.Event.Delta == nil {
				continue
			}
			if ev.Event.Delta.Type != "text_delta" || ev.Event.Delta.Text == "" {
				continue
			}
			aggregatedText.WriteString(ev.Event.Delta.Text)
			if err := writeFrame(Frame{
				Type:   FrameAgentTextDelta,
				TurnID: turnID,
				Delta:  ev.Event.Delta.Text,
			}); err != nil {
				log.Printf("[agent-runner %s] write text_delta: %v", turnID[:min(8, len(turnID))], err)
				return
			}

		case "result":
			// Terminal. Synth aggregates deltas itself but we also send
			// the final text + usage so it can reconcile + bill.
			usage := &AgentTurnUsage{}
			if ev.Usage != nil {
				usage.InputTokens = ev.Usage.InputTokens
				usage.OutputTokens = ev.Usage.OutputTokens
				usage.CacheReadTokens = ev.Usage.CacheReadInputTokens
				usage.CacheWriteTokens = ev.Usage.CacheCreationInputTokens
			}
			finalText := ev.Result
			if finalText == "" {
				finalText = aggregatedText.String()
			}
			if err := writeFrame(Frame{
				Type:      FrameAgentDone,
				TurnID:    turnID,
				FinalText: finalText,
				Usage:     usage,
			}); err != nil {
				log.Printf("[agent-runner %s] write done: %v", turnID[:min(8, len(turnID))], err)
			}
			// Definitive proof line. servedModel comes from claude.ai's
			// own response (assistant.message.model), so it's not just
			// "what we asked for" but "what actually answered".
			log.Printf("[agent-runner %s] DONE served_by=%s tokens=%d/%d cache_r/w=%d/%d cost_usd=%.4f (sub=Max, no $ charge)",
				turnID[:min(8, len(turnID))], servedModel,
				usage.InputTokens, usage.OutputTokens,
				usage.CacheReadTokens, usage.CacheWriteTokens,
				ev.TotalCostUSD)
			doneSent = true

		case "system":
			// Only log subtype=init (has model field). Other system
			// subtypes like "status" don't carry useful info for our
			// per-turn tracing.
			if ev.Subtype == "init" {
				log.Printf("[agent-runner %s] start model=%s session=%s", turnID[:min(8, len(turnID))], ev.Model, ev.SessionID)
			}

		default:
			// Other types we explicitly ignore for phase 1: assistant
			// (full block — we already streamed deltas), user (tool
			// results we didn't bridge), tool_use_summary, hook_*, etc.
		}
	}

	if err := scanner.Err(); err != nil {
		log.Printf("[agent-runner %s] scan: %v", turnID[:min(8, len(turnID))], err)
	}

	// If subprocess exited before emitting a result event, send a
	// best-effort done with whatever text we aggregated. Synth side
	// won't crash on missing usage — Usage pointer will be empty.
	if !doneSent {
		_ = writeFrame(Frame{
			Type:      FrameAgentDone,
			TurnID:    turnID,
			FinalText: aggregatedText.String(),
			Usage:     &AgentTurnUsage{},
		})
	}
}

// CancelAgentTurn signals the subprocess for the named turn to abort.
// Called by the WS read loop on incoming agent.cancel frames.
// Idempotent and no-op when no such turn exists.
func CancelAgentTurn(turnID string) {
	agentTurnsMu.Lock()
	state, ok := agentTurns[turnID]
	agentTurnsMu.Unlock()
	if !ok {
		return
	}
	state.cancel()
}

// streamEvent is the union of all stream-json event shapes. We only
// declare the fields we actually inspect; json.Unmarshal happily
// ignores the rest.
type streamEvent struct {
	Type    string             `json:"type"`
	Subtype string             `json:"subtype,omitempty"`   // when type=system: "init" | "status" | etc.
	Event   *streamSubEvent    `json:"event,omitempty"`     // when type=stream_event
	Result  string             `json:"result,omitempty"`    // when type=result
	Usage   *streamUsage       `json:"usage,omitempty"`     // when type=result
	// Total cost reported by claude CLI in the result event. Under
	// Max subscription this is INFORMATIONAL (no actual charge) — we
	// log it for visibility but the synth's usage tracker treats
	// companion-claude-max as $0 since the user already paid via
	// the subscription, not per-token.
	TotalCostUSD float64           `json:"total_cost_usd,omitempty"`
	Model        string            `json:"model,omitempty"`     // when type=system,subtype=init
	SessionID    string            `json:"session_id,omitempty"`
	// Message present on type=assistant. message.model is the
	// authoritative "what claude.ai actually used" — different from
	// the requested model only on fallbacks/availability issues.
	Message *streamAssistantMessage `json:"message,omitempty"`
}

type streamAssistantMessage struct {
	Model string `json:"model,omitempty"`
}

type streamSubEvent struct {
	Type  string             `json:"type"` // "content_block_delta" etc.
	Delta *streamSubEventDelta `json:"delta,omitempty"`
}

type streamSubEventDelta struct {
	Type string `json:"type"` // "text_delta"
	Text string `json:"text,omitempty"`
}

type streamUsage struct {
	InputTokens              int `json:"input_tokens"`
	OutputTokens             int `json:"output_tokens"`
	CacheReadInputTokens     int `json:"cache_read_input_tokens"`
	CacheCreationInputTokens int `json:"cache_creation_input_tokens"`
}

func boolPtr(b bool) *bool { return &b }

// safeWriteFrame returns a writeFrame func that serializes WS writes
// through the given mutex. Used by the WS client to feed the read
// loop's HandleAgentRunTurn call without racing the ping loop.
func safeWriteFrame(conn *websocket.Conn, mu *sync.Mutex) func(Frame) error {
	return func(f Frame) error {
		mu.Lock()
		defer mu.Unlock()
		return conn.WriteJSON(f)
	}
}
