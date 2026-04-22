package daemon

import (
	"fmt"
	"log"
	"os"
	"os/exec"
	"runtime"
	"time"

	"fyne.io/systray"

	"github.com/snth-ai/snth-companion/internal/config"
	"github.com/snth-ai/snth-companion/internal/tools"
)

// menubar.go — macOS menubar (and cross-platform systray) integration.
// We load this when the user runs `snth-companion` without --headless.
// systray.Run takes over the main thread and blocks until Quit, so we
// expect main() to call RunMenubar() as its final blocking step. All
// daemon work (WS client, HTTP UI) happens in goroutines started
// before RunMenubar.

type MenubarDeps struct {
	Client *Client
	UIURL  string
}

// RunMenubar blocks. Panics if called off the main goroutine on macOS
// because NSApplication requires main-thread dispatch.
func RunMenubar(deps MenubarDeps) {
	onReady := func() { buildMenu(deps) }
	onExit := func() { log.Printf("menubar exiting") }
	systray.Run(onReady, onExit)
}

// QuitMenubar tells systray to exit its event loop, letting RunMenubar
// return. Called from a signal handler goroutine.
func QuitMenubar() {
	systray.Quit()
}

func buildMenu(deps MenubarDeps) {
	systray.SetTitle("SNTH")
	systray.SetTooltip("SNTH Companion")

	// --- Status header (disabled, just for display) ---
	mStatus := systray.AddMenuItem("● Status…", "")
	mStatus.Disable()

	mVersion := systray.AddMenuItem("v"+Version, "Companion version")
	mVersion.Disable()

	systray.AddSeparator()

	// --- Primary actions ---
	mOpen := systray.AddMenuItem("Open Control Panel…", "Open the companion UI in a native window")
	mPairUI := systray.AddMenuItem("Pair…", "Open the pair form")
	mCopyURL := systray.AddMenuItem("Copy UI URL", "Copy the localhost UI URL to clipboard")
	mOpenBrowser := systray.AddMenuItem("Open in Browser (debug)", "Open the companion UI in the default browser")

	systray.AddSeparator()

	// --- Submenus (populated lazily) ---
	mTools := systray.AddMenuItem("Tools (0)", "Currently-registered tools advertised to the synth")
	mTools.Disable()

	mRoots := systray.AddMenuItem("Sandbox roots (0)", "Paths tools are allowed to touch without approval")
	mRoots.Disable()

	mSynth := systray.AddMenuItem("Paired synth: —", "The synth this companion is connected to")
	mSynth.Disable()

	systray.AddSeparator()

	mQuit := systray.AddMenuItem("Quit", "Stop the companion")

	// --- Click handlers ---
	go func() {
		for {
			select {
			case <-mOpen.ClickedCh:
				if err := openInWindow(deps.UIURL+"/", "SNTH Companion"); err != nil {
					log.Printf("open window: %v — falling back to browser", err)
					openURL(deps.UIURL + "/")
				}
			case <-mPairUI.ClickedCh:
				if err := openInWindow(deps.UIURL+"/pair", "Pair Synth"); err != nil {
					log.Printf("open window: %v — falling back to browser", err)
					openURL(deps.UIURL + "/pair")
				}
			case <-mCopyURL.ClickedCh:
				if err := copyToClipboard(deps.UIURL); err != nil {
					log.Printf("clipboard: %v", err)
				}
			case <-mOpenBrowser.ClickedCh:
				openURL(deps.UIURL + "/")
			case <-mQuit.ClickedCh:
				systray.Quit()
				return
			}
		}
	}()

	// --- Periodic status refresh ---
	go func() {
		t := time.NewTicker(2 * time.Second)
		defer t.Stop()
		refresh := func() {
			st := deps.Client.Status()
			cfg := config.Get()
			// Title dot: ● green / ○ red / ◐ yellow
			dot := "○"
			switch st.Status {
			case "connected":
				dot = "●"
			case "connecting", "paused":
				dot = "◐"
			}
			systray.SetTitle(dot + " SNTH")

			mStatus.SetTitle(dot + " " + prettyStatus(st.Status))
			if cfg != nil && cfg.PairedSynthID != "" {
				mSynth.SetTitle("Paired synth: " + cfg.PairedSynthID)
			} else {
				mSynth.SetTitle("Paired synth: —  (use Pair…)")
			}

			cat := tools.Catalog()
			mTools.SetTitle(fmt.Sprintf("Tools (%d)", len(cat)))

			roots := 0
			if cfg != nil {
				roots = len(cfg.SandboxRoots)
			}
			mRoots.SetTitle(fmt.Sprintf("Sandbox roots (%d)", roots))
		}
		refresh()
		for range t.C {
			refresh()
		}
	}()
}

func prettyStatus(s string) string {
	switch s {
	case "connected":
		return "Connected"
	case "connecting":
		return "Connecting…"
	case "paused":
		return "Not paired"
	case "disconnected":
		return "Disconnected"
	}
	return s
}

// openInWindow spawns the companion binary in --window mode as a child
// process. Each click creates a fresh window process; closing the
// window ends only that process, leaving the menubar running. We use a
// child process because systray and webview each need to own the main
// thread on macOS (NSApplication singleton).
func openInWindow(url, title string) error {
	exe, err := os.Executable()
	if err != nil {
		return err
	}
	cmd := exec.Command(exe, "--window", url, "--window-title", title)
	cmd.Stdout = os.Stdout
	cmd.Stderr = os.Stderr
	return cmd.Start()
}

// openURL opens the given URL using the OS default browser.
func openURL(url string) {
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

// copyToClipboard uses `pbcopy` on macOS, `clip` on Windows, `xclip`
// on Linux. If the tool is missing, logs and returns the error.
func copyToClipboard(s string) error {
	var cmd *exec.Cmd
	switch runtime.GOOS {
	case "darwin":
		cmd = exec.Command("pbcopy")
	case "windows":
		cmd = exec.Command("clip")
	default:
		cmd = exec.Command("xclip", "-selection", "clipboard")
	}
	stdin, err := cmd.StdinPipe()
	if err != nil {
		return err
	}
	if err := cmd.Start(); err != nil {
		return err
	}
	_, _ = fmt.Fprint(stdin, s)
	_ = stdin.Close()
	return cmd.Wait()
}

