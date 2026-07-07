package tools

// F4 regression: sanitizeYtDlpArgs allowed --cookies <path> with an
// unconfined arbitrary value, so yt-dlp would open ANY local file as a
// cookie jar (e.g. /etc/passwd, ~/.ssh/known_hosts). The fix confines
// --cookies to the companion's managed download dir, mirroring -o/--output,
// and rejects `..` escapes.
//
// These exercise the REAL sanitizeYtDlpArgs — the same function ytDlpHandler
// runs on every companion yt-dlp call.

import (
	"path/filepath"
	"strings"
	"testing"

	"github.com/snth-ai/snth-companion/internal/config"
)

// TestYtDlpCookiesArbitraryAbsPathConfined is the F4 regression: an absolute
// --cookies value outside the managed dir must be rejected (not passed
// through to yt-dlp verbatim).
//
// RED PROOF: remove the `if name == "--cookies"` confinement branch in
// sanitizeYtDlpArgs — then "/etc/passwd" passes through unchanged and this
// fails.
func TestYtDlpCookiesArbitraryAbsPathConfined(t *testing.T) {
	config.Load()
	dl := config.DownloadDir()

	for _, evil := range []string{"/etc/passwd", "/Users/x/.ssh/known_hosts", "/var/db/anything"} {
		out, err := sanitizeYtDlpArgs([]string{"https://youtu.be/x", "--cookies", evil})
		if err == nil {
			// If not rejected outright, it MUST at least be rewritten under
			// the managed dir — never left pointing at the arbitrary file.
			joined := strings.Join(out, " ")
			if strings.Contains(joined, evil) {
				t.Fatalf("F4 REGRESSION: --cookies %q passed through unconfined (argv=%q)", evil, joined)
			}
			if !cookieValUnderDir(out, dl) {
				t.Fatalf("F4: --cookies %q not confined under %q (argv=%q)", evil, dl, joined)
			}
			continue
		}
		// Rejection is the expected + preferred outcome for an abs path
		// outside the managed dir.
		if !strings.Contains(err.Error(), "cookies") {
			t.Fatalf("F4: unexpected error for --cookies %q: %v", evil, err)
		}
	}
}

// TestYtDlpCookiesDotDotRejected: `..` escapes are rejected exactly like
// -o/--output.
func TestYtDlpCookiesDotDotRejected(t *testing.T) {
	config.Load()
	_, err := sanitizeYtDlpArgs([]string{"https://youtu.be/x", "--cookies", "../../etc/passwd"})
	if err == nil {
		t.Fatalf("F4: --cookies with '..' must be rejected")
	}
	if !strings.Contains(err.Error(), "..") {
		t.Fatalf("F4: expected a '..' rejection error, got %v", err)
	}
}

// TestYtDlpCookiesUnderManagedDirAllowed: a --cookies value already inside
// the managed download dir is accepted (the only place a real
// companion-side cookie jar may live).
func TestYtDlpCookiesUnderManagedDirAllowed(t *testing.T) {
	config.Load()
	dl := config.DownloadDir()
	good := filepath.Join(dl, "yt-cookies.txt")
	out, err := sanitizeYtDlpArgs([]string{"https://youtu.be/x", "--cookies", good})
	if err != nil {
		t.Fatalf("F4: managed-dir cookie path rejected: %v", err)
	}
	if !cookieValUnderDir(out, dl) {
		t.Fatalf("F4: managed-dir cookie path not preserved under %q: argv=%q", dl, strings.Join(out, " "))
	}
}

// cookieValUnderDir finds the value following --cookies in argv and reports
// whether it lies under dir.
func cookieValUnderDir(argv []string, dir string) bool {
	for i := 0; i < len(argv)-1; i++ {
		if argv[i] == "--cookies" {
			v := filepath.Clean(argv[i+1])
			return v == dir || strings.HasPrefix(v, dir+string(filepath.Separator))
		}
	}
	return false
}
