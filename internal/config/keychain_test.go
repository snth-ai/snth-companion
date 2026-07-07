package config

// keychain_test.go — H8: token stored in the Keychain, JSON carries only a
// marker; fallback to plaintext when the Keychain is unavailable; E1/E2
// still hold with the Keychain in the loop.
//
// RED proof: disable the marker-swap in Save (leave redacted.Synths[i].Token
// as the real token) and TestTokenGoesToKeychainNotJSON fails — the on-disk
// JSON still contains the plaintext token.

import (
	"encoding/json"
	"errors"
	"os"
	"strings"
	"testing"
)

// fakeKeychain is an in-memory keychainStore. `unavailable` makes every op
// return ErrKeychainUnavailable to exercise the plaintext fallback.
type fakeKeychain struct {
	m           map[string]string
	unavailable bool
}

func newFakeKeychain() *fakeKeychain { return &fakeKeychain{m: map[string]string{}} }

func (f *fakeKeychain) Set(account, secret string) error {
	if f.unavailable {
		return ErrKeychainUnavailable
	}
	f.m[account] = secret
	return nil
}
func (f *fakeKeychain) Get(account string) (string, error) {
	if f.unavailable {
		return "", ErrKeychainUnavailable
	}
	v, ok := f.m[account]
	if !ok {
		return "", ErrKeychainMiss
	}
	return v, nil
}
func (f *fakeKeychain) Delete(account string) error {
	if f.unavailable {
		return ErrKeychainUnavailable
	}
	delete(f.m, account)
	return nil
}

// withFakeKeychain installs a fake backend for the test and restores the
// real one after.
func withFakeKeychain(t *testing.T) *fakeKeychain {
	t.Helper()
	prev := kc
	fk := newFakeKeychain()
	kc = fk
	t.Cleanup(func() { kc = prev })
	return fk
}

func rawConfigJSON(t *testing.T) string {
	t.Helper()
	b, err := os.ReadFile(Path())
	if err != nil {
		t.Fatalf("read config.json: %v", err)
	}
	return string(b)
}

func TestTokenGoesToKeychainNotJSON(t *testing.T) {
	isolate(t)
	fk := withFakeKeychain(t)

	SeedForTest(&Config{})
	if err := Update(func(c *Config) {
		c.AddOrUpdatePair(SynthPair{ID: "s1", URL: "https://s1", Token: "supersecret", HubURL: "https://hub"})
		_ = c.SetActive("s1")
	}); err != nil {
		t.Fatal(err)
	}

	// JSON must NOT contain the plaintext token; the marker instead.
	raw := rawConfigJSON(t)
	if strings.Contains(raw, "supersecret") {
		t.Fatalf("plaintext token leaked into config.json:\n%s", raw)
	}
	if !strings.Contains(raw, keychainMarker) {
		t.Fatalf("marker not written into config.json:\n%s", raw)
	}
	// The Keychain has the real token under the pair ID.
	if got := fk.m["s1"]; got != "supersecret" {
		t.Fatalf("keychain missing token for s1: %q", got)
	}

	// A fresh Load must re-hydrate the in-memory token from the Keychain.
	ResetForTest()
	kc = fk // isolate() -> ResetForTest cleared, but keep the same fake
	cfg, err := Load()
	if err != nil {
		t.Fatal(err)
	}
	ap := cfg.ActivePair()
	if ap == nil || ap.Token != "supersecret" {
		t.Fatalf("token not re-hydrated from keychain: %+v", ap)
	}
	if cfg.CompanionToken != "supersecret" {
		t.Fatalf("legacy scalar not re-hydrated: %q", cfg.CompanionToken)
	}
}

func TestKeychainUnavailableFallsBackToPlaintext(t *testing.T) {
	isolate(t)
	fk := withFakeKeychain(t)
	fk.unavailable = true // headless: every security call errors

	SeedForTest(&Config{})
	if err := Update(func(c *Config) {
		c.AddOrUpdatePair(SynthPair{ID: "s1", URL: "https://s1", Token: "plainfallback"})
		_ = c.SetActive("s1")
	}); err != nil {
		t.Fatal(err)
	}

	// With the Keychain down, the token MUST persist plaintext in JSON so
	// nothing breaks.
	raw := rawConfigJSON(t)
	if !strings.Contains(raw, "plainfallback") {
		t.Fatalf("fallback failed: token not in JSON when keychain unavailable:\n%s", raw)
	}

	// And a Load still yields it (from JSON, since the fake is still down).
	ResetForTest()
	kc = fk
	cfg, err := Load()
	if err != nil {
		t.Fatal(err)
	}
	if ap := cfg.ActivePair(); ap == nil || ap.Token != "plainfallback" {
		t.Fatalf("plaintext fallback not loaded: %+v", ap)
	}
}

