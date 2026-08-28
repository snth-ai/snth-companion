package config

import "testing"

func TestActiveHubBindingSnapshot(t *testing.T) {
	mu.Lock()
	previous := current
	current = &Config{ActiveSynthID: "new", Synths: []SynthPair{{ID: "new", HubURL: "http://new", Token: "new-token"}}, PairedSynthID: "old", HubURL: "http://old", CompanionToken: "old-token"}
	mu.Unlock()
	t.Cleanup(func() { mu.Lock(); current = previous; mu.Unlock() })
	id, url, token := ActiveHubBinding()
	if id != "new" || url != "http://new" || token != "new-token" {
		t.Fatalf("mixed binding: %q %q %q", id, url, token)
	}
}
