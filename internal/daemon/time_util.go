package daemon

import "time"

// nowISO is a tiny wrapper around time.Now().Format(time.RFC3339) —
// the half-dozen places we need to stamp "now" for JSON responses look
// cleaner with this than inline formatting.
func nowISO() string {
	return time.Now().UTC().Format(time.RFC3339)
}
