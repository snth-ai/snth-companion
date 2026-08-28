package tools

import (
	"strings"
	"testing"

	"github.com/snth-ai/snth-companion/internal/config"
)

// realSynthArgv mirrors the supersets emitted by the synth side
// (openpaw tools/{dj,media_download,youtube_transcript}.go) — the
// allowlist MUST accept every one of them.
func realSynthArgvs() [][]string {
	return [][]string{
		// youtube_transcript subtitles
		{"--write-auto-sub", "--write-sub", "--sub-lang", "en,ru", "--skip-download", "--convert-subs", "srt", "-o", "/synthtmp/%(id)s", "https://youtu.be/abc"},
		// youtube_transcript title
		{"--get-title", "--no-warnings", "https://youtu.be/abc"},
		// dj audio
		{"ytsearch1:some song", "-f", "bestaudio", "-x", "--audio-format", "mp3", "-o", "/synthtmp/x.%(ext)s", "--no-playlist", "--no-check-certificates", "--quiet", "--no-warnings"},
		// media_download metadata
		{"https://youtu.be/abc", "--dump-json", "--no-download", "--no-playlist", "--no-check-certificates", "--quiet"},
		// media_download download
		{"https://youtu.be/abc", "-o", "/synthtmp/%(id)s.%(ext)s", "-f", "b[ext=mp4]", "--merge-output-format", "mp4", "--no-playlist", "--no-check-certificates", "--quiet", "--no-warnings"},
		// with cookies
		{"--get-title", "https://youtu.be/abc", "--cookies", "/synthtmp/cookies.txt"},
	}
}

func TestSanitizeYtDlpAcceptsRealSynthArgv(t *testing.T) {
	for i, argv := range realSynthArgvs() {
		got, err := sanitizeYtDlpArgs(argv)
		if err != nil {
			t.Fatalf("case %d: real synth argv rejected: %v\n  argv=%v", i, err, argv)
		}
		// -o must have been re-rooted under the download dir.
		dl := config.DownloadDir()
		for j, tok := range got {
			if (tok == "-o" || tok == "--output") && j+1 < len(got) {
				if !strings.HasPrefix(got[j+1], dl) {
					t.Fatalf("case %d: -o not confined to download dir: %q (dl=%q)", i, got[j+1], dl)
				}
			}
		}
	}
}

func TestSanitizeYtDlpRejectsExec(t *testing.T) {
	_, err := sanitizeYtDlpArgs([]string{"--exec", "touch /tmp/pwned", "https://youtu.be/abc"})
	if err == nil {
		t.Fatal("--exec must be rejected")
	}
	if !strings.Contains(err.Error(), "--exec") {
		t.Fatalf("error should name --exec, got %v", err)
	}
}

func TestSanitizeYtDlpRejectsExecEqualsForm(t *testing.T) {
	if _, err := sanitizeYtDlpArgs([]string{"--exec=touch /tmp/x", "https://youtu.be/abc"}); err == nil {
		t.Fatal("--exec=... must be rejected")
	}
	if _, err := sanitizeYtDlpArgs([]string{"--exec-before-download", "cmd", "url"}); err == nil {
		t.Fatal("--exec-before-download must be rejected")
	}
}

func TestSanitizeYtDlpConfinesAbsoluteOutput(t *testing.T) {
	got, err := sanitizeYtDlpArgs([]string{"-o", "/etc/cron.d/pwn", "https://youtu.be/abc"})
	if err != nil {
		t.Fatalf("absolute -o should be confined, not error: %v", err)
	}
	dl := config.DownloadDir()
	for i, tok := range got {
		if tok == "-o" && i+1 < len(got) {
			if !strings.HasPrefix(got[i+1], dl) {
				t.Fatalf("-o /etc/... not confined: %q", got[i+1])
			}
			if strings.Contains(got[i+1], "cron.d") {
				t.Fatalf("attacker directory leaked into confined path: %q", got[i+1])
			}
		}
	}
}

func TestSanitizeYtDlpRejectsOutputDotDot(t *testing.T) {
	if _, err := sanitizeYtDlpArgs([]string{"-o", "../../etc/x", "url"}); err == nil {
		t.Fatal("-o with .. must be rejected")
	}
}

func TestSanitizeYtDlpRejectsUnknownFlag(t *testing.T) {
	if _, err := sanitizeYtDlpArgs([]string{"--paths", "/tmp", "url"}); err == nil {
		t.Fatal("--paths must be rejected")
	}
	if _, err := sanitizeYtDlpArgs([]string{"--batch-file", "/etc/passwd", "url"}); err == nil {
		t.Fatal("--batch-file must be rejected")
	}
	if _, err := sanitizeYtDlpArgs([]string{"--load-info-json", "x.json", "url"}); err == nil {
		t.Fatal("--load-info-json must be rejected")
	}
	if _, err := sanitizeYtDlpArgs([]string{"--external-downloader", "aria2c", "url"}); err == nil {
		t.Fatal("--external-downloader must be rejected")
	}
}
