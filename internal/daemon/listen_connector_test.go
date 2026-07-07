package daemon

// listen_connector_test.go — F4/P1.12: Stop/Start lifecycle race.

import (
	"sync"
	"testing"
	"time"

	"github.com/snth-ai/snth-companion/internal/config"
)

// TestListenStopStartRace hammers Start/Stop concurrently. Under -race it
// must be clean, and — critically — the old run's deferred cleanup must
// not clobber a newer session's state (F4). We assert the gen-guard
// invariant: after a Start that reports running, a stale run's defer with
// an older gen leaves running untouched.
func TestListenStopStartRace(t *testing.T) {
	config.SeedForTest(&config.Config{
		PairedSynthURL: "https://x",
		CompanionToken: "tok",
		PairedSynthID:  "s",
	})
	defer config.ResetForTest()

	l := &ListenConnector{status: "idle", device: "no-such-device"}

	var wg sync.WaitGroup
	for i := 0; i < 6; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			for j := 0; j < 100; j++ {
				_ = l.Start("no-such-device") // run() errors fast (device not found)
				l.Stop()
			}
		}()
	}
	wg.Wait()

	// Let any in-flight run goroutines finish their unwind.
	time.Sleep(200 * time.Millisecond)
	_ = l.Snapshot()
}

// TestListenStaleDeferDoesNotClobber is the focused F4 invariant: a run
// that started at gen=G must not clear running/status once gen has moved
// on (a newer Start happened). We simulate by capturing an old gen, doing
// a Start (bumps gen + sets running=true), then invoking the same guarded
// cleanup the old run's defer would run — it must be a no-op.
func TestListenStaleDeferDoesNotClobber(t *testing.T) {
	config.SeedForTest(&config.Config{
		PairedSynthURL: "https://x",
		CompanionToken: "tok",
		PairedSynthID:  "s",
	})
	defer config.ResetForTest()

	l := &ListenConnector{status: "idle", device: "d"}

	// Capture the gen an "old run" would have held.
	l.mu.Lock()
	oldGen := l.gen
	l.mu.Unlock()

	// A newer session starts: bumps gen, running=true, status=starting.
	if err := l.Start("d"); err != nil {
		t.Fatalf("start: %v", err)
	}
	defer l.Stop()

	// Simulate the OLD run's deferred cleanup firing late — this calls the
	// REAL cleanup path (finishRun) with the stale gen.
	l.finishRun(oldGen)

	l.mu.Lock()
	newGen := l.gen
	running := l.running
	l.mu.Unlock()

	if newGen == oldGen {
		t.Fatal("gen did not advance across Start — guard would be ineffective")
	}
	if !running {
		t.Fatal("stale run's cleanup clobbered the new session's running flag (F4 regression)")
	}
}
