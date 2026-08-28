package tools

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"

	"github.com/snth-ai/snth-companion/internal/config"
)

// remote_yt_dlp — the synth's transparent yt-dlp escape hatch.
//
// Hetzner datacenter IPs are blacklisted by YouTube; even residential
// proxy pools see ~50% bot-detection failure. Running yt-dlp locally
// on the user's Mac uses their actual residential IP and sidesteps
// the entire problem. dj / media_download / youtube_transcript on the
// synth side already prefer this path when a companion is online; the
// fallback to synth-side yt-dlp + proxy pool kicks in only when this
// handler is unreachable or errors.
//
// Args (from openpaw_server/main.go SetCompanionYtRunner closure):
//   { args: []string, combined: bool }
// Result:
//   { output: string, error: string }
//
// The synth caller already builds the full yt-dlp argv (including
// --audio-format, -o template, --cookies if available, etc). We just
// shell out and capture output. No approval prompt — the call is
// platform-driven background work, the user isn't expecting a dialog.

type ytDlpArgs struct {
	Args     []string `json:"args"`
	Combined bool     `json:"combined"`
}

type ytDlpResult struct {
	Output string `json:"output"`
	Error  string `json:"error,omitempty"`
}

const ytDlpTimeout = 5 * time.Minute

// RegisterYtDlp wires remote_yt_dlp into the tool registry.
//
// Reclassified safe → prompt (A1): yt-dlp natively supports --exec
// (arbitrary shell) and -o /abs/path (arbitrary write). The old handler
// only blocked --proxy, so a compromised synth had de-facto RCE. Now the
// argv is validated against an ALLOWLIST (sanitizeYtDlpArgs) AND every
// invocation prompts the user.
func RegisterYtDlp() {
	Register(Descriptor{
		Name:            "remote_yt_dlp",
		Description:     "Run yt-dlp on the paired Mac to download/inspect YouTube content using the user's residential IP. Synth-side dj / media_download / youtube_transcript route through this transparently.",
		DangerLevel:     "prompt",
		ApprovalSummary: ytDlpSummary,
	}, ytDlpHandler)
}

// ytDlpSummary renders the approval dialog text for remote_yt_dlp.
func ytDlpSummary(raw json.RawMessage) (string, string) {
	var a ytDlpArgs
	if err := json.Unmarshal(raw, &a); err != nil {
		return "", ""
	}
	joined := strings.Join(a.Args, " ")
	if len(joined) > 200 {
		joined = joined[:200] + "…"
	}
	return "Run yt-dlp on your Mac:\n    yt-dlp " + joined, ""
}

func ytDlpHandler(ctx context.Context, raw json.RawMessage) (any, error) {
	var args ytDlpArgs
	if err := json.Unmarshal(raw, &args); err != nil {
		return nil, fmt.Errorf("bad args: %w", err)
	}
	if len(args.Args) == 0 {
		return nil, errors.New("args is required")
	}

	safeArgs, err := sanitizeYtDlpArgs(args.Args)
	if err != nil {
		return nil, err
	}

	cctx, cancel := context.WithTimeout(ctx, ytDlpTimeout)
	defer cancel()

	cmd := exec.CommandContext(cctx, "yt-dlp", safeArgs...)
	cmd.Env = augmentPATH(os.Environ())

	var stdout, stderr bytes.Buffer
	cmd.Stdout = &stdout
	cmd.Stderr = &stderr

	runErr := cmd.Run()
	out := stdout.Bytes()
	if args.Combined {
		// Mirror exec.Cmd.CombinedOutput — stderr appended after stdout.
		out = append(out, stderr.Bytes()...)
	}

	if runErr != nil {
		errMsg := runErr.Error()
		if cctx.Err() == context.DeadlineExceeded {
			errMsg = fmt.Sprintf("timeout after %s", ytDlpTimeout)
		} else if strings.Contains(errMsg, "executable file not found") {
			errMsg = "yt-dlp is not installed on this Mac. Install via `brew install yt-dlp` (also requires ffmpeg for audio extraction: `brew install ffmpeg`)."
		} else if !args.Combined && stderr.Len() > 0 {
			// Stdout-only callers still want the error context; surface
			// stderr in the error field.
			errMsg = fmt.Sprintf("%s: %s", errMsg, strings.TrimSpace(stderr.String()))
		}
		return ytDlpResult{Output: string(out), Error: errMsg}, nil
	}

	return ytDlpResult{Output: string(out)}, nil
}

