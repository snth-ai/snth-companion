// Ring buffer of recent RPCs. Every tool_call → tool_result pair on the
// WS writes one entry when it resolves. In-memory only — on restart the
// log is empty. A persistent audit log would be nice but requires
// thinking about rotation, size caps, and what to redact; this ring is
// enough to answer "what did the synth just try to do on my machine?"
// which is the actual UX need.
package daemon

import (
	"sync"
	"time"
)

// AuditEntry is one RPC cycle. Zero-value is not useful — go through
// RecordAudit.
type AuditEntry struct {
	StartedAt  time.Time `json:"started_at"`
	DurationMs int64     `json:"duration_ms"`
	Tool       string    `json:"tool"`
	ArgsSummary string   `json:"args_summary"` // first ~200 chars of JSON
	Outcome    string    `json:"outcome"`      // "ok" | "error" | "denied"
	Error      string    `json:"error,omitempty"`
}

const auditBufSize = 500

var (
	auditMu  sync.Mutex
	auditBuf = make([]AuditEntry, 0, auditBufSize)
)

// RecordAudit appends to the ring buffer, evicting the oldest when
// full. Cheap — one lock, one slice shift.
func RecordAudit(e AuditEntry) {
	auditMu.Lock()
	defer auditMu.Unlock()
	if len(auditBuf) >= auditBufSize {
		// Drop oldest. copy+slice is O(n) but n≤500 so it's nothing.
		copy(auditBuf, auditBuf[1:])
		auditBuf = auditBuf[:auditBufSize-1]
	}
	auditBuf = append(auditBuf, e)
}

// RecentAudit returns the newest-first slice of the last n entries
// (n ≤ buffer size). Safe to call from HTTP handlers.
func RecentAudit(n int) []AuditEntry {
	auditMu.Lock()
	defer auditMu.Unlock()
	if n <= 0 || n > len(auditBuf) {
		n = len(auditBuf)
	}
	out := make([]AuditEntry, n)
	// Return newest-first.
	for i := 0; i < n; i++ {
		out[i] = auditBuf[len(auditBuf)-1-i]
	}
	return out
}
