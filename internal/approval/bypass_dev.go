//go:build dev

package approval

import "os"

// bypassValue is evaluated at init in DEV builds only (built with
// `-tags dev`). SNTH_COMPANION_BYPASS_APPROVAL=1 auto-approves everything
// without consulting the trust store — for local development / CI only.
// Release builds get the constant-false version in bypass_release.go.
var bypassValue = os.Getenv("SNTH_COMPANION_BYPASS_APPROVAL") == "1"

func bypassActive() bool { return bypassValue }
