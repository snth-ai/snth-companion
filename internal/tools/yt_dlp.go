package tools

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"strings"
	"time"
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
func RegisterYtDlp() {
	Register(Descriptor{
		Name:        "remote_yt_dlp",
		Description: "Run yt-dlp on the paired Mac to download/inspect YouTube content using the user's residential IP. Synth-side dj / media_download / youtube_transcript route through this transparently.",
		DangerLevel: "safe",
	}, ytDlpHandler)
}

func ytDlpHandler(ctx context.Context, raw json.RawMessage) (any, error) {
	var args ytDlpArgs
	if err := json.Unmarshal(raw, &args); err != nil {
		return nil, fmt.Errorf("bad args: %w", err)
	}
	if len(args.Args) == 0 {
		return nil, errors.New("args is required")
	}
	for _, a := range args.Args {
		if a == "--proxy" || strings.HasPrefix(a, "--proxy=") {
			// Synth side promised it wouldn't pass --proxy when routing
			// via companion. Belt-and-suspenders check so a stale build
			// doesn't quietly burn a residential proxy slot.
			return nil, errors.New("companion path forbids --proxy in yt-dlp args")
		}
	}

	cctx, cancel := context.WithTimeout(ctx, ytDlpTimeout)
	defer cancel()

	cmd := exec.CommandContext(cctx, "yt-dlp", args.Args...)
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
