package daemon

// pair_save_test.go — E2 regression: the legacy /pair/save handler must
// persist the new credentials, not have them clobbered by the
// syncLegacyFromActive copy from a stale active pair.

import (
	"net/http/httptest"
	"net/url"
	"strings"
	"testing"

	"github.com/snth-ai/snth-companion/internal/config"
)

func TestLegacyPairSavePersistsCreds(t *testing.T) {
	tmp := t.TempDir()
	t.Setenv("HOME", tmp)
	t.Setenv("XDG_CONFIG_HOME", tmp)
	config.ResetForTest()
	t.Cleanup(config.ResetForTest)
	// Start from empty config.
	config.SeedForTest(&config.Config{})

	s := &UIServer{Client: &Client{}}
	t.Cleanup(s.Client.Stop)

	form := url.Values{}
	form.Set("synth_url", "https://newsynth.example")
	form.Set("token", "brand-new-token")
	form.Set("synth_id", "newsynth")

	r := httptest.NewRequest("POST", "http://127.0.0.1/pair/save", strings.NewReader(form.Encode()))
	r.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	w := httptest.NewRecorder()
	s.handlePairSave(w, r)

	if w.Code != 303 {
		t.Fatalf("pair save: want 303 redirect, got %d (%s)", w.Code, w.Body.String())
	}

	// Force a fresh load from disk to prove the creds actually persisted.
	config.ResetForTest()
	cfg, err := config.Load()
	if err != nil {
		t.Fatal(err)
	}
	if cfg.PairedSynthURL != "https://newsynth.example" || cfg.CompanionToken != "brand-new-token" || cfg.PairedSynthID != "newsynth" {
		t.Fatalf("new pair creds not persisted: url=%q id=%q tok=%q",
			cfg.PairedSynthURL, cfg.PairedSynthID, cfg.CompanionToken)
	}
	if cfg.ActiveSynthID != "newsynth" {
		t.Fatalf("active synth not set: %q", cfg.ActiveSynthID)
	}
}
