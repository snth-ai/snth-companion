// Package tools is the dispatcher for RPC calls from the paired synth.
//
// Each tool is a Handler keyed by name. The daemon/ws_client decodes an
// incoming frame, looks up the handler, invokes it with the decoded args
// and a context (honoring synth-side timeouts + local approval cancels),
// and returns the Result which becomes the response frame.
//
// Tools must never panic on bad input — they return (nil, error) instead.
// The ws_client wraps the error into the response envelope.
package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"sync"
)

// Handler is the signature every tool implements. args is the raw JSON from
// the RPC frame; handlers unmarshal into their own struct. The returned
// interface{} is marshaled back to the synth as the "data" field.
type Handler func(ctx context.Context, args json.RawMessage) (any, error)

// Descriptor is what we advertise to the synth on registration so the
// synth can expose matching `remote_*` tools in its catalog. ArgsSchema
// is informational — the synth already knows its own argument shape.
type Descriptor struct {
	Name        string `json:"name"`
	Description string `json:"description"`
	DangerLevel string `json:"danger_level"` // "safe" | "prompt" | "always-prompt"
}

var (
	mu       sync.RWMutex
	handlers = map[string]Handler{}
	descs    = map[string]Descriptor{}
)

// Register wires a handler + descriptor under name. Idempotent — last
// registration wins (useful for tests).
func Register(desc Descriptor, h Handler) {
	mu.Lock()
	defer mu.Unlock()
	handlers[desc.Name] = h
	descs[desc.Name] = desc
}

// Dispatch invokes the handler for name or returns an error if unknown.
func Dispatch(ctx context.Context, name string, args json.RawMessage) (any, error) {
	mu.RLock()
	h, ok := handlers[name]
	mu.RUnlock()
	if !ok {
		return nil, fmt.Errorf("unknown tool: %q", name)
	}
	return h(ctx, args)
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
