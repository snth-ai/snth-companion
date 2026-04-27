// Package trust holds the user-controlled approval policy that sits in
// front of approval.Request. It answers: "for this tool right now, do
// I auto-approve, auto-deny, or ask the user?"
//
// State is JSON-persisted at
// `~/Library/Application Support/SNTH Companion/trust.json`. The schema
// is small on purpose — per-tool tri-state, a master switch, an
// optional global expiry, and a list of write-roots that fs_write
// honors even when its per-tool state is `trusted`.
//
// Critical tools (subagent, messages_send) are listed in
// AlwaysPromptTools and are NEVER auto-approved by master mode. They
// still respect a per-tool override when the user explicitly opts in,
// because the user is the source of truth — the always-prompt list is
// just a default safety net for "trust everything" master toggle.
package trust

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"
)

// Decision is what Get returns. Callers translate Prompt into the real
// osascript prompt; the other two are decisive.
type Decision string

const (
	DecisionTrusted Decision = "trusted"
	DecisionDenied  Decision = "denied"
	DecisionPrompt  Decision = "prompt"
)

// ToolMode is the per-tool override.
//
//   - "prompt"  — show the dialog (default)
//   - "trusted" — auto-approve, no dialog
//   - "denied"  — auto-deny, no dialog
type ToolMode string

const (
	ModePrompt  ToolMode = "prompt"
	ModeTrusted ToolMode = "trusted"
	ModeDenied  ToolMode = "denied"
)

// AlwaysPromptTools — never auto-approved by the master toggle. Listed
// here because spawning a 60-min coding sub-agent or sending a message
// from the user's phone is the kind of action you DO want a confirm
// click for, even if you're "fully trusted" mode for everything else.
//
// Per-tool override still works: if the user explicitly flips one of
// these to ModeTrusted, that's their call.
var AlwaysPromptTools = map[string]bool{
	"subagent":      true, // 60-min CLI delegation, costs $$$
	"messages_send": true, // sends from the user's iMessage
}

// State is the on-disk schema. Backwards-compat: missing fields are
// safe defaults (prompt for everything).
type State struct {
	// Master, when true, auto-approves any tool not listed in the
	// always-prompt set and not explicitly denied per-tool.
	Master bool `json:"master"`

	// Tools — per-tool override. nil/missing key = "prompt" default.
	Tools map[string]ToolMode `json:"tools,omitempty"`

	// MasterExpires — when set, master auto-flips to false at that
	// instant. Get() also returns Prompt when expired.
	MasterExpires *time.Time `json:"master_expires,omitempty"`

	// WritePolicy bounds where fs_write can land in trusted mode.
	// Empty list = no path restriction (trust applies fully).
	// Non-empty = even with master/per-tool=trusted, fs_write is
	// auto-approved ONLY when the destination is under one of these
	// (Tilde-expanded, Clean'd) roots.
	AllowedWriteRoots []string `json:"allowed_write_roots,omitempty"`

	// UpdatedAt — informational, populated on every Save.
	UpdatedAt time.Time `json:"updated_at"`
}

// --- store ---

type Store struct {
	mu   sync.RWMutex
	path string
	st   State
}

// NewStore loads or initializes state at supportDir/trust.json.
// supportDir is "" → defaults to ~/Library/Application Support/SNTH Companion.
// On first boot the file is missing → in-memory zero State (everything
// prompt-mode), file written on first Save().
func NewStore(supportDir string) (*Store, error) {
	if supportDir == "" {
		home, err := os.UserHomeDir()
		if err != nil {
			return nil, fmt.Errorf("homedir: %w", err)
		}
		supportDir = filepath.Join(home, "Library", "Application Support", "SNTH Companion")
	}
	if err := os.MkdirAll(supportDir, 0o700); err != nil {
		return nil, fmt.Errorf("mkdir support: %w", err)
	}
	s := &Store{path: filepath.Join(supportDir, "trust.json")}
	if err := s.load(); err != nil {
		return nil, err
	}
	return s, nil
}

func (s *Store) load() error {
	b, err := os.ReadFile(s.path)
	if err != nil {
		if os.IsNotExist(err) {
			s.st = State{}
			return nil
		}
		return fmt.Errorf("read %s: %w", s.path, err)
	}
	if err := json.Unmarshal(b, &s.st); err != nil {
		return fmt.Errorf("parse %s: %w", s.path, err)
	}
	return nil
}

// Path returns the JSON file location (informational).
func (s *Store) Path() string { return s.path }

// Snapshot returns a copy of the current state — safe for read-only
// inspection / serializing to the SPA. The map is shallow-copied; do
// not mutate the returned slice / map fields.
func (s *Store) Snapshot() State {
	s.mu.RLock()
	defer s.mu.RUnlock()
	out := s.st
	if s.st.Tools != nil {
		out.Tools = make(map[string]ToolMode, len(s.st.Tools))
		for k, v := range s.st.Tools {
			out.Tools[k] = v
		}
	}
	if s.st.AllowedWriteRoots != nil {
		out.AllowedWriteRoots = append([]string(nil), s.st.AllowedWriteRoots...)
	}
	return out
}

