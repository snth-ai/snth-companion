//go:build !windows

package daemon

import "syscall"

// isProcessAlive uses kill(pid, 0) on POSIX. Signal 0 doesn't actually
// deliver; it just checks whether the process exists and whether we
// have permission to send it signals.
func isProcessAlive(pid int) bool {
	if err := syscall.Kill(pid, 0); err != nil {
		// ESRCH = no such process; EPERM = it's someone else's process
		// (still alive, different user — err on the side of blocking).
		return err != syscall.ESRCH
	}
	return true
}