// --- argv allowlist (A1 / P0.2) ---------------------------------------------
//
// yt-dlp's flag surface includes remote-code-exec (--exec) and
// arbitrary-write (-o /abs) knobs. Rather than blocklist the dangerous
// ones (unbounded — new ones ship every release), we ALLOWLIST the exact
// read/inspect + controlled-download flags the synth actually emits
// (dj.go / media_download.go / youtube_transcript.go) plus their obvious
// aliases, and confine -o/--output to the companion download dir.

// ytDlpBoolFlags are allowlisted flags that take NO value.
var ytDlpBoolFlags = map[string]bool{
	"--write-auto-sub":        true,
	"--write-auto-subs":       true,
	"--write-sub":             true,
	"--write-subs":            true,
	"--skip-download":         true,
	"--no-playlist":           true,
	"--no-check-certificates": true,
	"--quiet":                 true,
	"--no-warnings":           true,
	"--get-title":             true,
	"--dump-json":             true,
	"--no-download":           true,
	"-x":                      true,
	"--extract-audio":         true,
}

// ytDlpValueFlags are allowlisted flags that consume the NEXT token as
// their value (space-separated form). The "=" form is handled too.
var ytDlpValueFlags = map[string]bool{
	"--sub-lang":            true,
	"--sub-langs":           true,
	"--convert-subs":        true,
	"-o":                    true,
	"--output":              true,
	"-f":                    true,
	"--format":              true,
	"--audio-format":        true,
	"--merge-output-format": true,
	"--cookies":             true,
}

// ytDlpRejectedFlags are explicitly refused even though the allowlist
// would already drop them — a defense-in-depth list so the error is
// specific and the intent is documented.
var ytDlpRejectedFlags = map[string]bool{
	"--exec":                 true,
	"--exec-before-download": true,
	"--paths":                true,
	"-P":                     true,
	"--batch-file":           true,
	"-a":                     true,
	"--load-info-json":       true,
	"--load-info":            true,
	"--postprocessor-args":   true,
	"--ppa":                  true,
	"--external-downloader":  true,
	"--downloader":           true,
	"--proxy":                true,
}

// sanitizeYtDlpArgs validates argv against the allowlist and returns a
// safe argv with -o/--output confined to the companion download dir. Any
// flag not in the allowlist (or any rejected flag) fails closed.
func sanitizeYtDlpArgs(in []string) ([]string, error) {
	dlDir := config.DownloadDir()
	if err := os.MkdirAll(dlDir, 0o700); err != nil {
		return nil, fmt.Errorf("prepare download dir: %w", err)
	}

	out := make([]string, 0, len(in))
	for i := 0; i < len(in); i++ {
		tok := in[i]

		// Split "--flag=value" into name/value for uniform handling.
		name := tok
		inlineVal := ""
		hasInline := false
		if strings.HasPrefix(tok, "-") {
			if eq := strings.IndexByte(tok, '='); eq >= 0 {
				name = tok[:eq]
				inlineVal = tok[eq+1:]
				hasInline = true
			}
		}

		// Positional (URL / ytsearch:...): anything not starting with '-'.
		if !strings.HasPrefix(tok, "-") {
			if err := validateYtDlpPositional(tok); err != nil {
				return nil, err
			}
			out = append(out, tok)
			continue
		}

		if ytDlpRejectedFlags[name] {
			return nil, fmt.Errorf("yt-dlp flag %q is not allowed via the companion", name)
		}

		if ytDlpBoolFlags[name] {
			if hasInline {
				return nil, fmt.Errorf("yt-dlp flag %q does not take a value", name)
			}
			out = append(out, name)
			continue
		}

		if ytDlpValueFlags[name] {
			var val string
			if hasInline {
				val = inlineVal
			} else {
				if i+1 >= len(in) {
					return nil, fmt.Errorf("yt-dlp flag %q requires a value", name)
				}
				i++
				val = in[i]
			}
			if name == "-o" || name == "--output" {
				confined, err := confineYtDlpOutput(val, dlDir)
				if err != nil {
					return nil, err
				}
				val = confined
			}
			if name == "--cookies" {
				confined, err := confineYtDlpCookies(val, dlDir)
				if err != nil {
					return nil, err
				}
				val = confined
			}
			out = append(out, name, val)
			continue
		}

		return nil, fmt.Errorf("yt-dlp flag %q is not on the companion allowlist", name)
	}
	return out, nil
}

