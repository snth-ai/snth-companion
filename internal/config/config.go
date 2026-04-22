// Package config persists the companion's local state in a JSON file under
// ~/Library/Application Support/snth-companion/config.json on macOS (or
// $XDG_CONFIG_HOME/snth-companion on Linux, for when we add that target).
//
// The on-disk state is the single source of truth for what synth this
// companion is paired to, what local paths the user has granted access to,
// and what command patterns are auto-approved. Everything else (active WS
// connection, in-flight RPCs, UI state) is in-memory only.
package config

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"sync"
)

// Config is the persisted state. All fields are JSON-serializable so future
// versions can migrate by adding fields (zero values stay reasonable).
type Config struct {
	// PairedSynthURL is the base URL of the synth this companion is paired
	// to, e.g. "https://hub.snth.ai/instances/mia_snthai_bot". Empty means
	// not paired — the companion will show the pairing UI on start.
	PairedSynthURL string `json:"paired_synth_url,omitempty"`

	// PairedSynthID is the synth's slug, used for display and for deriving
	// the default sandbox root (~/SNTH/<slug>/).
	PairedSynthID string `json:"paired_synth_id,omitempty"`

	// CompanionToken is the bearer token issued by the synth at pair-claim
	// time. Sent on every WS connect as Authorization: Bearer <token>.
	CompanionToken string `json:"companion_token,omitempty"`

	// HubURL is the base URL of the hub that vended the pairing code
	// (e.g. "https://hub.snth.ai"). Persisted from the pair-claim flow
	// so UIs like Channels can deep-link into per-instance hub pages.
	HubURL string `json:"hub_url,omitempty"`

	// SandboxRoots are the absolute paths the companion will allow tools to
	// touch without an approval prompt. The default root (~/SNTH/<slug>/)
	// is injected by EnsureDefaults and always present.
	SandboxRoots []string `json:"sandbox_roots"`

	// AutoApproveBashPatterns is a list of prefix matches — if a requested
	// bash command starts with one of these, we skip the approval prompt.
	// Defaults: "git status", "git diff", "git log", "ls", "pwd", "rg",
	// "cat", "head", "tail", "wc".
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
	cfg.ensureDefaults()
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

// DefaultSandboxRoot is the path derived from the paired synth's ID.
// Returns empty if not paired.
func DefaultSandboxRoot(cfg *Config) string {
	if cfg.PairedSynthID == "" {
		return ""
	}
	home, _ := os.UserHomeDir()
	return filepath.Join(home, "SNTH", cfg.PairedSynthID)
}

func (c *Config) ensureDefaults() {
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
