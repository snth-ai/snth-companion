// Package config persists the companion's local state in a JSON file under
// ~/Library/Application Support/snth-companion/config.json on macOS (or
// $XDG_CONFIG_HOME/snth-companion on Linux, for when we add that target).
//
// The on-disk state is the single source of truth for what synths this
// companion is paired to, what local paths the user has granted access to,
// and what command patterns are auto-approved. Everything else (active WS
// connection, in-flight RPCs, UI state) is in-memory only.
//
// Multi-synth pairing (Phase 1, role-based, 2026-04-27): a companion now
// holds N pairs in `Synths` and a single `ActiveSynthID`. Legacy scalar
// fields (PairedSynth*) are auto-populated from the active pair for
// backward compatibility while callers migrate.
package config

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"sync"
	"time"
)

// CompanionRole is one of the fixed roles a companion plays for a given
// owner. The role is per-COMPANION (this Mac), not per-pair — it's the
// machine's identity, not the relationship.
//
//	synth-host    — the synth lives here (its primary working machine)
//	user-device   — the human owner's daily device, synth is a guest
//	shared        — multi-tenant or office machine, restrictive defaults
type CompanionRole string

const (
	RoleSynthHost  CompanionRole = "synth-host"
	RoleUserDevice CompanionRole = "user-device"
	RoleShared     CompanionRole = "shared"
)

// SynthRole is one of the fixed roles a paired synth plays for THIS
// companion. The role is per-PAIR — same synth could be primary on the
// home Mac and test on the work laptop.
//
//	primary    — main synth this companion serves
//	secondary  — companion synth, supports primary's work
//	test       — work-in-progress / experimental, restrictive defaults
type SynthRole string

const (
	SynthRolePrimary   SynthRole = "primary"
	SynthRoleSecondary SynthRole = "secondary"
	SynthRoleTest      SynthRole = "test"
)

// SynthPair is one paired synth — its connection coordinates, role, and
// freeform tags. Stored in Config.Synths.
type SynthPair struct {
	ID         string    `json:"id"`             // synth instance_id, also display key
	URL        string    `json:"url"`            // wss://<synth>/api/companion/ws base
	Token      string    `json:"token"`          // bearer
	HubURL     string    `json:"hub_url,omitempty"`
	Label      string    `json:"label,omitempty"` // human-friendly name override
	Role       SynthRole `json:"role"`
	Tags       []string  `json:"tags,omitempty"` // freeform user-defined
	CreatedAt  time.Time `json:"created_at"`
	LastSeenAt time.Time `json:"last_seen_at,omitempty"`
}

// Config is the persisted state. All fields are JSON-serializable so future
// versions can migrate by adding fields (zero values stay reasonable).
type Config struct {
	// CompanionRole — this Mac's role across all paired synths. Defaults
	// to user-device on first boot. Owner can flip via Privacy UI.
	CompanionRole CompanionRole `json:"companion_role,omitempty"`

	// CompanionTags — freeform user-supplied labels for THIS companion.
	// Surfaced in synth's /api/companion/status so synth-side rules can
	// react ("don't run subagents on tag=battery-only", etc.).
	CompanionTags []string `json:"companion_tags,omitempty"`

	// Synths — every pair this companion has set up. Order is creation
	// order; ActiveSynthID picks which one the runtime currently serves.
	Synths []SynthPair `json:"synths,omitempty"`

	// ActiveSynthID — id of the SynthPair the WS client is currently
	// connected to. Empty when no pairs exist or none picked yet.
	// Switching active synth disconnects the current WS and connects
	// to the new pair's URL.
	ActiveSynthID string `json:"active_synth_id,omitempty"`

	// --- Legacy single-pair fields (deprecated as of Phase 1) ---
	//
	// Auto-populated from the active SynthPair on Load() / SetActive()
	// so existing callers (sandbox.DefaultSandboxRoot, ws_client, etc.)
	// keep working without an immediate refactor. New callers should
	// read Synths/ActiveSynthID directly via ActivePair().

	PairedSynthURL string `json:"paired_synth_url,omitempty"`
	PairedSynthID  string `json:"paired_synth_id,omitempty"`
	CompanionToken string `json:"companion_token,omitempty"`
	HubURL         string `json:"hub_url,omitempty"`

	// SandboxRoots are the absolute paths the companion will allow tools to
	// touch without an approval prompt. The default root (~/SNTH/<slug>/)
	// is injected by EnsureDefaults and always present.
	SandboxRoots []string `json:"sandbox_roots"`

	// AutoApproveBashPatterns — prefix matches that skip the approval prompt.
	AutoApproveBashPatterns []string `json:"auto_approve_bash_patterns"`

	// LogRetentionDays is how long to keep the local audit log.
	LogRetentionDays int `json:"log_retention_days"`
}

