package daemon

// audit_persist_test.go — H5 durable audit log.
//
// RED proof: with the persistAudit() call removed from RecordAudit (or the
// marshal/write disabled in append), TestAuditPersistsToDisk fails because
// no JSONL lines land on disk. TestAuditRetentionPrunesOldFiles fails if
// pruneLocked is a no-op (the stale file survives).

import (
	"bufio"
	"encoding/json"
	"os"
	"path/filepath"
	"testing"
	"time"
)

// resetPersistForTest clears the process-global persister between tests.
func resetPersistForTest() {
	auditPersistMu.Lock()
	if auditPersist != nil && auditPersist.openFile != nil {
		_ = auditPersist.openFile.Close()
	}
	auditPersist = nil
	auditPersistMu.Unlock()
	// Also clear the ring so newest-first assertions are deterministic.
	auditMu.Lock()
	auditBuf = auditBuf[:0]
	auditMu.Unlock()
}

func readJSONL(t *testing.T, path string) []AuditEntry {
	t.Helper()
	f, err := os.Open(path)
	if err != nil {
		t.Fatalf("open %s: %v", path, err)
	}
	defer f.Close()
	var out []AuditEntry
	sc := bufio.NewScanner(f)
	for sc.Scan() {
		line := sc.Bytes()
		if len(line) == 0 {
			continue
		}
		var e AuditEntry
		if err := json.Unmarshal(line, &e); err != nil {
			t.Fatalf("line is not valid JSON: %q: %v", string(line), err)
		}
		out = append(out, e)
	}
	if err := sc.Err(); err != nil {
		t.Fatalf("scan: %v", err)
	}
	return out
}

func TestAuditPersistsToDisk(t *testing.T) {
	resetPersistForTest()
	t.Cleanup(resetPersistForTest)

	dir := t.TempDir()
	fixed := time.Date(2026, 7, 7, 12, 0, 0, 0, time.UTC)
	initAuditPersistenceAt(dir, 30, func() time.Time { return fixed })

	RecordAudit(AuditEntry{StartedAt: fixed, Tool: "remote_bash", ArgsSummary: `{"cmd":"ls"}`, Outcome: "ok"})
	RecordAuditApproval("remote_fs_write", `{"path":"~/x"}`, "denied", "trusted-deny", "")

	day := fixed.Format("2006-01-02")
	path := filepath.Join(dir, auditFilePrefix+day+".jsonl")
	got := readJSONL(t, path)
	if len(got) != 2 {
		t.Fatalf("expected 2 JSONL entries on disk, got %d", len(got))
	}
	if got[0].Tool != "remote_bash" || got[0].Outcome != "ok" {
		t.Fatalf("entry 0 mismatch: %+v", got[0])
	}
	if got[1].Tool != "remote_fs_write" || got[1].Outcome != "approval" || got[1].Decision != "denied" {
		t.Fatalf("entry 1 mismatch: %+v", got[1])
	}

	// The in-memory ring must still work (newest-first).
	ring := RecentAudit(0)
	if len(ring) != 2 {
		t.Fatalf("ring expected 2 entries, got %d", len(ring))
	}
	if ring[0].Tool != "remote_fs_write" {
		t.Fatalf("ring not newest-first: %+v", ring[0])
	}
}

func TestAuditRetentionPrunesOldFiles(t *testing.T) {
	resetPersistForTest()
	t.Cleanup(resetPersistForTest)

	dir := t.TempDir()
	if err := os.MkdirAll(dir, 0o700); err != nil {
		t.Fatal(err)
	}
	// Synthetic old file (10 days back) + a within-window file (yesterday).
	today := time.Date(2026, 7, 7, 0, 0, 0, 0, time.UTC)
	oldDay := today.AddDate(0, 0, -10).Format("2006-01-02")
	recentDay := today.AddDate(0, 0, -1).Format("2006-01-02")
	oldPath := filepath.Join(dir, auditFilePrefix+oldDay+".jsonl")
	recentPath := filepath.Join(dir, auditFilePrefix+recentDay+".jsonl")
	if err := os.WriteFile(oldPath, []byte("{}\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(recentPath, []byte("{}\n"), 0o600); err != nil {
		t.Fatal(err)
	}

	// Retention 7 days, clock pinned at today → init runs a boot prune.
	initAuditPersistenceAt(dir, 7, func() time.Time { return today.Add(12 * time.Hour) })

	if _, err := os.Stat(oldPath); !os.IsNotExist(err) {
		t.Fatalf("old file past retention was NOT pruned: %v", err)
	}
	if _, err := os.Stat(recentPath); err != nil {
		t.Fatalf("within-window file was wrongly pruned: %v", err)
	}
}

func TestAuditPersistenceDisabledWhenRetentionNonPositive(t *testing.T) {
	resetPersistForTest()
	t.Cleanup(resetPersistForTest)

	dir := t.TempDir()
	initAuditPersistenceAt(dir, 0, time.Now) // disabled
	RecordAudit(AuditEntry{Tool: "remote_bash", Outcome: "ok"})

	// No audit dir / no files should have been created.
	entries, err := os.ReadDir(dir)
	if err != nil {
		t.Fatal(err)
	}
	for _, e := range entries {
		if len(e.Name()) >= len(auditFilePrefix) && e.Name()[:len(auditFilePrefix)] == auditFilePrefix {
			t.Fatalf("persistence disabled but wrote %s", e.Name())
		}
	}
	// Ring must still record.
	if len(RecentAudit(0)) != 1 {
		t.Fatalf("ring should record even when disk persistence is off")
	}
}
