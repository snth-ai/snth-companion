//go:build !dev

package approval

import (
	"os"
	"testing"
)

// TestBypassCompiledOutInReleaseBuild proves A7: in a release build (no
// `dev` tag), the bypass is the constant false even when the env var is
// set. The synth cannot re-enable auto-approve-everything via a leaked
// plist / inherited env.
func TestBypassCompiledOutInReleaseBuild(t *testing.T) {
	t.Setenv("SNTH_COMPANION_BYPASS_APPROVAL", "1")
	// os.Getenv confirms the env is set — but the compiled-in value ignores it.
	if os.Getenv("SNTH_COMPANION_BYPASS_APPROVAL") != "1" {
		t.Fatal("test setup: env not set")
	}
	if BypassActive() {
		t.Fatal("release build must NOT honor SNTH_COMPANION_BYPASS_APPROVAL")
	}
	if bypass {
		t.Fatal("package-level bypass var must be false in release build")
	}
}