// validateYtDlpPositional rejects a positional argument that looks like a
// local file path escape. URLs and ytsearch/scheme targets pass.
func validateYtDlpPositional(tok string) error {
	if strings.Contains(tok, "..") && (strings.Contains(tok, "/") || strings.Contains(tok, "\\")) {
		return fmt.Errorf("yt-dlp positional %q looks like a path escape", tok)
	}
	return nil
}

// confineYtDlpOutput rewrites an -o/--output template so it lands under
// the companion download dir. It keeps the caller's filename template
// (e.g. "%(id)s.%(ext)s") but forces the directory to dlDir. Templates
// containing ".." are rejected.
func confineYtDlpOutput(tmpl, dlDir string) (string, error) {
	if tmpl == "" {
		return "", fmt.Errorf("empty -o template")
	}
	if strings.Contains(tmpl, "..") {
		return "", fmt.Errorf("-o template %q must not contain '..'", tmpl)
	}
	// Take only the final path component (the filename template); discard
	// any directory the caller tried to steer to (absolute or relative).
	base := filepath.Base(tmpl)
	if base == "" || base == "." || base == string(filepath.Separator) {
		base = "%(title)s.%(ext)s"
	}
	return filepath.Join(dlDir, base), nil
}

// confineYtDlpCookies confines a --cookies value to the companion's managed
// download dir. yt-dlp opens the value as a Netscape cookie jar, so an
// unconfined value (e.g. /etc/passwd, ~/.ssh/known_hosts) would let a
// compromised synth make yt-dlp open ANY local file (F4). No exec/write/
// exfil results (yt-dlp doesn't echo the jar back), but we still close the
// residual arbitrary-open.
//
// NOTE on the login_site flow: the synth's cloud-cookie file is created
// on the SYNTH host (openpaw tools/cloud_cookies.go: os.CreateTemp("",
// "ytck-*.txt")) and its path is passed verbatim in the argv — that path
// does NOT exist on the companion's Mac, so the "--cookies <synth-tmp>"
// flow was already a no-op over the companion runner (yt-dlp can't open a
// file that isn't there). Confining to DownloadDir therefore breaks no
// working flow; a real companion-side cookie jar must live under the
// managed dir. `..` escapes are rejected exactly like -o/--output.
func confineYtDlpCookies(val, dlDir string) (string, error) {
	if val == "" {
		return "", fmt.Errorf("empty --cookies path")
	}
	if strings.Contains(val, "..") {
		return "", fmt.Errorf("--cookies path %q must not contain '..'", val)
	}
	// Already under the managed dir → keep verbatim.
	clean := filepath.Clean(val)
	if clean == dlDir || strings.HasPrefix(clean, dlDir+string(filepath.Separator)) {
		return clean, nil
	}
	// Anything else (an absolute path elsewhere, e.g. the synth-side
	// /synthtmp/cookies.txt that doesn't exist on the Mac, or a relative
	// path) is re-rooted under the managed dir by its final component,
	// exactly like -o/--output. This keeps the synth's login_site argv
	// flowing (fail-open, not a hard error that aborts the whole download)
	// while making it impossible for yt-dlp to open a file OUTSIDE the
	// managed dir as a cookie jar.
	base := filepath.Base(val)
	if base == "" || base == "." || base == string(filepath.Separator) {
		return "", fmt.Errorf("--cookies path %q is not a file", val)
	}
	return filepath.Join(dlDir, base), nil
}

// augmentPATH prepends common Mac binary locations to the inherited
// PATH so yt-dlp and ffmpeg are findable when the companion is started
// via launchd (which strips PATH down to /usr/bin:/bin:/usr/sbin:/sbin
// and ignores ~/.zshrc).
func augmentPATH(env []string) []string {
	const extra = "/opt/homebrew/bin:/usr/local/bin:/opt/miniconda3/bin"
	out := make([]string, 0, len(env))
	found := false
	for _, kv := range env {
		if strings.HasPrefix(kv, "PATH=") {
			out = append(out, "PATH="+extra+":"+kv[len("PATH="):])
			found = true
			continue
		}
		out = append(out, kv)
	}
	if !found {
		out = append(out, "PATH="+extra+":/usr/bin:/bin:/usr/sbin:/sbin")
	}
	return out
}
