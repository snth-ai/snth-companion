package trust

import (
	"testing"
	"time"
)

// masterStore returns a store with master-trust ON (no expiry) at a temp
// path, so we can check what master auto-approves.
func masterStore(t *testing.T) *Store {
	t.Helper()
	s, err := NewStore(t.TempDir())
	if err != nil {
		t.Fatalf("new store: %v", err)
	}
	if err := s.SetMaster(true, nil); err != nil {
		t.Fatalf("set master: %v", err)
	}
	return s
}

// TestMasterNeverAutoApprovesHighImpact proves P0.6/A6: with master-trust
// ON, the high-impact verbs still return Prompt (not Trusted). Keys are the
// descriptor Names the central gate passes.
func TestMasterNeverAutoApprovesHighImpact(t *testing.T) {
	s := masterStore(t)
	mustPrompt := []string{
		"remote_bash",
		"remote_messages_recent",
		"remote_messages_send",
		"remote_fs_write",
		"remote_browser",
		"remote_contacts_search",
		"remote_yt_dlp",
		"remote_subagent",
		"companion_ssh",
	}
	for _, tool := range mustPrompt {
		if got := s.Get(tool, ""); got != DecisionPrompt {
			t.Errorf("master-trust auto-approved %q (got %v), must stay Prompt", tool, got)
		}
	}
}

// TestMasterAutoApprovesLowImpact confirms master-trust still works for a
// tool NOT on the always-prompt list (so we didn't over-lock).
func TestMasterAutoApprovesLowImpact(t *testing.T) {
	s := masterStore(t)
	if got := s.Get("remote_notes_create", ""); got != DecisionTrusted {
		t.Fatalf("master-trust should auto-approve remote_notes_create, got %v", got)
	}
}

// TestPerToolOverrideStillWins confirms an explicit per-tool trusted mode
// overrides the always-prompt safety net (user is the source of truth).
func TestPerToolOverrideStillWins(t *testing.T) {
	s := masterStore(t)
	if err := s.SetTool("remote_bash", ModeTrusted); err != nil {
		t.Fatal(err)
	}
	if got := s.Get("remote_bash", ""); got != DecisionTrusted {
		t.Fatalf("explicit per-tool trusted should win, got %v", got)
	}
}

// TestExpiredMasterPrompts sanity-checks the expiry path is unaffected.
func TestExpiredMasterPrompts(t *testing.T) {
	s, err := NewStore(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	past := time.Now().Add(-time.Hour)
	if err := s.SetMaster(true, &past); err != nil {
		t.Fatal(err)
	}
	if got := s.Get("remote_notes_create", ""); got != DecisionPrompt {
		t.Fatalf("expired master should prompt, got %v", got)
	}
}
