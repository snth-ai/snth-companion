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
}

// ToolDesc is the wire version of tools.Descriptor. Kept separate so the
// tools package doesn't need to import the protocol package.
type ToolDesc struct {
	Name        string `json:"name"`
	Description string `json:"description"`
	DangerLevel string `json:"danger_level"`
}
