//go:build cgo

package daemon

// window.go — native WebView window. Runs in a child process because
// systray.Run and webview both need to own the main thread on macOS
// (NSApplication singleton). The parent process (systray) execs itself
// with --window <url> to spawn an isolated window process.
//
// Requires CGO because webview_go wraps the platform WebKit/WebView2
// library via C bindings. Non-CGO builds fall back to the stub in
// window_nocgo.go (just opens the default browser).

import (
	"log"
	"runtime"

	webview "github.com/webview/webview_go"
)

// RunWindow opens a native WebView window loading the given URL and
// blocks until the user closes it. Must be called from the main
// goroutine on macOS.
func RunWindow(url, title string) {
	runtime.LockOSThread()
	w := webview.New(false)
	defer w.Destroy()
	w.SetTitle(title)
	w.SetSize(1100, 780, webview.HintNone)
	w.Navigate(url)
	log.Printf("webview opened: %s", url)
	w.Run()
}
