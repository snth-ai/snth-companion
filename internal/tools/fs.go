package tools

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"sort"
	"time"
	"unicode/utf8"

	"github.com/snth-ai/snth-companion/internal/approval"
	"github.com/snth-ai/snth-companion/internal/config"
	"github.com/snth-ai/snth-companion/internal/sandbox"
)

// RegisterFS wires remote_fs_read, remote_fs_write, remote_fs_list into the
// tool registry. They share a single package so args + sandbox logic stay
// in one place.
func RegisterFS() {
	Register(Descriptor{
		Name:        "remote_fs_read",
		Description: "Read a file on the paired Mac. Binary files are returned base64-encoded.",
		DangerLevel: "prompt",
	}, fsReadHandler)
	Register(Descriptor{
		Name:        "remote_fs_write",
		Description: "Write a file on the paired Mac. Creates parent dirs if missing. Overwrites existing files.",
		DangerLevel: "prompt",
	}, fsWriteHandler)
	Register(Descriptor{
		Name:        "remote_fs_list",
		Description: "List the entries of a directory on the paired Mac.",
		DangerLevel: "safe",
	}, fsListHandler)
}

const (
	fsMaxReadBytes  = 2 * 1024 * 1024 // 2 MiB — bigger reads require chunking
	fsMaxWriteBytes = 4 * 1024 * 1024 // 4 MiB
)

// --- fs_read ----------------------------------------------------------------

type fsReadArgs struct {
	Path string `json:"path"`
}

type fsReadResult struct {
	Path     string `json:"path"`
	Content  string `json:"content"`
	Encoding string `json:"encoding"` // "utf-8" | "base64"
	Size     int64  `json:"size"`
	Mtime    time.Time `json:"mtime"`
	Truncated bool  `json:"truncated,omitempty"`
}

func fsReadHandler(ctx context.Context, raw json.RawMessage) (any, error) {
	var a fsReadArgs
	if err := json.Unmarshal(raw, &a); err != nil {
		return nil, fmt.Errorf("bad args: %w", err)
	}
	p, err := resolveAndCheck(ctx, a.Path, "Read file", false)
	if err != nil {
		return nil, err
	}
	info, err := os.Lstat(p)
	if err != nil {
		return nil, fmt.Errorf("stat: %w", err)
	}
	if info.IsDir() {
		return nil, fmt.Errorf("%s is a directory, use remote_fs_list", p)
	}

	f, err := os.Open(p)
	if err != nil {
		return nil, fmt.Errorf("open: %w", err)
	}
	defer f.Close()

	size := info.Size()
	readCap := size
	truncated := false
	if readCap > fsMaxReadBytes {
		readCap = fsMaxReadBytes
		truncated = true
	}
	buf := make([]byte, readCap)
	n, err := f.Read(buf)
	if err != nil && err.Error() != "EOF" {
		return nil, fmt.Errorf("read: %w", err)
	}
	buf = buf[:n]

	result := fsReadResult{
		Path:      p,
		Size:      size,
		Mtime:     info.ModTime(),
		Truncated: truncated,
	}
	if utf8.Valid(buf) {
		result.Encoding = "utf-8"
		result.Content = string(buf)
	} else {
		result.Encoding = "base64"
		result.Content = base64.StdEncoding.EncodeToString(buf)
	}
	return result, nil
}

// --- fs_write ---------------------------------------------------------------

type fsWriteArgs struct {
	Path     string `json:"path"`
	Content  string `json:"content"`
	Encoding string `json:"encoding,omitempty"` // "utf-8" (default) | "base64"
	Mode     int    `json:"mode,omitempty"`     // 0644 default
}

type fsWriteResult struct {
	Path    string `json:"path"`
	Bytes   int    `json:"bytes"`
	Created bool   `json:"created"` // true if the file didn't exist before
}

