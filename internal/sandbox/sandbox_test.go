package sandbox

import (
	"os"
	"path/filepath"
	"testing"
)

// TestResolveSymlinkEscapeOnCreate proves A8: a not-yet-existing leaf
// under a symlinked directory must resolve OUT of the sandbox root so the
// containment check rejects it. Before the fix, Resolve skipped symlink
// resolution when the leaf didn't exist, so `<root>/link/x` (link -> /etc)
// stayed "inside" the root.
func TestResolveSymlinkEscapeOnCreate(t *testing.T) {
	root := t.TempDir()
	outside := t.TempDir() // stands in for /etc

	link := filepath.Join(root, "link")
	if err := os.Symlink(outside, link); err != nil {
		t.Fatalf("symlink: %v", err)
	}

	// Leaf does not exist yet (fs_write create case).
	target := filepath.Join(link, "x")
	resolved, err := Resolve(target)
	if err != nil {
		t.Fatalf("resolve: %v", err)
	}

	// The resolved path must live under `outside`, NOT under `root`, so a
	// containment check against root fails (escape rejected).
	realOutside, _ := filepath.EvalSymlinks(outside)
	if !underRoot(resolved, realOutside) {
		t.Fatalf("resolved %q did not follow the symlink into %q", resolved, realOutside)
	}
	if InsideAny([]string{root}, resolved) {
		t.Fatalf("symlink escape: resolved %q still counted as inside root %q", resolved, root)
	}
}

// TestResolveNormalCreateStaysInside proves the fix doesn't break the
// legitimate case: a new file under a real (non-symlinked) subdir of the
// root is still inside the root.
func TestResolveNormalCreateStaysInside(t *testing.T) {
	root, err := filepath.EvalSymlinks(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	sub := filepath.Join(root, "sub")
	if err := os.MkdirAll(sub, 0o755); err != nil {
		t.Fatal(err)
	}
	target := filepath.Join(sub, "newfile.txt") // doesn't exist yet
	resolved, err := Resolve(target)
	if err != nil {
		t.Fatalf("resolve: %v", err)
	}
	if !InsideAny([]string{root}, resolved) {
		t.Fatalf("normal create %q should be inside root %q, got resolved %q", target, root, resolved)
	}
}

func underRoot(p, root string) bool {
	if p == root {
		return true
	}
	sep := string(os.PathSeparator)
	return len(p) > len(root) && p[:len(root)] == root && p[len(root):len(root)+1] == sep
}