var (
	mu      sync.RWMutex
	current *Config
)

// Path returns the on-disk location of the config file.
func Path() string {
	return filepath.Join(dir(), "config.json")
}

func dir() string {
	if runtime.GOOS == "darwin" {
		home, err := os.UserHomeDir()
		if err == nil {
			return filepath.Join(home, "Library", "Application Support", "snth-companion")
		}
	}
	if p := os.Getenv("XDG_CONFIG_HOME"); p != "" {
		return filepath.Join(p, "snth-companion")
	}
	home, _ := os.UserHomeDir()
	return filepath.Join(home, ".config", "snth-companion")
}

// Load reads config.json if present, otherwise returns a fresh Config with
// defaults applied. The result is cached; subsequent calls to Get return the
// same pointer.
func Load() (*Config, error) {
	mu.Lock()
	defer mu.Unlock()
	if current != nil {
		return current, nil
	}
	if err := os.MkdirAll(dir(), 0o700); err != nil {
		return nil, fmt.Errorf("config dir: %w", err)
	}
	raw, err := os.ReadFile(Path())
	if err != nil && !os.IsNotExist(err) {
		return nil, fmt.Errorf("read config: %w", err)
	}
	cfg := &Config{}
	if len(raw) > 0 {
		if err := json.Unmarshal(raw, cfg); err != nil {
			return nil, fmt.Errorf("parse config: %w", err)
		}
	}
	cfg.migrateFromLegacy()
	cfg.ensureDefaults()
	cfg.syncLegacyFromActive()
	current = cfg
	return cfg, nil
}

// Get returns the cached config. Load must have been called first.
func Get() *Config {
	mu.RLock()
	defer mu.RUnlock()
	return current
}

// Save writes the current config to disk atomically (temp + rename).
func Save() error {
	mu.RLock()
	cfg := current
	mu.RUnlock()
	if cfg == nil {
		return fmt.Errorf("config not loaded")
	}
	cfg.syncLegacyFromActive()
	raw, err := json.MarshalIndent(cfg, "", "  ")
	if err != nil {
		return err
	}
	if err := os.MkdirAll(dir(), 0o700); err != nil {
		return err
	}
	tmp := Path() + ".tmp"
	if err := os.WriteFile(tmp, raw, 0o600); err != nil {
		return err
	}
	return os.Rename(tmp, Path())
}

// Update applies a mutation under the lock and persists.
func Update(fn func(*Config)) error {
	mu.Lock()
	fn(current)
	mu.Unlock()
	return Save()
}

// ActivePair returns the SynthPair currently selected as active, or nil
// when no pairs are configured.
func (c *Config) ActivePair() *SynthPair {
	if c == nil {
		return nil
	}
	if c.ActiveSynthID == "" {
		return nil
	}
	for i := range c.Synths {
		if c.Synths[i].ID == c.ActiveSynthID {
			return &c.Synths[i]
		}
	}
	return nil
}

// SetActive switches the active synth pair by id and re-syncs legacy
// scalar fields. Caller must Save() afterwards.
func (c *Config) SetActive(id string) error {
	for i := range c.Synths {
		if c.Synths[i].ID == id {
			c.ActiveSynthID = id
			c.syncLegacyFromActive()
			return nil
		}
	}
	return fmt.Errorf("synth %q not in paired list", id)
}

