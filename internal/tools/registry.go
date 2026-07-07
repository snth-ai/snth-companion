// Package tools is the dispatcher for RPC calls from the paired synth.
//
// Each tool is a Handler keyed by name. The daemon/ws_client decodes an
// incoming frame, looks up the handler, invokes it with the decoded args
// and a context (honoring synth-side timeouts + local approval cancels),
// and returns the Result which becomes the response frame.
//
// Tools must never panic on bad input — they return (nil, error) instead.
// The ws_client wraps the error into the response envelope.
//
// # Central approval gate (P0.1)
//
// Dispatch is the SINGLE enforcement point for approval. A handler CANNOT
// execute a `prompt`/`always-prompt` tool without an approval decision —
// enforcement no longer relies on each handler remembering to call
// approval.Request (a forgotten call used to be a silent bypass; see the
// yt_dlp / contacts holes). The gate keys off the descriptor's
// DangerLevel, with two optional per-tool hooks:
//
//   - ApprovalSummary(args) → (summary, path): renders the rich dialog
//     text (bash → the command, fs_write → the resolved path,
//     messages_send → target + preview). Optional; a generic summary is
//     used when absent.
//   - GatePolicy(args) → GateDecision: lets a tool compute the effective
//     gate from its args when it is not a fixed function of DangerLevel
//     (bash/fs auto-approve INSIDE the sandbox and escalate to
//     always-prompt outside; browser gates per-action). The gate still
//     runs in Dispatch — the handler never calls approval itself.
//
// The approval function is injected once at boot (SetApprovalFn) so this
// package does not import internal/approval (import-cycle avoidance). If
// it is never set the gate is DENY-CLOSED: a mis-wired build fails safe,
// not open.
package tools

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"sync"
)

// Handler is the signature every tool implements. args is the raw JSON from
// the RPC frame; handlers unmarshal into their own struct. The returned
// interface{} is marshaled back to the synth as the "data" field.
type Handler func(ctx context.Context, args json.RawMessage) (any, error)

// GateDecision is what a GatePolicy returns: how the central Dispatch gate
// should treat a specific invocation.
type GateDecision int

const (
	// GateSkip runs the handler with no approval prompt (e.g. an fs op
	// inside the sandbox, a read-only browser action). The tool's own
	// finer-grained logic decided this call is safe.
	GateSkip GateDecision = iota
	// GatePrompt requires an approval decision at "prompt" danger.
	GatePrompt
	// GateAlwaysPrompt requires an approval decision at "always-prompt"
	// danger (never auto-approved by master trust).
	GateAlwaysPrompt
)

// Descriptor is what we advertise to the synth on registration so the
// synth can expose matching `remote_*` tools in its catalog. ArgsSchema
// is informational — the synth already knows its own argument shape.
type Descriptor struct {
	Name        string `json:"name"`
	Description string `json:"description"`
	DangerLevel string `json:"danger_level"` // "safe" | "prompt" | "always-prompt"

	// ApprovalSummary, when set, renders the rich dialog text for this
	// tool from its raw args. It returns the human summary and, for
	// path-scoped tools (fs_read/fs_write), the resolved path the trust
	// store checks against AllowedWriteRoots. Best-effort: it must never
	// panic on bad JSON — return ("", "") and the gate falls back to a
	// generic summary. Not serialized (json:"-").
	ApprovalSummary func(args json.RawMessage) (summary string, path string) `json:"-"`

	// GatePolicy, when set, computes the effective gate decision for a
	// specific invocation. Used by tools whose approval requirement is a
	// function of their ARGS, not just their static DangerLevel:
	//   - bash / fs: auto-approve inside the sandbox (GateSkip), escalate
	//     to GateAlwaysPrompt outside.
	//   - browser: read-only actions GateSkip, mutating actions gate.
	// When absent, the gate is derived from DangerLevel alone. Must never
	// panic on bad JSON. Not serialized (json:"-").
	GatePolicy func(args json.RawMessage) GateDecision `json:"-"`
}