// E1 with the Keychain in the loop: removing the last pair must clear the
// token EVERYWHERE — legacy scalars AND the Keychain entry.
func TestUnpairClearsTokenIncludingKeychain(t *testing.T) {
	isolate(t)
	fk := withFakeKeychain(t)

	SeedForTest(&Config{})
	if err := Update(func(c *Config) {
		c.AddOrUpdatePair(SynthPair{ID: "s1", URL: "https://s1", Token: "t1"})
		_ = c.SetActive("s1")
	}); err != nil {
		t.Fatal(err)
	}
	if fk.m["s1"] == "" {
		t.Fatalf("precondition: keychain should hold s1 token")
	}

	if err := Update(func(c *Config) { c.RemovePair("s1") }); err != nil {
		t.Fatal(err)
	}
	cfg := Get()
	if cfg.CompanionToken != "" || cfg.PairedSynthID != "" {
		t.Fatalf("E1: legacy scalars not cleared: %+v", cfg)
	}
	if _, ok := fk.m["s1"]; ok {
		t.Fatalf("E1: keychain entry for removed pair not deleted: %v", fk.m)
	}
	if _, ok := fk.m[legacyKeychainAccount]; ok {
		t.Fatalf("E1: legacy keychain entry not deleted: %v", fk.m)
	}
	// JSON must carry no token either.
	raw := rawConfigJSON(t)
	if strings.Contains(raw, "t1") {
		t.Fatalf("E1: token still in JSON after unpair:\n%s", raw)
	}
}

// E2 with the Keychain: re-pair to a new synth survives a Save round-trip;
// the Keychain holds the NEW token, the old one is gone.
func TestPairSaveKeychainRoundTrip(t *testing.T) {
	isolate(t)
	fk := withFakeKeychain(t)

	SeedForTest(&Config{})
	if err := Update(func(c *Config) {
		c.AddOrUpdatePair(SynthPair{ID: "s1", URL: "https://s1", Token: "t1"})
		_ = c.SetActive("s1")
	}); err != nil {
		t.Fatal(err)
	}
	if err := Update(func(c *Config) {
		c.RemovePair("s1")
		c.AddOrUpdatePair(SynthPair{ID: "s2", URL: "https://s2", Token: "t2"})
		_ = c.SetActive("s2")
	}); err != nil {
		t.Fatal(err)
	}

	ResetForTest()
	kc = fk
	cfg, err := Load()
	if err != nil {
		t.Fatal(err)
	}
	if cfg.ActiveSynthID != "s2" {
		t.Fatalf("E2: active not s2: %q", cfg.ActiveSynthID)
	}
	if ap := cfg.ActivePair(); ap == nil || ap.Token != "t2" {
		t.Fatalf("E2: new token not persisted/hydrated: %+v", ap)
	}
	if fk.m["s2"] != "t2" {
		t.Fatalf("E2: keychain missing s2 token: %v", fk.m)
	}
	if _, ok := fk.m["s1"]; ok {
		t.Fatalf("E2: stale s1 keychain entry survived: %v", fk.m)
	}
}

// Sanity that ErrKeychainMiss/Unavailable are distinct (guards a refactor
// that collapses them).
func TestKeychainErrsDistinct(t *testing.T) {
	if errors.Is(ErrKeychainMiss, ErrKeychainUnavailable) {
		t.Fatal("miss and unavailable must be distinct sentinels")
	}
}

// Ensure a Save with a fake keychain doesn't accidentally emit a "token":
// "<plaintext>" pair — belt-and-suspenders JSON shape check.
func TestJSONShapeHasMarkerField(t *testing.T) {
	isolate(t)
	withFakeKeychain(t)
	SeedForTest(&Config{})
	_ = Update(func(c *Config) {
		c.AddOrUpdatePair(SynthPair{ID: "s1", URL: "u", Token: "zzz"})
		_ = c.SetActive("s1")
	})
	var onDisk Config
	if err := json.Unmarshal([]byte(rawConfigJSON(t)), &onDisk); err != nil {
		t.Fatal(err)
	}
	for _, s := range onDisk.Synths {
		if s.Token != keychainMarker {
			t.Fatalf("synth %s token in JSON = %q, want marker", s.ID, s.Token)
		}
	}
}
