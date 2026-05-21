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
	// systray.Run has created the shared NSApplication by the time
	// onReady fires — set the Dock icon to the brand mark (matters for
	// the bare-binary dev deploy that has no .app bundle).
	SetBrandDockIcon()

	// Heart icon (SF Symbol, template PNG). Empty until the first status
	// refresh flips it to filled on a live connection. SetTemplateIcon
	// lets macOS tint it for light/dark menu bars automatically.
	systray.SetTemplateIcon(iconHeartEmpty, iconHeartEmpty)
	systray.SetTitle("")
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

	// --- Menu bar style submenu ---
	mStyle := systray.AddMenuItem("Menu bar style", "What shows next to the heart icon")
	mStyleHeart := mStyle.AddSubMenuItemCheckbox("Heart only", "Just the heart icon", false)
	mStyleSNTH := mStyle.AddSubMenuItemCheckbox("Heart + snth", "Heart icon + the word \"snth\"", false)
	mStyleName := mStyle.AddSubMenuItemCheckbox("Heart + agent name", "Heart icon + active synth's name", false)

	systray.AddSeparator()

	mQuit := systray.AddMenuItem("Quit", "Stop the companion")

	// --- Click handlers ---
	// URLs point at the React SPA (served at /ui/). HashRouter means
	// in-page routes look like /ui/#/pair etc. — safe to paste into
	// the WebView child window without worrying about server-side
	// fallback routes. "Open in Browser (debug)" also goes to the
	// new UI; the old server-rendered pages remain at their raw
	// paths (/, /pair, /channels...) for legacy access.
	spaURL := deps.UIURL + "/ui/"
	pairURL := deps.UIURL + "/ui/#/pair"
	go func() {
		for {
			select {
			case <-mOpen.ClickedCh:
				if err := openInWindow(spaURL, "SNTH Companion"); err != nil {
					log.Printf("open window: %v — falling back to browser", err)
					openURL(spaURL)
				}
			case <-mPairUI.ClickedCh:
				if err := openInWindow(pairURL, "Pair Synth"); err != nil {
					log.Printf("open window: %v — falling back to browser", err)
					openURL(pairURL)
				}
			case <-mCopyURL.ClickedCh:
				if err := copyToClipboard(spaURL); err != nil {
					log.Printf("clipboard: %v", err)
				}
			case <-mOpenBrowser.ClickedCh:
				openURL(spaURL)
			case <-mStyleHeart.ClickedCh:
				setMenubarDisplay(config.MenubarHeart)
			case <-mStyleSNTH.ClickedCh:
				setMenubarDisplay(config.MenubarHeartSNTH)
			case <-mStyleName.ClickedCh:
				setMenubarDisplay(config.MenubarHeartName)
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
			connected := st.Status == "connected"

			// Heart icon: filled when connected, empty otherwise.
			if connected {
				systray.SetTemplateIcon(iconHeartFill, iconHeartFill)
			} else {
				systray.SetTemplateIcon(iconHeartEmpty, iconHeartEmpty)
			}

			// Trailing text per the menubar_display setting.
			systray.SetTitle(menubarText(cfg))

			// Status dot is still used inside the dropdown header.
			dot := "○"
			switch st.Status {
			case "connected":
				dot = "●"
			case "connecting", "paused":
				dot = "◐"
			}
			mStatus.SetTitle(dot + " " + prettyStatus(st.Status))
			if cfg != nil && cfg.PairedSynthID != "" {
				mSynth.SetTitle("Paired synth: " + cfg.PairedSynthID)
			} else {
				mSynth.SetTitle("Paired synth: —  (use Pair…)")
			}

			// Reflect the active style choice as submenu checkmarks.
			mode := config.MenubarHeart
			if cfg != nil && cfg.MenubarDisplay != "" {
				mode = cfg.MenubarDisplay
			}
			setChecked(mStyleHeart, mode == config.MenubarHeart)
			setChecked(mStyleSNTH, mode == config.MenubarHeartSNTH)
			setChecked(mStyleName, mode == config.MenubarHeartName)

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

// menubarText returns the trailing text shown after the heart icon,
// per the menubar_display setting. Empty string = icon only.
func menubarText(cfg *config.Config) string {
	mode := config.MenubarHeart
	if cfg != nil && cfg.MenubarDisplay != "" {
		mode = cfg.MenubarDisplay
	}
	switch mode {
	case config.MenubarHeartSNTH:
		return "snth"
	case config.MenubarHeartName:
		return friendlySynthName(cfg)
	default:
		return ""
	}
}

// friendlySynthName resolves the active synth's human name. Prefers the
// pair's explicit Label; otherwise prettifies the instance id
// ("mia_snthai_bot" → "Mia"). Falls back to "snth" when unpaired.
func friendlySynthName(cfg *config.Config) string {
	if cfg == nil {
		return "snth"
	}
	if p := cfg.ActivePair(); p != nil {
		if p.Label != "" {
			return p.Label
		}
		if name := prettifySynthID(p.ID); name != "" {
			return name
		}
	}
	if name := prettifySynthID(cfg.PairedSynthID); name != "" {
		return name
	}
	return "snth"
}

// prettifySynthID turns a raw instance id into a display name. Instance
// ids look like "mia_snthai_bot" / "monica-andrew-test" — take the
// first token before the first separator and title-case it.
func prettifySynthID(id string) string {
	if id == "" {
		return ""
	}
	first := id
	for i, r := range id {
		if r == '_' || r == '-' {
			first = id[:i]
			break
		}
	}
	if first == "" {
		return ""
	}
	// Title-case the first rune.
	runes := []rune(first)
	if runes[0] >= 'a' && runes[0] <= 'z' {
		runes[0] -= 32
	}
	return string(runes)
}

// setMenubarDisplay persists a new menu-bar style choice. The refresh
// loop picks it up on its next tick (within 2s).
func setMenubarDisplay(mode string) {
	if err := config.Update(func(c *config.Config) {
		c.MenubarDisplay = mode
	}); err != nil {
		log.Printf("menubar: save display mode: %v", err)
	}
}

// setChecked toggles a systray checkbox menu item.
func setChecked(item *systray.MenuItem, on bool) {
	if on {
		item.Check()
	} else {
		item.Uncheck()
	}
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

