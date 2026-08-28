// Package sandbox decides whether a requested path is inside one of the
// granted roots. It's the last line of defense before a tool touches disk.
//
// The rules:
//   - Every path is resolved via filepath.Abs + Clean, which collapses
//     "../.." / symlink tricks away.
//   - A path is "inside" a root iff abs(path) == root OR abs(path) starts
//     with root + string(os.PathSeparator). Plain HasPrefix is NOT enough
//     ("~/SNTH/foo" matches "~/SNTH/foobar" falsely).
//   - Paths outside any root are allowed only after explicit user approval.
//     The approval layer lives in internal/approval; this package just
//     answers "inside or not".
package sandbox

import (
	"fmt"
	"os"
	"path/filepath"
)

// Resolve turns a possibly-relative path into an absolute, symlink-free
// canonical form. When path is empty it returns cwd (useful for bash tool
// with no explicit cwd). Leading `~` or `~/` is expanded to the user's
// home directory — Go's filepath.Abs doesn't do this, but LLMs and humans
// both habitually type tilde-paths.
func Resolve(path string) (string, error) {
	if path == "" {
		wd, err := os.Getwd()
		if err != nil {
			return "", err
		}
		return wd, nil
	}
	if path == "~" || (len(path) >= 2 && path[:2] == "~/") {
		home, err := os.UserHomeDir()
		if err != nil {
			return "", fmt.Errorf("resolve ~: %w", err)
		}
		if path == "~" {
			path = home
		} else {
			path = filepath.Join(home, path[2:])
		}
	}
	abs, err := filepath.Abs(path)
	if err != nil {
		return "", fmt.Errorf("resolve abs: %w", err)
	}
	abs = filepath.Clean(abs)

	// Resolve symlinks. EvalSymlinks fails on a path whose leaf doesn't
	// exist yet (the fs_write create case) — previously we skipped
	// resolution entirely there, so `<root>/link -> /etc` + `.../link/x`
	// wrote outside the sandbox with no prompt (A8). Fix: resolve the
	// LONGEST EXISTING ANCESTOR (which may itself be or contain a
	// symlink), then re-attach the missing tail. Containment is re-checked
	// by the caller AFTER this resolution, so a symlinked ancestor that
	// points out of the root is caught.
	if real, err := filepath.EvalSymlinks(abs); err == nil {
		return real, nil
	}
	existing := abs
	var tail []string
	for {
		parent := filepath.Dir(existing)
		if parent == existing {
			// Reached the root ("/" or a volume) without finding an
			// existing ancestor — nothing to resolve, return the clean abs.
			break
		}
		if _, statErr := os.Lstat(existing); statErr == nil {
			// `existing` exists — resolve its symlinks and rejoin the tail.
			if real, evErr := filepath.EvalSymlinks(existing); evErr == nil {
				parts := append([]string{real}, tail...)
				return filepath.Clean(filepath.Join(parts...)), nil
			}
			break
		}
		tail = append([]string{filepath.Base(existing)}, tail...)
		existing = parent
	}
	return abs, nil
}

// Contains reports whether path is inside root (exact or descendant).
func Contains(root, path string) bool {
	r, err := Resolve(root)
	if err != nil {
		return false
	}
	p, err := Resolve(path)
	if err != nil {
		return false
	}
	if r == p {
		return true
	}
	sep := string(os.PathSeparator)
	if len(p) > len(r) && p[:len(r)] == r && p[len(r):len(r)+1] == sep {
		return true
	}
	return false
}

// InsideAny returns true if path is inside at least one of roots.
func InsideAny(roots []string, path string) bool {
	for _, r := range roots {
		if Contains(r, path) {
			return true
		}
	}
	return false
}

// EnsureDir creates the directory if missing (0700). Used to materialize
// the default ~/SNTH/<slug>/ sandbox on first connect.
func EnsureDir(path string) error {
	return os.MkdirAll(path, 0o700)
}
