//go:build !darwin

package daemon

// SetDockIcon is a no-op on non-macOS platforms.
func SetDockIcon(png []byte) {}

// SetBrandDockIcon is a no-op on non-macOS platforms.
func SetBrandDockIcon() {}
