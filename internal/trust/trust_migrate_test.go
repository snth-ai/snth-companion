package trust

// trust_migrate_test.go — H7: single Application Support dir + migration of
// a legacy trust.json from the old "SNTH Companion" dir into the shared
// "snth-companion" dir.
//
// RED proof: revert NewStore to default supportDir to the old
// "SNTH Companion" path (and drop the migrateLegacyTrust call) and
// TestMigratesLegacyTrust fails — the store reads the empty new dir and the
// seeded Master:true state is lost (Snapshot().Master == false).
//
// These tests re-home HOME so config.Dir() (the new default) and the legacy
// path both resolve under a temp dir; darwin only (matches the app target).

import (
	"encoding/json"
	"os"
	"path/filepath"
	"runtime"
	"testing"

	"github.com/snth-ai/snth-companion/internal/config"
)

func newSupportDir(t *testing.T) string {
	t.Helper()
	return config.Dir()
}

func legacyDir(t *testing.T) string {
	t.Helper()
	home, err := os.UserHomeDir()
	if err != nil {
		t.Fatal(err)
	}
	return filepath.Join(home, "Library", "Application Support", "SNTH Companion")
}

func writeTrustJSON(t *testing.T, dir string, st State) {
	t.Helper()
	if err := os.MkdirAll(dir, 0o700); err != nil {
		t.Fatal(err)
	}
	b, err := json.Marshal(st)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "trust.json"), b, 0o600); err != nil {
		t.Fatal(err)
	}
}

func TestMigratesLegacyTrust(t *testing.T) {
	if runtime.GOOS != "darwin" {
		t.Skip("H7 legacy dir path is darwin-only")
	}
	t.Setenv("HOME", t.TempDir())

	// Seed a legacy trust.json (Master ON) in the OLD dir; new dir empty.
	writeTrustJSON(t, legacyDir(t), State{Master: true})

	s, err := NewStore("") // default → new shared dir, triggers migration
	if err != nil {
		t.Fatal(err)
	}
	// State must have been carried over.
	if !s.Snapshot().Master {
		t.Fatalf("legacy trust state was NOT migrated (Master lost)")
	}
	// The store must now point at the NEW dir.
	if got, want := s.Path(), filepath.Join(newSupportDir(t), "trust.json"); got != want {
		t.Fatalf("store path = %q, want new-dir %q", got, want)
	}
	if _, err := os.Stat(s.Path()); err != nil {
		t.Fatalf("migrated file missing at new path: %v", err)
	}
	// Legacy file should have been removed (single-dir invariant).
	if _, err := os.Stat(filepath.Join(legacyDir(t), "trust.json")); !os.IsNotExist(err) {
		t.Fatalf("legacy trust.json not removed after migration: %v", err)
	}
}

func TestFreshInstallUsesNewDir(t *testing.T) {
	if runtime.GOOS != "darwin" {
		t.Skip("darwin-only")
	}
	t.Setenv("HOME", t.TempDir())

	// No legacy file, no new file.
	s, err := NewStore("")
	if err != nil {
		t.Fatal(err)
	}
	if got, want := s.Path(), filepath.Join(newSupportDir(t), "trust.json"); got != want {
		t.Fatalf("fresh install path = %q, want %q", got, want)
	}
	// Zero-state, everything prompts.
	if s.Snapshot().Master {
		t.Fatalf("fresh install should be zero-state (master off)")
	}
}

func TestExistingNewDirNotClobberedByLegacy(t *testing.T) {
	if runtime.GOOS != "darwin" {
		t.Skip("darwin-only")
	}
	t.Setenv("HOME", t.TempDir())

	// New dir already has trust (Master OFF, one denied tool); a STALE
	// legacy file (Master ON) must NOT overwrite it.
	writeTrustJSON(t, newSupportDir(t), State{Master: false, Tools: map[string]ToolMode{"remote_bash": ModeDenied}})
	writeTrustJSON(t, legacyDir(t), State{Master: true})

	s, err := NewStore("")
	if err != nil {
		t.Fatal(err)
	}
	snap := s.Snapshot()
	if snap.Master {
		t.Fatalf("stale legacy file clobbered the existing new-dir state")
	}
	if snap.Tools["remote_bash"] != ModeDenied {
		t.Fatalf("new-dir per-tool state lost: %+v", snap.Tools)
	}
}
