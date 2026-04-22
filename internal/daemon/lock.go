package daemon

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"

	"github.com/snth-ai/snth-companion/internal/config"
)

// Single-instance lock: one companion per machine. Written to
// <config-dir>/lock.json at start; re-checked before binding the UI
// server. If a prior process is still alive (kill(pid, 0) succeeds),
// the new one refuses to start and prints the running instance's URL
// so the user can just open it.
//
// This is cheap belt-and-suspenders. The real guarantee against
// "superseded by new connection" WS wars comes from the lock refusing
// a second daemon before any sockets are opened.
//
// Not a hard OS-level file lock — those are a portability mess on
// macOS vs. Linux. pid-liveness check plus fs.mtime is enough for a
// user-facing background app.

type lockFile struct {
	PID    int    `json:"pid"`
	UIURL  string `json:"ui_url"`
	Opened string `json:"opened"`
}

func lockPath() string {
	return filepath.Join(filepath.Dir(config.Path()), "lock.json")
}

// AcquireLock attempts to register this process as the owner. On
// success returns a releaser; on failure returns an error whose
// message includes the URL of the running instance.
func AcquireLock(uiURL string) (func(), error) {
	p := lockPath()
	if err := os.MkdirAll(filepath.Dir(p), 0o700); err != nil {
		return nil, err
	}

	if raw, err := os.ReadFile(p); err == nil {
		var prev lockFile
		if jerr := json.Unmarshal(raw, &prev); jerr == nil && prev.PID > 0 {
			if isProcessAlive(prev.PID) {
				return nil, fmt.Errorf(
					"another snth-companion is already running (pid=%d, UI=%s). Open that UI instead, or kill it with: kill %d",
					prev.PID, prev.UIURL, prev.PID)
			}
		}
	}

	lf := lockFile{
		PID:    os.Getpid(),
		UIURL:  uiURL,
		Opened: nowISO(),
	}
	raw, _ := json.MarshalIndent(lf, "", "  ")
	if err := os.WriteFile(p, raw, 0o600); err != nil {
		return nil, fmt.Errorf("write lock: %w", err)
	}
	releaser := func() {
		// Only remove if we still own it (pid matches). Avoids racing
		// with a successor that took over after we somehow lost the
		// lock and got displaced.
		if raw, err := os.ReadFile(p); err == nil {
			var cur lockFile
			if jerr := json.Unmarshal(raw, &cur); jerr == nil && cur.PID == os.Getpid() {
				_ = os.Remove(p)
			}
		}
	}
	return releaser, nil
}

// isProcessAlive is provided per-platform: kill(pid, 0) on POSIX,
// best-effort on Windows. See lock_unix.go / lock_windows.go.