func fsWriteHandler(ctx context.Context, raw json.RawMessage) (any, error) {
	var a fsWriteArgs
	if err := json.Unmarshal(raw, &a); err != nil {
		return nil, fmt.Errorf("bad args: %w", err)
	}

	data := []byte(a.Content)
	if a.Encoding == "base64" {
		b, err := base64.StdEncoding.DecodeString(a.Content)
		if err != nil {
			return nil, fmt.Errorf("decode base64: %w", err)
		}
		data = b
	}
	if len(data) > fsMaxWriteBytes {
		return nil, fmt.Errorf("content too large: %d bytes (max %d)", len(data), fsMaxWriteBytes)
	}

	mode := fs.FileMode(0o644)
	if a.Mode != 0 {
		mode = fs.FileMode(a.Mode) & 0o777
	}

	p, err := resolveAndCheck(ctx, a.Path, fmt.Sprintf("Write %d bytes to file", len(data)), true)
	if err != nil {
		return nil, err
	}

	_, statErr := os.Lstat(p)
	created := os.IsNotExist(statErr)

	if err := os.MkdirAll(filepath.Dir(p), 0o755); err != nil {
		return nil, fmt.Errorf("mkdir: %w", err)
	}
	// Atomic write: temp + rename. Protects from partial writes on crash.
	tmp, err := os.CreateTemp(filepath.Dir(p), ".snth-companion-*")
	if err != nil {
		return nil, fmt.Errorf("tmp: %w", err)
	}
	if _, err := tmp.Write(data); err != nil {
		tmp.Close()
		os.Remove(tmp.Name())
		return nil, fmt.Errorf("write: %w", err)
	}
	if err := tmp.Chmod(mode); err != nil {
		tmp.Close()
		os.Remove(tmp.Name())
		return nil, fmt.Errorf("chmod: %w", err)
	}
	if err := tmp.Close(); err != nil {
		os.Remove(tmp.Name())
		return nil, fmt.Errorf("close: %w", err)
	}
	if err := os.Rename(tmp.Name(), p); err != nil {
		os.Remove(tmp.Name())
		return nil, fmt.Errorf("rename: %w", err)
	}
	return fsWriteResult{Path: p, Bytes: len(data), Created: created}, nil
}

// --- fs_list ----------------------------------------------------------------

type fsListArgs struct {
	Path string `json:"path"`
}

type fsEntry struct {
	Name  string    `json:"name"`
	Type  string    `json:"type"` // "file" | "dir" | "symlink" | "other"
	Size  int64     `json:"size"`
	Mtime time.Time `json:"mtime"`
}

type fsListResult struct {
	Path    string    `json:"path"`
	Entries []fsEntry `json:"entries"`
}

func fsListHandler(ctx context.Context, raw json.RawMessage) (any, error) {
	var a fsListArgs
	if err := json.Unmarshal(raw, &a); err != nil {
		return nil, fmt.Errorf("bad args: %w", err)
	}
	p, err := resolveAndCheck(ctx, a.Path, "List directory", false)
	if err != nil {
		return nil, err
	}
	info, err := os.Lstat(p)
	if err != nil {
		return nil, fmt.Errorf("stat: %w", err)
	}
	if !info.IsDir() {
		return nil, fmt.Errorf("%s is not a directory", p)
	}
	ents, err := os.ReadDir(p)
	if err != nil {
		return nil, fmt.Errorf("readdir: %w", err)
	}
	out := make([]fsEntry, 0, len(ents))
	for _, e := range ents {
		fi, err := e.Info()
		if err != nil {
			continue
		}
		kind := "other"
		switch {
		case fi.Mode()&os.ModeSymlink != 0:
			kind = "symlink"
		case fi.IsDir():
			kind = "dir"
		case fi.Mode().IsRegular():
			kind = "file"
		}
		out = append(out, fsEntry{
			Name:  e.Name(),
			Type:  kind,
			Size:  fi.Size(),
			Mtime: fi.ModTime(),
		})
	}
	sort.Slice(out, func(i, j int) bool { return out[i].Name < out[j].Name })
	return fsListResult{Path: p, Entries: out}, nil
}

// --- shared ------------------------------------------------------------------

// resolveAndCheck normalizes path, enforces sandbox containment, and on
// out-of-sandbox access pops the approval dialog. "write" = whether the
// dialog copy should warn about a write (vs a read/list).
func resolveAndCheck(ctx context.Context, path, action string, write bool) (string, error) {
	if path == "" {
		return "", fmt.Errorf("path is required")
	}
	cfg := config.Get()
	resolved, err := sandbox.Resolve(path)
	if err != nil {
		return "", fmt.Errorf("resolve: %w", err)
	}
	if sandbox.InsideAny(cfg.SandboxRoots, resolved) {
		return resolved, nil
	}
	danger := "always-prompt"
	summary := fmt.Sprintf("%s outside sandbox:\n    %s", action, resolved)
	if write {
		summary = fmt.Sprintf("%s OUTSIDE SANDBOX (writable):\n    %s", action, resolved)
	}
	tool := "remote_fs_read"
	if write {
		tool = "remote_fs_write"
	}
	ok, err := approval.Request(ctx, approval.Request_{
		Tool:    tool,
		Summary: summary,
		Danger:  danger,
		Path:    resolved,
	})
	if err != nil {
		return "", fmt.Errorf("approval: %w", err)
	}
	if !ok {
		return "", fmt.Errorf("user denied")
	}
	return resolved, nil
}
