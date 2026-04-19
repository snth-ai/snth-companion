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
// with no explicit cwd).
func Resolve(path string) (string, error) {
	if path == "" {
		wd, err := os.Getwd()
		if err != nil {
			return "", err
		}
		return wd, nil
	}
	abs, err := filepath.Abs(path)
	if err != nil {
		return "", fmt.Errorf("resolve abs: %w", err)
	}
	// EvalSymlinks would be ideal but fails on paths that don't exist yet
	// (e.g. fs_write to a new file). We fall back to Clean which at least
	// removes "..".
	if _, err := os.Lstat(abs); err == nil {
		if real, err := filepath.EvalSymlinks(abs); err == nil {
			abs = real
		}
	}
	return filepath.Clean(abs), nil
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