// ApprovalFn is the injected approval primitive. It mirrors
// approval.Request but is decoupled to avoid an import cycle. Returns
// (allowed, error); a plain user-deny is (false, nil).
type ApprovalFn func(ctx context.Context, tool, summary, danger, path string) (bool, error)

var (
	mu       sync.RWMutex
	handlers = map[string]Handler{}
	descs    = map[string]Descriptor{}

	approvalMu sync.RWMutex
	approvalFn ApprovalFn
)

// SetApprovalFn wires the package-global approval primitive. Called once
// at boot from cmd/companion/main. Until set, the gate denies every
// prompt/always-prompt tool (deny-closed).
func SetApprovalFn(fn ApprovalFn) {
	approvalMu.Lock()
	defer approvalMu.Unlock()
	approvalFn = fn
}

func currentApprovalFn() ApprovalFn {
	approvalMu.RLock()
	defer approvalMu.RUnlock()
	return approvalFn
}

// ErrDenied is returned by Dispatch when the approval gate does not allow
// the call (user denied, trust-denied, or deny-closed because no approval
// fn was wired). The handler is NOT invoked in this case.
var ErrDenied = errors.New("denied by user")

// Register wires a handler + descriptor under name. Idempotent — last
// registration wins (useful for tests).
func Register(desc Descriptor, h Handler) {
	mu.Lock()
	defer mu.Unlock()
	handlers[desc.Name] = h
	descs[desc.Name] = desc
}

// Dispatch invokes the handler for name or returns an error if unknown.
// Before invoking the handler it runs the central approval gate: for a
// tool whose effective gate is prompt/always-prompt, the injected
// ApprovalFn must allow the call, else Dispatch returns ErrDenied WITHOUT
// running the handler. "safe" tools (and GateSkip invocations) run
// directly.
func Dispatch(ctx context.Context, name string, args json.RawMessage) (any, error) {
	mu.RLock()
	h, ok := handlers[name]
	d, hasDesc := descs[name]
	mu.RUnlock()
	if !ok {
		return nil, fmt.Errorf("unknown tool: %q", name)
	}

	if hasDesc {
		if allowed, err := gate(ctx, d, args); err != nil {
			return nil, err
		} else if !allowed {
			return nil, ErrDenied
		}
	}
	return h(ctx, args)
}

// gate evaluates the central approval decision for one invocation.
// Returns (true, nil) when the handler may run, (false, nil) when the
// call is denied, or (_, err) on an approval-layer error.
func gate(ctx context.Context, d Descriptor, args json.RawMessage) (bool, error) {
	decision := staticGate(d.DangerLevel)
	if d.GatePolicy != nil {
		decision = d.GatePolicy(args)
	}
	if decision == GateSkip {
		return true, nil
	}

	danger := "prompt"
	if decision == GateAlwaysPrompt {
		danger = "always-prompt"
	}

	summary := fmt.Sprintf("%s wants to run %s", "The synth", d.Name)
	var path string
	if d.ApprovalSummary != nil {
		if s, p := d.ApprovalSummary(args); s != "" {
			summary = s
			path = p
		} else if p != "" {
			path = p
		}
	}

	fn := currentApprovalFn()
	if fn == nil {
		// Deny-closed: a mis-wired build must fail safe, not open.
		return false, nil
	}
	return fn(ctx, d.Name, summary, danger, path)
}

// staticGate maps a descriptor DangerLevel to the default gate decision.
func staticGate(danger string) GateDecision {
	switch danger {
	case "always-prompt":
		return GateAlwaysPrompt
	case "prompt":
		return GatePrompt
	default: // "safe" or unknown → no gate
		return GateSkip
	}
}

// Catalog returns the list of descriptors to send to the synth on connect.
func Catalog() []Descriptor {
	mu.RLock()
	defer mu.RUnlock()
	out := make([]Descriptor, 0, len(descs))
	for _, d := range descs {
		out = append(out, d)
	}
	return out
}
