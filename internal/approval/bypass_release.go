//go:build !dev

package approval

// bypassValue is a COMPILE-TIME constant false in release builds. The
// SNTH_COMPANION_BYPASS_APPROVAL env var is therefore inert in any binary
// built without the `dev` tag — it cannot re-enable the escape hatch even
// if it leaks into a shipped plist or inherited environment (A7). To use
// the bypass locally, build with `-tags dev`.
const bypassValue = false

// bypassActive reports whether the compiled-in bypass is on. Always false
// in release builds; used for the boot-time warning.
func bypassActive() bool { return bypassValue }
