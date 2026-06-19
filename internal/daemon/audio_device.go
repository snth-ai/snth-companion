package daemon

// audio_device.go — macOS system audio-output routing for the Chrome-join
// call flow. The listen connector (listen_connector.go) captures the
// BlackHole 2ch INPUT; for that input to carry the call we must force the
// system OUTPUT to a BlackHole-reaching device while Mia is on a call.
//
// Two output targets, picked by whether a human is watching:
//   - "Multi-Output Device" — fans audio to BlackHole AND the speakers, and
//     is immune to Screen Sharing's default-output hijack. Used when
//     screensharingd is up (Sasha is viewing the Air) so he still hears it.
//   - "BlackHole 2ch" — plain loopback, silent on the speakers. Used when
//     nobody is screen-sharing (autonomous Mia).
//
// Proven live 2026-06-19: Multi-Output -> BlackHole = ~-24 dB into the
// capture; a plain "BlackHole 2ch" default is hijacked to -91 dB silence
// while Screen Sharing is active. See project_realtime_synth memory.

import (
	"context"
	"fmt"
	"os/exec"
	"strings"
	"time"
)

const (
	outputMultiOutput = "Multi-Output Device"
	outputBlackHole   = "BlackHole 2ch"
)

// switchAudioSourceBin resolves the SwitchAudioSource CLI. launchd agents
// inherit only /usr/bin:/bin, so the brew path is checked explicitly first.
func switchAudioSourceBin() (string, error) {
	for _, c := range []string{"/opt/homebrew/bin/SwitchAudioSource", "/usr/local/bin/SwitchAudioSource"} {
		if _, err := exec.LookPath(c); err == nil {
			return c, nil
		}
	}
	if p, err := exec.LookPath("SwitchAudioSource"); err == nil {
		return p, nil
	}
	return "", fmt.Errorf("SwitchAudioSource not found (brew install switchaudio-osx)")
}

// currentAudioOutput returns the current default output device name.
func currentAudioOutput(ctx context.Context) (string, error) {
	bin, err := switchAudioSourceBin()
	if err != nil {
		return "", err
	}
	cctx, cancel := context.WithTimeout(ctx, 5*time.Second)
	defer cancel()
	out, err := exec.CommandContext(cctx, bin, "-c", "-t", "output").Output()
	if err != nil {
		return "", fmt.Errorf("read output device: %w", err)
	}
	return strings.TrimSpace(string(out)), nil
}

// switchAudioOutput sets the system default output device by name.
func switchAudioOutput(ctx context.Context, device string) error {
	bin, err := switchAudioSourceBin()
	if err != nil {
		return err
	}
	cctx, cancel := context.WithTimeout(ctx, 5*time.Second)
	defer cancel()
	cmd := exec.CommandContext(cctx, bin, "-s", device, "-t", "output")
	var stderr strings.Builder
	cmd.Stderr = &stderr
	if err := cmd.Run(); err != nil {
		msg := strings.TrimSpace(stderr.String())
		if msg != "" {
			return fmt.Errorf("switch output to %q: %w (%s)", device, err, msg)
		}
		return fmt.Errorf("switch output to %q: %w", device, err)
	}
	return nil
}

// isScreenSharingActive reports whether macOS Screen Sharing (screensharingd)
// is running — i.e. someone is viewing this Mac remotely. Used to choose the
// capture output: a plain BlackHole default is hijacked by Screen Sharing, so
// when it is up we route through the Multi-Output Device instead.
func isScreenSharingActive() bool {
	// pgrep exits 0 when at least one process matches.
	cctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	err := exec.CommandContext(cctx, "pgrep", "-x", "screensharingd").Run()
	return err == nil
}

// pickCaptureOutput chooses the output device that gets call audio into the
// BlackHole capture: Multi-Output Device when a human is screen-sharing
// (keeps the speakers live + survives the hijack), plain BlackHole otherwise.
func pickCaptureOutput() string {
	if isScreenSharingActive() {
		return outputMultiOutput
	}
	return outputBlackHole
}
