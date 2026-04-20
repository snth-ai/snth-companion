// Command snth-companion is the local sidecar for a SNTH synth. It
// maintains a persistent WebSocket to the paired synth and proxies a small
// set of sandboxed local tools (bash, filesystem, Apple Shortcuts) so the
// synth can act on the user's Mac.
//
// Usage:
//
//	snth-companion              # runs with local UI on 127.0.0.1:<random>
//	snth-companion --headless   # same, no browser auto-open
//
// First run prints the UI URL to stderr. The user visits it once to pair
// the companion with their synth (Day-1 manual form; Day-4 TG flow).
package main

import (
	"flag"
	"fmt"
	"log"
	"os"
	"os/signal"
	"runtime"
	"syscall"

	"github.com/snth-ai/snth-companion/internal/config"
	"github.com/snth-ai/snth-companion/internal/daemon"
	"github.com/snth-ai/snth-companion/internal/tools"
)

func main() {
	headless := flag.Bool("headless", false, "Don't auto-open the UI in the default browser")
	printVersion := flag.Bool("version", false, "Print version and exit")
	flag.Parse()

	if *printVersion {
		fmt.Println("snth-companion", daemon.Version)
		return
	}

	log.SetFlags(log.LstdFlags | log.Lmicroseconds)
	log.SetPrefix("[companion] ")

	if _, err := config.Load(); err != nil {
		log.Fatalf("load config: %v", err)
	}
	log.Printf("config at %s", config.Path())

	// Register all tools. They're already in the catalog before the WS
	// client connects, so the hello frame advertises them correctly.
	tools.RegisterBash()
	tools.RegisterFS()
	tools.RegisterShortcut()
	tools.RegisterCalendar()
	tools.RegisterNotes()
	tools.RegisterClipboard()
	tools.RegisterNotify()
	tools.RegisterReminders()
	tools.RegisterContacts()
	tools.RegisterMessages()
	tools.RegisterBrowser()
	tools.RegisterFlights()

	client := &daemon.Client{}

	_, uiURL, err := daemon.StartUIServer(client)
	if err != nil {
		log.Fatalf("start ui server: %v", err)
	}

	release, err := daemon.AcquireLock(uiURL)
	if err != nil {
		log.Fatalf("lock: %v", err)
	}
	defer release()

	client.Start()
	log.Printf("UI at %s", uiURL)

	if *headless || runtime.GOOS != "darwin" {
		// Headless: no menubar, no auto-open. Just wait for a signal.
		fmt.Fprintf(os.Stderr, "\nOpen %s in your browser.\n\n", uiURL)
		sigCh := make(chan os.Signal, 1)
		signal.Notify(sigCh, syscall.SIGINT, syscall.SIGTERM)
		<-sigCh
		log.Printf("shutting down")
		client.Stop()
		return
	}

	// GUI mode: systray.Run owns the main goroutine. A signal handler
	// goroutine tears the menubar down cleanly on Ctrl-C so the lock
	// file gets released.
	sigCh := make(chan os.Signal, 1)
	signal.Notify(sigCh, syscall.SIGINT, syscall.SIGTERM)
	go func() {
		<-sigCh
		log.Printf("signal received, tearing down")
		systrayQuitHook()
	}()

	daemon.RunMenubar(daemon.MenubarDeps{Client: client, UIURL: uiURL})
	client.Stop()
	log.Printf("shutdown complete")
}

// systrayQuitHook is a thin redirect to systray.Quit that lets main
// avoid importing systray directly. Kept in a separate func so we can
// stub it in tests.
func systrayQuitHook() {
	daemon.QuitMenubar()
}
