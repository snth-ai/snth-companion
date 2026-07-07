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

	"github.com/snth-ai/snth-companion/internal/config"
	"github.com/snth-ai/snth-companion/internal/sandbox"
)

// RegisterFS wires remote_fs_read, remote_fs_write, remote_fs_list into the
// tool registry. They share a single package so args + sandbox logic stay
// in one place.
func RegisterFS() {
	Register(Descriptor{
		Name:            "remote_fs_read",
		Description:     "Read a file on the paired Mac. Binary files are returned base64-encoded.",
		DangerLevel:     "prompt",
		GatePolicy:      fsReadGatePolicy,
		ApprovalSummary: fsReadSummary,
	}, fsReadHandler)
	Register(Descriptor{
		Name:            "remote_fs_write",
		Description:     "Write a file on the paired Mac. Creates parent dirs if missing. Overwrites existing files.",
		DangerLevel:     "prompt",
		GatePolicy:      fsWriteGatePolicy,
		ApprovalSummary: fsWriteSummary,
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
	// Optional range read. When Length > 0, returns up to Length bytes
	// starting at Offset; clamped to fsMaxReadBytes per call. Added
	// 2026-05-26 so the synth's companion_copy_to_workspace can stitch
	// >2 MiB files (Instagram reels, podcasts) by issuing N range reads.
	// Old clients omit both → behavior identical to the legacy single
	// 2 MiB-cap read. Result.Size is always the FULL file size; the
	// caller compares len(decoded) vs Size to know whether to chunk on.
	Offset int64 `json:"offset,omitempty"`
	Length int64 `json:"length,omitempty"`
}

type fsReadResult struct {
	Path      string    `json:"path"`
	Content   string    `json:"content"`
	Encoding  string    `json:"encoding"` // "utf-8" | "base64"
	Size      int64     `json:"size"`     // total file size, not bytes returned
	Mtime     time.Time `json:"mtime"`
	Truncated bool      `json:"truncated,omitempty"`
	// Range-read echo so the caller doesn't have to track per-request
	// state — set whenever a non-default range was honored.
	Offset int64 `json:"offset,omitempty"`
	Bytes  int   `json:"bytes,omitempty"` // bytes actually returned in Content (post-decode)
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

	// Range path: caller asked for a specific window. Clamp length to
	// fsMaxReadBytes so the WS frame stays healthy. truncated=false in
	// this path — it's an explicit slice, not silent truncation.
	if a.Length > 0 {
		if a.Offset < 0 {
			return nil, fmt.Errorf("offset must be >= 0, got %d", a.Offset)
		}
		if a.Offset > size {
			return nil, fmt.Errorf("offset %d past file size %d", a.Offset, size)
		}
		length := a.Length
		if length > fsMaxReadBytes {
			length = fsMaxReadBytes
		}
		if a.Offset+length > size {
			length = size - a.Offset
		}
		if _, err := f.Seek(a.Offset, 0); err != nil {
			return nil, fmt.Errorf("seek: %w", err)
		}
		buf := make([]byte, length)
		n, err := f.Read(buf)
		if err != nil && err.Error() != "EOF" {
			return nil, fmt.Errorf("read: %w", err)
		}
		buf = buf[:n]
		result := fsReadResult{
			Path:   p,
			Size:   size,
			Mtime:  info.ModTime(),
			Offset: a.Offset,
			Bytes:  n,
		}
		// Range reads are almost always binary slices; only call them
		// utf-8 if the slice itself is valid and starts/ends at byte
		// boundaries. Cheapest correct rule: utf-8 only at offset 0
		// AND the slice is the whole file AND valid utf-8.
		if a.Offset == 0 && int64(n) == size && utf8.Valid(buf) {
			result.Encoding = "utf-8"
			result.Content = string(buf)
		} else {
			result.Encoding = "base64"
			result.Content = base64.StdEncoding.EncodeToString(buf)
		}
		return result, nil
	}

	// Legacy single-shot path — unchanged: read up to 2 MiB, mark
	// truncated when the file is bigger. Old synths rely on this.
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
		Bytes:     n,
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

// resolveAndCheck normalizes + resolves path and enforces sandbox
// containment. Approval for out-of-sandbox access is handled by the
// central Dispatch gate (fsReadGatePolicy / fsWriteGatePolicy), so this
// helper now only resolves the path — by the time a handler runs, the
// gate has already approved (or the call was inside the sandbox). The
// `action` / `write` args are retained for call-site clarity but no
// longer drive a prompt here.
func resolveAndCheck(ctx context.Context, path, action string, write bool) (string, error) {
	if path == "" {
		return "", fmt.Errorf("path is required")
	}
	resolved, err := sandbox.Resolve(path)
	if err != nil {
		return "", fmt.Errorf("resolve: %w", err)
	}
	return resolved, nil
}

// fsResolveInside resolves a path and reports whether it lies inside a
// sandbox root. Shared by the fs gate policies + summaries.
func fsResolveInside(path string) (resolved string, inside bool, err error) {
	if path == "" {
		return "", false, fmt.Errorf("path is required")
	}
	resolved, err = sandbox.Resolve(path)
	if err != nil {
		return "", false, err
	}
	cfg := config.Get()
	return resolved, sandbox.InsideAny(cfg.SandboxRoots, resolved), nil
}

// fsGatePolicy is the shared gate decision for fs_read/fs_write: inside
// the sandbox → GateSkip, outside → GateAlwaysPrompt. Fails closed
// (GateAlwaysPrompt) on bad args or a resolve error.
func fsGatePolicy(raw json.RawMessage) GateDecision {
	var probe struct {
		Path string `json:"path"`
	}
	if err := json.Unmarshal(raw, &probe); err != nil {
		return GateAlwaysPrompt
	}
	_, inside, err := fsResolveInside(probe.Path)
	if err != nil {
		return GateAlwaysPrompt
	}
	if inside {
		return GateSkip
	}
	return GateAlwaysPrompt
}

func fsReadGatePolicy(raw json.RawMessage) GateDecision  { return fsGatePolicy(raw) }
func fsWriteGatePolicy(raw json.RawMessage) GateDecision { return fsGatePolicy(raw) }

// fsReadSummary renders the approval dialog text for an out-of-sandbox
// remote_fs_read. Returns the resolved path so the trust store can gate
// on AllowedWriteRoots.
func fsReadSummary(raw json.RawMessage) (string, string) {
	var a fsReadArgs
	if err := json.Unmarshal(raw, &a); err != nil {
		return "", ""
	}
	resolved, _, err := fsResolveInside(a.Path)
	if err != nil {
		resolved = a.Path
	}
	return fmt.Sprintf("Read file outside sandbox:\n    %s", resolved), resolved
}

// fsWriteSummary renders the approval dialog text for an out-of-sandbox
// remote_fs_write, warning that the target is writable.
func fsWriteSummary(raw json.RawMessage) (string, string) {
	var a fsWriteArgs
	if err := json.Unmarshal(raw, &a); err != nil {
		return "", ""
	}
	resolved, _, err := fsResolveInside(a.Path)
	if err != nil {
		resolved = a.Path
	}
	n := len(a.Content)
	if a.Encoding == "base64" {
		if b, derr := base64.StdEncoding.DecodeString(a.Content); derr == nil {
			n = len(b)
		}
	}
	return fmt.Sprintf("Write %d bytes to file OUTSIDE SANDBOX (writable):\n    %s", n, resolved), resolved
}
