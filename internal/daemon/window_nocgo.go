//go:build !cgo

package daemon

// window_nocgo.go — CGO-off fallback for RunWindow. The real
// webview_go requires CGO + a platform C toolchain (WebKit on macOS,
// WebView2 on Windows, GTK+webkit2gtk on Linux). When building without
// CGO (cross-compile from Mac to Windows without mingw, for instance)
// we fall back to opening the URL in the default browser.

import (
	"log"
	"os/exec"
	"runtime"
)

func RunWindow(url, title string) {
	log.Printf("webview unavailable (CGO off): opening %q in default browser", url)
	var cmd *exec.Cmd
	switch runtime.GOOS {
	case "darwin":
		cmd = exec.Command("open", url)
	case "windows":
		cmd = exec.Command("rundll32", "url.dll,FileProtocolHandler", url)
	default:
		cmd = exec.Command("xdg-open", url)
	}
	if err := cmd.Start(); err != nil {
		log.Printf("open URL: %v", err)
	}
}
