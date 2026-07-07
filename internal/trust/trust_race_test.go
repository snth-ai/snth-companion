package trust

// trust_race_test.go — F3: SetTool must not race Get's map read.
//
// Pre-fix, SetTool shallow-copied State (sharing the Tools map) and
// mutated it unlocked while Get read st.Tools[tool] outside the lock =>
// "concurrent map read and map write". This test hammers both paths and
// must be clean under -race.

import (
	"fmt"
	"sync"
	"testing"
)

func TestSetToolGetRace(t *testing.T) {
	s, err := NewStore(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	// Pre-populate a few keys so Get has map entries to read.
	for i := 0; i < 8; i++ {
		_ = s.SetTool(fmt.Sprintf("tool%d", i), ModeTrusted)
	}

	var wg sync.WaitGroup
	stop := make(chan struct{})

	// Writers flipping tool modes (mutating the map).
	for w := 0; w < 4; w++ {
		wg.Add(1)
		go func(w int) {
			defer wg.Done()
			i := 0
			for {
				select {
				case <-stop:
					return
				default:
				}
				name := fmt.Sprintf("tool%d", (w+i)%8)
				if i%2 == 0 {
					_ = s.SetTool(name, ModeDenied)
				} else {
					_ = s.SetTool(name, ModePrompt) // delete path
				}
				i++
			}
		}(w)
	}

	// Readers hammering Get + GetDanger (reading the map).
	for r := 0; r < 4; r++ {
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
				name := fmt.Sprintf("tool%d", i%8)
				_ = s.Get(name, "")
				_ = s.GetDanger(name, "", true)
				i++
			}
		}()
	}

	// Run for a bounded number of iterations.
	done := make(chan struct{})
	go func() {
		for i := 0; i < 5000; i++ {
			_ = s.Snapshot()
		}
		close(done)
	}()
	<-done
	close(stop)
	wg.Wait()
}
