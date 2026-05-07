// Package daemon contains the long-running pieces of the companion: the
// local HTTP UI server and the persistent WebSocket client to the paired
// synth. protocol.go defines the wire shapes shared with openpaw_server's
// companion_ws.go — keep them in lockstep.
package daemon

import "encoding/json"

// Every frame on the wire is one of these four types. "type" is always
// present; other fields depend on the type.
const (
	FrameHello      = "hello"       // client → server on connect
	FrameWelcome    = "welcome"     // server → client after auth ok
	FrameToolCall   = "tool_call"   // server → client: invoke a tool
	FrameToolResult = "tool_result" // client → server: result of a tool_call
	FramePing       = "ping"        // either direction, application-level keepalive
	FramePong       = "pong"        // response to ping
	FrameError      = "error"       // either direction, fatal

	// Streaming-turn frame types — companion-claude-max experimental
	// provider on the synth side delegates one full Agent SDK / claude CLI
	// turn to the companion. Coexist with the legacy RPC types above;
	// routing decides which path a frame takes by presence of TurnID.
	// Companions that haven't shipped the agent runner simply never emit
	// or receive these frames.
	FrameAgentRunTurn      = "agent.run_turn"       // server → client: start a turn
	FrameAgentTextDelta    = "agent.text_delta"     // client → server: streaming output
	FrameAgentToolUseEvent = "agent.tool_use_event" // client → server: informational logging
	FrameAgentDone         = "agent.done"           // client → server: terminal success
	FrameAgentError        = "agent.error"          // client → server: terminal failure
	FrameAgentCancel       = "agent.cancel"         // server → client: abort current turn

	// CapAgentSdkMaxReady is advertised in the hello-frame Capabilities
	// list when this companion has `claude` CLI installed + authed (a
	// Max subscription's OAuth in ~/.claude/) and the agent runner is
	// enabled. Synth-side companion_ws.go filters the pool by this
	// before opening a streaming turn.
	CapAgentSdkMaxReady = "agent_sdk_max_ready"
)

// Frame is the envelope for every message. Unused fields are omitted.
type Frame struct {
	Type string `json:"type"`

	// Hello (client → server)
	CompanionVersion string     `json:"companion_version,omitempty"`
	Capabilities     []ToolDesc `json:"capabilities,omitempty"`

	// Multi-companion identity fields (Phase 1, 2026-04-27).
	// Synth-side companion_ws.go derives the pool slot from these so
	// reconnects from the same Mac collapse onto the same slot while
	// different Macs (Air + Mini) coexist.
	CompanionRole     string   `json:"companion_role,omitempty"`      // "synth-host" | "user-device" | "shared"
	CompanionTags     []string `json:"companion_tags,omitempty"`      // freeform user labels
	CompanionDeviceID string   `json:"companion_device_id,omitempty"` // hostname / stable id

	// Welcome (server → client)
	SynthVersion string `json:"synth_version,omitempty"`
	SynthID      string `json:"synth_id,omitempty"`

	// Tool call (server → client) / result (client → server)
	CallID string          `json:"call_id,omitempty"`
	Tool   string          `json:"tool,omitempty"`
	Args   json.RawMessage `json:"args,omitempty"`
	Data   json.RawMessage `json:"data,omitempty"`
	Error  string          `json:"error,omitempty"`

	// Streaming-turn fields (companion-claude-max experimental). All
	// optional — legacy frames leave them empty. When TurnID is set,
	// the frame belongs to one streaming turn opened by the synth via
	// FrameAgentRunTurn.
	TurnID       string          `json:"turn_id,omitempty"`
	Model        string          `json:"model,omitempty"`         // for agent.run_turn
	Effort       string          `json:"effort,omitempty"`        // "low"|"medium"|"high"|"max"
	SystemPrompt string          `json:"system_prompt,omitempty"`
	Prompt       string          `json:"prompt,omitempty"`
	HistoryText  string          `json:"history_text,omitempty"`  // pre-flattened conversation history
	ToolCatalog  json.RawMessage `json:"tool_catalog,omitempty"`  // JSON Schemas, reserved for phase-2 MCP bridge
	MaxTurns     int             `json:"max_turns,omitempty"`
	Disallowed   []string        `json:"disallowed_tools,omitempty"`
	Delta        string          `json:"delta,omitempty"`         // for agent.text_delta
	FinalText    string          `json:"final_text,omitempty"`    // for agent.done
	Usage        *AgentTurnUsage `json:"usage,omitempty"`         // for agent.done
	Recoverable  *bool           `json:"recoverable,omitempty"`   // for agent.error (pointer to distinguish unset)
	InputPreview string          `json:"input_preview,omitempty"` // for agent.tool_use_event
}

// AgentTurnUsage carries token accounting for one streaming turn.
// Mirrors snth-side llm.AgentTurnUsage exactly.
type AgentTurnUsage struct {
	InputTokens      int `json:"input_tokens"`
	OutputTokens     int `json:"output_tokens"`
	CacheReadTokens  int `json:"cache_read_tokens"`
	CacheWriteTokens int `json:"cache_write_tokens"`
}

// ToolDesc is the wire version of tools.Descriptor. Kept separate so the
// tools package doesn't need to import the protocol package.
type ToolDesc struct {
	Name        string `json:"name"`
	Description string `json:"description"`
	DangerLevel string `json:"danger_level"`
}
