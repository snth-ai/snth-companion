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
	"os/exec"
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

	client := &daemon.Client{}
	client.Start()

	_, uiURL, err := daemon.StartUIServer(client)
	if err != nil {
		log.Fatalf("start ui server: %v", err)
	}
	log.Printf("UI at %s", uiURL)

	if !*headless && runtime.GOOS == "darwin" {
		// Nudge the user: open the UI in their default browser on first
		// run. They'll use it to enter pair credentials.
		go func() {
			cmd := exec.Command("open", uiURL)
			if err := cmd.Start(); err != nil {
				log.Printf("open browser: %v (visit %s manually)", err, uiURL)
			}
		}()
	} else {
		fmt.Fprintf(os.Stderr, "\nOpen %s in your browser.\n\n", uiURL)
	}

	// Wait for SIGINT/SIGTERM.
	sigCh := make(chan os.Signal, 1)
	signal.Notify(sigCh, syscall.SIGINT, syscall.SIGTERM)
	<-sigCh
	log.Printf("shutting down")
	client.Stop()
}
