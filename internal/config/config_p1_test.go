package config

// config_p1_test.go — E1/E2/E3 correctness of the config singleton.
//
// These tests exercise the pairs-list <-> legacy-scalar sync and the
// Save() locking. They point HOME at a temp dir so Save writes there.

import (
	"sync"
	"testing"
)

// isolate re-homes the config dir to a temp dir and resets the singleton
// so each test starts clean and never touches the real config.json.
func isolate(t *testing.T) {
	t.Helper()
	tmp := t.TempDir()
	t.Setenv("HOME", tmp)
	t.Setenv("XDG_CONFIG_HOME", tmp)
	ResetForTest()
	t.Cleanup(ResetForTest)
}

// E1 — removing the last pair must wipe the legacy scalars so loop() reads
// "not paired" instead of reconnecting to the just-unpaired synth.
func TestRemoveLastPairWipesLegacyScalars(t *testing.T) {
	isolate(t)
	SeedForTest(&Config{})
	if err := Update(func(c *Config) {
		c.AddOrUpdatePair(SynthPair{ID: "s1", URL: "https://s1", Token: "t1", HubURL: "https://hub"})
	}); err != nil {
		t.Fatal(err)
	}
	cfg := Get()
	if cfg.PairedSynthURL != "https://s1" || cfg.CompanionToken != "t1" {
		t.Fatalf("legacy scalars not seeded from pair: %+v", cfg)
	}

	if err := Update(func(c *Config) { c.RemovePair("s1") }); err != nil {
		t.Fatal(err)
	}
	cfg = Get()
	if cfg.PairedSynthURL != "" || cfg.PairedSynthID != "" || cfg.CompanionToken != "" || cfg.HubURL != "" {
		t.Fatalf("legacy scalars not cleared after removing last pair: url=%q id=%q tok=%q hub=%q",
			cfg.PairedSynthURL, cfg.PairedSynthID, cfg.CompanionToken, cfg.HubURL)
	}
	if len(cfg.Synths) != 0 || cfg.ActiveSynthID != "" {
		t.Fatalf("pairs not empty: %+v", cfg.Synths)
	}
}

// E2 — a fresh pair added via AddOrUpdatePair+SetActive survives a Save()
// round-trip; the legacy scalars reflect the NEW pair, not a stale one.
// This models the fixed legacy pair-save/claim handlers (which now write
// into the pairs list).
func TestPairSaveSurvivesSaveRoundTrip(t *testing.T) {
	isolate(t)
	// Start with an existing active pair (s1).
	SeedForTest(&Config{})
	if err := Update(func(c *Config) {
		c.AddOrUpdatePair(SynthPair{ID: "s1", URL: "https://s1", Token: "t1"})
		_ = c.SetActive("s1")
	}); err != nil {
		t.Fatal(err)
	}

	// Now "re-pair" to a different synth via the fixed path.
	if err := Update(func(c *Config) {
		c.AddOrUpdatePair(SynthPair{ID: "s2", URL: "https://s2", Token: "t2"})
		_ = c.SetActive("s2")
	}); err != nil {
		t.Fatal(err)
	}

	// Force a fresh Load from disk to prove it persisted, not just cached.
	ResetForTest()
	cfg, err := Load()
	if err != nil {
		t.Fatal(err)
	}
	if cfg.ActiveSynthID != "s2" {
		t.Fatalf("active synth not persisted as s2: %q", cfg.ActiveSynthID)
	}
	if cfg.PairedSynthURL != "https://s2" || cfg.CompanionToken != "t2" || cfg.PairedSynthID != "s2" {
		t.Fatalf("legacy scalars not from the new pair: url=%q id=%q tok=%q",
			cfg.PairedSynthURL, cfg.PairedSynthID, cfg.CompanionToken)
	}
}

// E3 — Save() must hold the lock across syncLegacyFromActive (which WRITES
// fields) + marshal, so it can't race a concurrent Update. Run under -race.
func TestConfigSaveUpdateRace(t *testing.T) {
	isolate(t)
	SeedForTest(&Config{})
	if err := Update(func(c *Config) {
		c.AddOrUpdatePair(SynthPair{ID: "s1", URL: "https://s1", Token: "t1"})
		_ = c.SetActive("s1")
	}); err != nil {
		t.Fatal(err)
	}

	var wg sync.WaitGroup
	stop := make(chan struct{})
	// Writer: hammer Update (mutates the struct + Saves).
	wg.Add(1)
	go func() {
		defer wg.Done()
		i := 0
		for {
			select {
			case <-stop:
				return
			default:
			}
			i++
			_ = Update(func(c *Config) {
				c.MenubarDisplay = MenubarHeart
				for j := range c.Synths {
					c.Synths[j].Label = "l"
				}
			})
		}
	}()
	// Concurrent Saver.
	wg.Add(1)
	go func() {
		defer wg.Done()
		for {
			select {
			case <-stop:
				return
			default:
			}
			_ = Save()
		}
	}()
	// Concurrent Save from a second goroutine — this is the E3 race: two
	// Saves (each running syncLegacyFromActive = a WRITE) plus Update all
	// touching the struct. Under the fix all three hold mu; pre-fix Save
	// wrote under RLock and raced.
	wg.Add(1)
	go func() {
		defer wg.Done()
		for {
			select {
			case <-stop:
				return
			default:
			}
			_ = Save()
		}
	}()

	// Let them run briefly.
	for i := 0; i < 2000; i++ {
		_ = Get()
	}
	close(stop)
	wg.Wait()
}
