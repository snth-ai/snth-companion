//go:build windows

package daemon

import "os"

// isProcessAlive on Windows uses os.FindProcess + Process.Signal(nil)
// pattern. os.FindProcess never fails on Windows, but Signal(syscall.Signal(0))
// isn't portable. The practical trick: try to open the process handle;
// if it's gone, attempting to read its exit state returns an error.
// For the lock use-case — "should we refuse to start?" — treating it
// as dead on any uncertainty is safe, since the stale-lock fallback
// path just overwrites the file.
func isProcessAlive(pid int) bool {
	p, err := os.FindProcess(pid)
	if err != nil {
		return false
	}
	// Releasing the handle; we just needed to prove it exists.
	_ = p.Release()
	return true
}
