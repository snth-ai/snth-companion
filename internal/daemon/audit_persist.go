package daemon

// audit_persist.go — durable on-disk audit log (review finding H5).
//
// The in-memory ring (audit.go) answers "what did the synth just try to
// do?" but is wiped on restart. For a shipped product we also keep an
// append-only JSONL trail under the companion support dir so there is a
// durable record of every RPC + approval decision on the user's Mac.
//
// Layout: <support>/audit/audit-YYYY-MM-DD.jsonl — one JSON object per
// line, one file per UTC day. Daily rotation caps any single file and
// makes age-based pruning a whole-file delete. Retention is driven by
// config.LogRetentionDays:
//
//	N > 0  → keep the last N days, prune older files on boot + on rollover.
//	N <= 0 → DISABLED: no disk persistence at all (safe, documented
//	         choice — the caller explicitly asked for zero retention, so
//	         we honor it rather than silently defaulting to 30). The
//	         in-memory ring is unaffected either way.
//
// ensureDefaults() sets LogRetentionDays=30 when the field is 0 on a
// fresh/loaded config, so the DEFAULT is 30-day persistence; only an
// explicit non-positive value disables it.

import (
	"bufio"
	"encoding/json"
	"os"
	"path/filepath"
	"sync"
	"time"

	"github.com/snth-ai/snth-companion/internal/config"
)

const auditDirName = "audit"

// auditFilePrefix + "YYYY-MM-DD.jsonl" is one day's file.
const auditFilePrefix = "audit-"

// auditPersister owns the disk side. It is process-global, initialized
// lazily on the first RecordAudit after InitAuditPersistence has run.
type auditPersister struct {
	mu        sync.Mutex
	dir       string        // <support>/audit
	retention int           // config.LogRetentionDays, cached at init
	nowFn     func() time.Time
	// openDay + openFile cache the currently-open day so we don't
	// re-open + re-stat on every entry.
	openDay  string
	openFile *os.File
}

var (
	auditPersistMu sync.Mutex
	auditPersist   *auditPersister // nil = persistence not initialized / disabled
)

// InitAuditPersistence wires durable audit logging using the shared
// support dir and the retention setting. Called once at boot AFTER
// config.Load. Retention <= 0 disables disk persistence (ring only).
// Best-effort: a failure to prepare the dir just leaves persistence off
// and logs nothing fatal — the ring still works.
func InitAuditPersistence(retentionDays int) {
	initAuditPersistenceAt(filepath.Join(config.Dir(), auditDirName), retentionDays, time.Now)
}

// initAuditPersistenceAt is the test seam — it takes an explicit dir +
// clock so tests never touch the real support dir.
func initAuditPersistenceAt(dir string, retentionDays int, nowFn func() time.Time) {
	auditPersistMu.Lock()
	defer auditPersistMu.Unlock()
	if retentionDays <= 0 {
		// Explicitly disabled — tear down any prior persister.
		if auditPersist != nil && auditPersist.openFile != nil {
			_ = auditPersist.openFile.Close()
		}
		auditPersist = nil
		return
	}
	if err := os.MkdirAll(dir, 0o700); err != nil {
		// Can't persist — leave it off, ring still records.
		auditPersist = nil
		return
	}
	auditPersist = &auditPersister{
		dir:       dir,
		retention: retentionDays,
		nowFn:     nowFn,
	}
	auditPersist.pruneLocked() // sweep stale files on boot
}

// persistAudit is called by RecordAudit for every entry. No-op when
// persistence is disabled. Failures are swallowed (best-effort trail).
func persistAudit(e AuditEntry) {
	auditPersistMu.Lock()
	p := auditPersist
	auditPersistMu.Unlock()
	if p == nil {
		return
	}
	p.append(e)
}

// append writes one JSONL line to today's file, rotating (and pruning)
// when the UTC day changes.
func (p *auditPersister) append(e AuditEntry) {
	p.mu.Lock()
	defer p.mu.Unlock()

	day := p.nowFn().UTC().Format("2006-01-02")
	if p.openFile == nil || p.openDay != day {
		// Day rolled over (or first write) — close the old file, prune,
		// open the new one.
		if p.openFile != nil {
			_ = p.openFile.Close()
			p.openFile = nil
		}
		p.pruneLocked()
		f, err := os.OpenFile(p.filePathForDay(day), os.O_APPEND|os.O_CREATE|os.O_WRONLY, 0o600)
		if err != nil {
			return
		}
		p.openFile = f
		p.openDay = day
	}

	line, err := json.Marshal(e)
	if err != nil {
		return
	}
	w := bufio.NewWriter(p.openFile)
	_, _ = w.Write(line)
	_ = w.WriteByte('\n')
	_ = w.Flush()
}

func (p *auditPersister) filePathForDay(day string) string {
	return filepath.Join(p.dir, auditFilePrefix+day+".jsonl")
}

// pruneLocked deletes audit files whose day is older than the retention
// window. Caller holds p.mu (or is in init before any concurrent use).
// Retention N means: keep files dated within the last N days (today
// inclusive), delete anything older.
func (p *auditPersister) pruneLocked() {
	entries, err := os.ReadDir(p.dir)
	if err != nil {
		return
	}
	// Cutoff = midnight UTC, (retention-1) days before today. Files
	// dated strictly before the cutoff day are removed.
	today := p.nowFn().UTC().Truncate(24 * time.Hour)
	cutoff := today.AddDate(0, 0, -(p.retention - 1))
	for _, ent := range entries {
		if ent.IsDir() {
			continue
		}
		name := ent.Name()
		if len(name) != len(auditFilePrefix)+len("2006-01-02")+len(".jsonl") ||
			name[:len(auditFilePrefix)] != auditFilePrefix {
			continue
		}
		dayStr := name[len(auditFilePrefix) : len(auditFilePrefix)+len("2006-01-02")]
		day, perr := time.Parse("2006-01-02", dayStr)
		if perr != nil {
			continue
		}
		if day.Before(cutoff) {
			_ = os.Remove(filepath.Join(p.dir, name))
		}
	}
}
