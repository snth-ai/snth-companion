package config

// testhelpers.go — small seams so other packages' tests can drive the
// config singleton without touching disk. Not build-tagged (needs to be
// visible to external _test packages), but harmless in production: nobody
// calls these outside tests.

// SeedForTest installs cfg as the process-global config (bypassing disk
// Load). It applies the same defaults + legacy sync that Load would, so
// callers get a realistic Config. Intended for tests only.
func SeedForTest(cfg *Config) {
	mu.Lock()
	defer mu.Unlock()
	if cfg == nil {
		cfg = &Config{}
	}
	cfg.migrateFromLegacy()
	cfg.ensureDefaults()
	cfg.syncLegacyFromActive()
	current = cfg
}

// ResetForTest clears the process-global config so a later Load re-reads
// from disk. Intended for tests only.
func ResetForTest() {
	mu.Lock()
	defer mu.Unlock()
	current = nil
}