// Save replaces state atomically (temp + rename) and updates UpdatedAt.
func (s *Store) Save(next State) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	next.UpdatedAt = time.Now().UTC()
	b, err := json.MarshalIndent(next, "", "  ")
	if err != nil {
		return fmt.Errorf("marshal: %w", err)
	}
	tmp := s.path + ".tmp"
	if err := os.WriteFile(tmp, b, 0o600); err != nil {
		return fmt.Errorf("write tmp: %w", err)
	}
	if err := os.Rename(tmp, s.path); err != nil {
		return fmt.Errorf("rename: %w", err)
	}
	s.st = next
	return nil
}

// SetTool flips one tool's mode without touching the rest of state.
func (s *Store) SetTool(name string, mode ToolMode) error {
	s.mu.Lock()
	st := s.st
	s.mu.Unlock()
	if st.Tools == nil {
		st.Tools = map[string]ToolMode{}
	}
	if mode == ModePrompt {
		delete(st.Tools, name)
	} else {
		st.Tools[name] = mode
	}
	return s.Save(st)
}

// SetMaster sets the master toggle. expires==nil → no expiry; otherwise
// auto-prompts again after the deadline.
func (s *Store) SetMaster(on bool, expires *time.Time) error {
	st := s.Snapshot()
	st.Master = on
	st.MasterExpires = expires
	return s.Save(st)
}

// RevokeAll resets the entire state to defaults (everything prompts
// again). Logs the timestamp of the kill switch.
func (s *Store) RevokeAll() error {
	return s.Save(State{})
}

// AddWriteRoot appends a root path (de-duplicated, Clean'd).
func (s *Store) AddWriteRoot(root string) error {
	st := s.Snapshot()
	root = filepath.Clean(expandHome(root))
	for _, r := range st.AllowedWriteRoots {
		if r == root {
			return nil
		}
	}
	st.AllowedWriteRoots = append(st.AllowedWriteRoots, root)
	return s.Save(st)
}

// RemoveWriteRoot removes a root path.
func (s *Store) RemoveWriteRoot(root string) error {
	st := s.Snapshot()
	root = filepath.Clean(expandHome(root))
	out := st.AllowedWriteRoots[:0]
	for _, r := range st.AllowedWriteRoots {
		if r != root {
			out = append(out, r)
		}
	}
	st.AllowedWriteRoots = out
	return s.Save(st)
}

// --- decision path ---

// Get evaluates trust state for `tool` and returns one of the three
// decisions. `path`, when non-empty, gates fs_write style tools by
// checking AllowedWriteRoots — outside the roots, even master/per-tool
// trusted falls back to Prompt (the user explicitly didn't opt in for
// that location).
func (s *Store) Get(tool, path string) Decision {
	s.mu.RLock()
	st := s.st
	s.mu.RUnlock()

	// Explicit deny wins over everything (user's panic-deny per tool).
	if mode, ok := st.Tools[tool]; ok {
		switch mode {
		case ModeDenied:
			return DecisionDenied
		case ModeTrusted:
			if !inAllowedRoot(path, st.AllowedWriteRoots) {
				return DecisionPrompt
			}
			return DecisionTrusted
		}
	}

	// Master only fires when not expired and tool isn't in the
	// always-prompt safety set.
	if !st.Master {
		return DecisionPrompt
	}
	if st.MasterExpires != nil && !time.Now().Before(*st.MasterExpires) {
		return DecisionPrompt
	}
	if AlwaysPromptTools[tool] {
		return DecisionPrompt
	}
	if !inAllowedRoot(path, st.AllowedWriteRoots) {
		return DecisionPrompt
	}
	return DecisionTrusted
}

// inAllowedRoot returns true when:
//   - path is "" (caller didn't pass a path; tool isn't path-scoped), OR
//   - allowed list is empty (no path restriction set), OR
//   - the path lies under at least one of the roots.
func inAllowedRoot(path string, allowed []string) bool {
	if path == "" || len(allowed) == 0 {
		return true
	}
	clean := filepath.Clean(expandHome(path))
	for _, r := range allowed {
		if clean == r || strings.HasPrefix(clean, r+string(filepath.Separator)) {
			return true
		}
	}
	return false
}

func expandHome(p string) string {
	if p == "" || !strings.HasPrefix(p, "~") {
		return p
	}
	home, err := os.UserHomeDir()
	if err != nil {
		return p
	}
	if p == "~" {
		return home
	}
	if strings.HasPrefix(p, "~/") {
		return filepath.Join(home, p[2:])
	}
	return p
}