// AddOrUpdatePair registers a new pair (or updates existing by id). Sets
// ActiveSynthID to this pair if there was no active one before. Caller
// must Save() afterwards.
func (c *Config) AddOrUpdatePair(p SynthPair) {
	if p.Role == "" {
		p.Role = SynthRolePrimary
	}
	if p.CreatedAt.IsZero() {
		p.CreatedAt = time.Now().UTC()
	}
	for i := range c.Synths {
		if c.Synths[i].ID == p.ID {
			// preserve created_at on update
			p.CreatedAt = c.Synths[i].CreatedAt
			c.Synths[i] = p
			if c.ActiveSynthID == "" {
				c.ActiveSynthID = p.ID
				c.syncLegacyFromActive()
			}
			return
		}
	}
	c.Synths = append(c.Synths, p)
	if c.ActiveSynthID == "" {
		c.ActiveSynthID = p.ID
		c.syncLegacyFromActive()
	}
}

// RemovePair removes by id. If the removed one was active, ActiveSynthID
// flips to the first remaining pair (or "" if none). Caller must Save().
func (c *Config) RemovePair(id string) {
	out := c.Synths[:0]
	for _, p := range c.Synths {
		if p.ID != id {
			out = append(out, p)
		}
	}
	c.Synths = out
	if c.ActiveSynthID == id {
		c.ActiveSynthID = ""
		if len(c.Synths) > 0 {
			c.ActiveSynthID = c.Synths[0].ID
		}
		c.syncLegacyFromActive()
	}
}

// migrateFromLegacy promotes pre-multi-pair scalar fields into Synths
// when the slice is empty. Idempotent — running on already-migrated
// config is a no-op.
func (c *Config) migrateFromLegacy() {
	if len(c.Synths) > 0 {
		return
	}
	if c.PairedSynthURL == "" || c.PairedSynthID == "" || c.CompanionToken == "" {
		return
	}
	c.Synths = []SynthPair{{
		ID:        c.PairedSynthID,
		URL:       c.PairedSynthURL,
		Token:     c.CompanionToken,
		HubURL:    c.HubURL,
		Role:      SynthRolePrimary,
		CreatedAt: time.Now().UTC(),
	}}
	c.ActiveSynthID = c.PairedSynthID
}

// syncLegacyFromActive copies the active pair's fields into the legacy
// scalars so existing callers (DefaultSandboxRoot, ws client, hub keys
// page) keep working until each is refactored to read ActivePair().
//
// When Synths is empty, leaves legacy fields alone — caller may have
// just written legacy fields directly via the deprecated path, and
// the next migrateFromLegacy() round (next Load) will promote them.
func (c *Config) syncLegacyFromActive() {
	a := c.ActivePair()
	if a == nil {
		// Only wipe when ActiveSynthID was non-empty but the pair is
		// gone (post-RemovePair scenario). When Synths is just empty
		// from the start, don't touch legacy fields.
		if c.ActiveSynthID == "" && len(c.Synths) == 0 {
			return
		}
		c.PairedSynthURL = ""
		c.PairedSynthID = ""
		c.CompanionToken = ""
		c.HubURL = ""
		return
	}
	c.PairedSynthURL = a.URL
	c.PairedSynthID = a.ID
	c.CompanionToken = a.Token
	c.HubURL = a.HubURL
}

// DefaultSandboxRoot is the path derived from the active synth's ID.
// Returns empty if not paired.
func DefaultSandboxRoot(cfg *Config) string {
	if cfg.PairedSynthID == "" {
		return ""
	}
	home, _ := os.UserHomeDir()
	return filepath.Join(home, "SNTH", cfg.PairedSynthID)
}

func (c *Config) ensureDefaults() {
	if c.CompanionRole == "" {
		c.CompanionRole = RoleUserDevice
	}
	if c.AutoApproveBashPatterns == nil {
		c.AutoApproveBashPatterns = []string{
			"git status", "git diff", "git log", "git branch",
			"ls", "pwd", "whoami", "date",
			"rg ", "grep ", "find ",
			"cat ", "head ", "tail ", "wc ",
			"echo ",
		}
	}
	if c.LogRetentionDays == 0 {
		c.LogRetentionDays = 30
	}
	if c.SandboxRoots == nil {
		c.SandboxRoots = []string{}
	}
	if root := DefaultSandboxRoot(c); root != "" {
		has := false
		for _, r := range c.SandboxRoots {
			if r == root {
				has = true
				break
			}
		}
		if !has {
			c.SandboxRoots = append(c.SandboxRoots, root)
		}
	}
}
