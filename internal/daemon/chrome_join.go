package daemon

// chrome_join.go — the "Mia joins the call" half of the Real-Time Synth
// feature. Drives the companion's persistent (headed) Playwright Chromium
// — profile ~/Library/Application Support/snth-companion/mia-chrome — into a
// Google Meet link as a participant (mic + camera OFF, listen-only), forces
// the system audio output to a BlackHole-reaching device, then starts the
// listen connector so the call audio streams to the hub transcribe provider.
//
// The join logic is a port of OpenClaw's google-meet meetStatusScript: ONE
// idempotent JS probe injected via the Playwright worker's `eval` action and
// polled until we are in the call (or a human-only step like Google login /
// lobby admission blocks us). DOM-state polling beats snapshot+ref clicking
// on Meet's dynamic React DOM.
//
// Listen vs Participate: this is listen-only v1 (mic muted). Participate
// (unmute + speak via the realtime provider) reuses the SAME join; it only
// flips the mic + adds the duplex audio bridge. Chrome-join + the listen
// connector (listen_connector.go) are the two halves; /api/call/join chains
// them.

import (
	"context"
	"encoding/json"
	"fmt"
	"log"
	"net/http"
	"net/url"
	"strings"
	"sync"
	"time"

	"github.com/snth-ai/snth-companion/internal/browser"
	"github.com/snth-ai/snth-companion/internal/config"
)

// CallState is a singleton; the UI / API drives it via /api/call/*.
type CallState struct {
	mu            sync.Mutex
	running       bool
	cancel        context.CancelFunc
	status        string // idle | launching | waiting_admission | in_call | manual_action | left | error
	meetURL       string
	manualReason  string
	manualMessage string
	lastErr       string
	lastProbe     string // last join-probe summary (title/url/notes) for diagnosis
	output         string // capture output device currently forced
	origOutput     string // output device captured at join start (to restore)
	outputSwitched bool   // we actually switched the system output (fallback) — restore needed
	startedAt      time.Time
	tornDown       bool // cleanup() ran — idempotency guard
}

var GlobalCall = &CallState{status: "idle"}

type CallStatus struct {
	Running       bool         `json:"running"`
	Status        string       `json:"status"`
	MeetURL       string       `json:"meet_url,omitempty"`
	ManualReason  string       `json:"manual_reason,omitempty"`
	ManualMessage string       `json:"manual_message,omitempty"`
	LastError     string       `json:"last_error,omitempty"`
	LastProbe     string       `json:"last_probe,omitempty"`
	Output        string       `json:"output_device,omitempty"`
	StartedAt     time.Time    `json:"started_at,omitempty"`
	Listen        ListenStatus `json:"listen"`
}

func (c *CallState) Snapshot() CallStatus {
	c.mu.Lock()
	defer c.mu.Unlock()
	return CallStatus{
		Running:       c.running,
		Status:        c.status,
		MeetURL:       c.meetURL,
		ManualReason:  c.manualReason,
		ManualMessage: c.manualMessage,
		LastError:     c.lastErr,
		LastProbe:     c.lastProbe,
		Output:        c.output,
		StartedAt:     c.startedAt,
		Listen:        GlobalListen.Snapshot(),
	}
}

func (c *CallState) setStatus(st, errMsg string) {
	c.mu.Lock()
	c.status = st
	if errMsg != "" {
		c.lastErr = errMsg
	}
	c.mu.Unlock()
}

// Join drives Chrome into the Meet link and (on success) starts listen.
func (c *CallState) Join(meetURL string) error {
	meetURL = strings.TrimSpace(meetURL)
	if !isMeetURL(meetURL) {
		return fmt.Errorf("not a Google Meet link: %q", meetURL)
	}
	cfg := config.Get()
	if cfg == nil || strings.TrimSpace(cfg.CompanionToken) == "" {
		return fmt.Errorf("not paired (no companion token)")
	}

	c.mu.Lock()
	if c.running {
		c.mu.Unlock()
		return fmt.Errorf("already in a call (%s)", c.status)
	}
	ctx, cancel := context.WithCancel(context.Background())
	c.cancel = cancel
	c.running = true
	c.status = "launching"
	c.meetURL = meetURL
	c.manualReason = ""
	c.manualMessage = ""
	c.lastErr = ""
	c.output = ""
	c.origOutput = ""
	c.outputSwitched = false
	c.tornDown = false
	c.startedAt = time.Now()
	c.mu.Unlock()

	go c.run(ctx, meetURL)
	return nil
}

func (c *CallState) run(ctx context.Context, meetURL string) {
	defer func() {
		c.mu.Lock()
		c.running = false
		c.cancel = nil
		c.mu.Unlock()
	}()

	// Navigate the headed Chromium (mia-chrome profile) to the Meet link.
	// We do NOT switch the system output here: once in-call the probe routes
	// ONLY Meet's <audio>/<video> elements to BlackHole via setSinkId, so the
	// speakers never carry the call (no echo) and Screen Sharing can't hijack
	// it. The system-output switch is a FALLBACK if setSinkId never confirms.
	log.Printf("[call] joining %s", meetURL)
	if _, err := browser.PWNavigate(ctx, meetURL); err != nil {
		c.setStatus("error", "chrome navigate: "+err.Error())
		return
	}
	c.setStatus("joining", "")

	// Capture the current system output up-front so a later fallback switch
	// can always be reverted — don't rely on reading it mid-fallback.
	if orig, err := currentAudioOutput(ctx); err == nil {
		c.mu.Lock()
		c.origOutput = orig
		c.mu.Unlock()
	} else {
		log.Printf("[call] could not read current output: %v", err)
	}

	// --- JOIN PHASE: poll until in-call / human-only blocker / timeout. ---
	deadline := time.Now().Add(150 * time.Second)
	evalErrs := 0
	for {
		select {
		case <-ctx.Done():
			c.cleanup("left")
			return
		default:
		}
		if time.Now().After(deadline) {
			c.mu.Lock()
			if c.status == "waiting_admission" {
				c.manualReason = "meet-admission-timeout"
				c.manualMessage = "Nobody admitted Mia within the wait window. Admit her and rejoin."
				c.status = "manual_action"
			} else {
				c.status = "error"
				c.lastErr = "join timed out"
			}
			c.mu.Unlock()
			return
		}

		probe, err := c.evalJoin(ctx)
		if err != nil {
			evalErrs++
			if evalErrs >= 8 {
				c.setStatus("error", "join probe: "+err.Error())
				return
			}
			time.Sleep(1500 * time.Millisecond)
			continue
		}
		evalErrs = 0
		c.recordProbe(probe)

		if probe.InCall {
			break // -> in-call setup below
		}
		// Terminal human-only blockers (login / permission). Lobby/admission
		// is NOT terminal — keep waiting, the host may admit.
		if probe.ManualActionRequired && probe.ManualActionReason != "meet-admission-required" {
			c.mu.Lock()
			c.status = "manual_action"
			c.manualReason = probe.ManualActionReason
			c.manualMessage = probe.ManualActionMessage
			c.mu.Unlock()
			log.Printf("[call] manual action: %s — %s", probe.ManualActionReason, probe.ManualActionMessage)
			return
		}
		if probe.LobbyWaiting || probe.ManualActionReason == "meet-admission-required" {
			c.setStatus("waiting_admission", "")
		}
		time.Sleep(1500 * time.Millisecond)
	}

	// --- IN-CALL: start the BlackHole capture. ---
	if err := GlobalListen.Start(""); err != nil {
		c.cleanup("error")
		c.setStatus("error", "listen start: "+err.Error())
		return
	}
	c.setStatus("in_call", "")
	log.Printf("[call] in call %s — listen started", meetURL)

	// --- IN-CALL MAINTENANCE: re-apply setSinkId for new media elements,
	//     fall back to a system-output switch if it never confirms, and
	//     auto-clean-up when the call ends (inCall flips to false). ---
	notRoutedSince := time.Now()
	switched := false
	mErrs := 0
	for {
		select {
		case <-ctx.Done():
			c.cleanup("left")
			return
		default:
		}
		time.Sleep(3 * time.Second)

		probe, err := c.evalJoin(ctx)
		if err != nil {
			mErrs++
			if mErrs >= 8 { // sustained failure (page gone / chrome dropped) -> ended
				c.cleanup("call_ended")
				return
			}
			continue
		}
		mErrs = 0
		c.recordProbe(probe)

		if !probe.InCall {
			log.Printf("[call] call ended (inCall=false) — cleaning up")
			c.cleanup("call_ended")
			return
		}

		if probe.AudioRouted {
			notRoutedSince = time.Now()
			c.mu.Lock()
			c.output = probe.AudioDevice
			c.mu.Unlock()
			if switched {
				// setSinkId took over — undo the fallback so speakers stop echoing.
				c.restoreOutput(ctx)
				switched = false
			}
		} else if !switched && time.Since(notRoutedSince) > 8*time.Second {
			// setSinkId never confirmed — fall back to switching the system
			// output (capture works, but the speakers will echo). origOutput
			// was already captured at join start.
			out := pickCaptureOutput()
			if e := switchAudioOutput(ctx, out); e == nil {
				switched = true
				c.mu.Lock()
				c.outputSwitched = true
				c.output = out + " (fallback)"
				c.mu.Unlock()
				log.Printf("[call] setSinkId unconfirmed — fell back to system output %q", out)
			}
		}
	}
}

// recordProbe stores a one-line probe summary for /api/call/status and logs it.
func (c *CallState) recordProbe(p *joinProbe) {
	summary := fmt.Sprintf("inCall=%v lobby=%v routed=%v reason=%q name=%v title=%q notes=%v",
		p.InCall, p.LobbyWaiting, p.AudioRouted, p.ManualActionReason, p.NameFilled, p.Title, p.Notes)
	c.mu.Lock()
	c.lastProbe = summary
	c.mu.Unlock()
	log.Printf("[call] probe: %s", summary)
}

// cleanup tears down a call exactly once: stop listen, click leave, navigate
// the tab off Meet, restore any system-output fallback. Idempotent via
// tornDown so Leave() and the run() goroutine can both call it safely.
func (c *CallState) cleanup(status string) {
	c.mu.Lock()
	if c.tornDown {
		c.status = status
		c.mu.Unlock()
		return
	}
	c.tornDown = true
	c.mu.Unlock()

	GlobalListen.Stop()
	tctx, cancel := context.WithTimeout(context.Background(), 12*time.Second)
	defer cancel()
	if _, err := browser.PWEval(tctx, meetLeaveScript); err != nil {
		log.Printf("[call] leave click: %v", err)
	}
	// Leaving the Meet page also disconnects her from any still-live call.
	if _, err := browser.PWNavigate(tctx, "about:blank"); err != nil {
		log.Printf("[call] navigate away: %v", err)
	}
	c.restoreOutput(tctx)
	c.mu.Lock()
	c.running = false
	c.status = status
	c.output = ""
	c.mu.Unlock()
}

// Leave stops the call: cancels the run loop and tears down (idempotent).
func (c *CallState) Leave() {
	c.mu.Lock()
	cancel := c.cancel
	c.cancel = nil
	c.mu.Unlock()
	if cancel != nil {
		cancel() // an active run() goroutine cleans up on ctx.Done
	}
	// If run() already exited (manual_action / timeout) nothing reacts to the
	// cancel — so tear down here too. cleanup() is idempotent via tornDown.
	c.cleanup("left")
}

func (c *CallState) restoreOutput(ctx context.Context) {
	c.mu.Lock()
	did := c.outputSwitched
	orig := c.origOutput
	c.mu.Unlock()
	if !did {
		return // never switched the system output -> nothing to restore
	}
	if strings.TrimSpace(orig) == "" {
		log.Printf("[call] cannot restore output: original not captured")
		return
	}
	// Use a fresh short context — the run ctx may be cancelled on leave.
	rctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	if err := switchAudioOutput(rctx, orig); err != nil {
		msg := fmt.Sprintf("restore output to %q: %v", orig, err)
		c.mu.Lock()
		c.lastErr = msg
		c.mu.Unlock()
		log.Printf("[call] %s", msg)
		return
	}
	c.mu.Lock()
	c.outputSwitched = false
	c.mu.Unlock()
}

// joinProbe mirrors the JSON returned by meetJoinScript.
type joinProbe struct {
	InCall               bool     `json:"inCall"`
	MicMuted             *bool    `json:"micMuted"`
	CameraOff            *bool    `json:"cameraOff"`
	ClickedJoin          bool     `json:"clickedJoin"`
	LobbyWaiting         bool     `json:"lobbyWaiting"`
	NameFilled           bool     `json:"nameFilled"`
	ManualActionRequired bool     `json:"manualActionRequired"`
	ManualActionReason   string   `json:"manualActionReason"`
	ManualActionMessage  string   `json:"manualActionMessage"`
	AudioRouted          bool     `json:"audioRouted"`
	AudioDevice          string   `json:"audioDevice"`
	Title                string   `json:"title"`
	URL                  string   `json:"url"`
	Notes                []string `json:"notes"`
}

func (c *CallState) evalJoin(ctx context.Context) (*joinProbe, error) {
	raw, err := browser.PWEval(ctx, meetJoinScript)
	if err != nil {
		return nil, err
	}
	raw = strings.TrimSpace(raw)
	if raw == "" || raw == "undefined" || raw == "null" {
		return nil, fmt.Errorf("empty probe result")
	}
	var p joinProbe
	if err := json.Unmarshal([]byte(raw), &p); err != nil {
		return nil, fmt.Errorf("decode probe: %w (head: %.160s)", err, raw)
	}
	return &p, nil
}

func isMeetURL(s string) bool {
	u, err := url.Parse(s)
	if err != nil {
		return false
	}
	h := strings.ToLower(u.Hostname())
	return u.Scheme == "https" && (h == "meet.google.com" || strings.HasSuffix(h, ".meet.google.com"))
}

// meetJoinScript — idempotent Meet join probe, listen-mode (mic + camera OFF,
// guest name "Mia"). Ported from OpenClaw google-meet meetStatusScript. A
// called ASYNC IIFE returning a plain object (the routing step is async);
// the Playwright worker awaits + JSON-stringifies it. Re-running is safe: it
// only clicks join/mute/camera when the control is in the wrong state, and
// once in-call it routes Meet's own <audio>/<video> output to BlackHole via
// setSinkId so the speakers never carry the call (no echo, Screen-Sharing
// safe) while the BlackHole-input capture still gets it.
const meetJoinScript = `(async () => {
  const text = (n) => ((n && (n.innerText || n.textContent)) || "").trim();
  const buttons = [...document.querySelectorAll('button')];
  const label = (b) => [b.getAttribute("aria-label"), b.getAttribute("data-tooltip"), text(b)].filter(Boolean).join(" ");
  const labels = buttons.map(label).filter(Boolean);
  const notes = [];
  const find = (re) => buttons.find((b) => re.test(label(b)) && !b.disabled);
  const findCC = (re) => buttons.find((b) => re.test(label(b)) && !/remotely mute|someone else/i.test(label(b)) && !b.disabled);

  // Guest name (present only when NOT signed into Google).
  let nameFilled = false;
  const nameInput = [...document.querySelectorAll('input')].find((el) => /your name/i.test(el.getAttribute('aria-label') || el.placeholder || ''));
  if (nameInput && !nameInput.value) {
    try {
      nameInput.focus();
      const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, "value").set;
      setter.call(nameInput, "Mia");
      nameInput.dispatchEvent(new Event('input', { bubbles: true }));
      nameInput.dispatchEvent(new Event('change', { bubbles: true }));
      nameFilled = true;
      notes.push("filled guest name Mia");
    } catch (e) { notes.push("name fill failed: " + (e && e.message || e)); }
  }

  const pageText = text(document.body).toLowerCase();
  const host = location.hostname.toLowerCase();
  const permText = [pageText, ...labels].join("\n");
  const permissionNeeded = /permission needed|allow .*(microphone|camera)|blocked .*(microphone|camera)/i.test(permText);

  // Microphone OFF (listen-only).
  let mic = findCC(/^\s*turn (?:off|on) microphone\b/i);
  if (!mic) {
    const cc = document.querySelector('[role="region"][aria-label="Call controls"]');
    mic = [...((cc && cc.querySelectorAll('button')) || [])].find((b) => /^\s*turn (?:off|on) microphone\b/i.test(label(b)));
  }
  if (mic && /turn off microphone/i.test(label(mic))) { mic.click(); notes.push("muted mic"); }

  // Camera OFF.
  let cam = findCC(/^\s*turn (?:off|on) camera\b/i);
  if (!cam) {
    const cc = document.querySelector('[role="region"][aria-label="Call controls"]');
    cam = [...((cc && cc.querySelectorAll('button')) || [])].find((b) => /^\s*turn (?:off|on) camera\b/i.test(label(b)));
  }
  if (cam && /turn off camera/i.test(label(cam))) { cam.click(); notes.push("camera off"); }

  // Skip-mic prompt ("Continue without microphone / camera"). Kept narrow
  // (no bare "not now") so we never click an unrelated dismissal dialog.
  const noMic = find(/\b(continue|join|use) without (microphone|mic|camera)\b/i);
  if (noMic) { noMic.click(); notes.push("continue without mic"); }

  // Join.
  const join = find(/join now|ask to join/i);
  if (join) { join.click(); notes.push("clicked join"); }

  const inCall = buttons.some((b) => /leave call/i.test(b.getAttribute('aria-label') || text(b)));
  const lobbyWaiting = !inCall && /asking to be let in|you.?ll join when someone lets you in|waiting to be let in/i.test(pageText);

  // Route Meet's media output to BlackHole (per-element setSinkId) so the
  // call audio reaches the BlackHole-input capture WITHOUT touching the
  // system default output -> no speaker echo, immune to Screen Sharing.
  // Re-applied every tick because Meet adds an element per speaker.
  let audioRouted = false, audioDevice = "";
  if (inCall && navigator.mediaDevices && navigator.mediaDevices.enumerateDevices) {
    try {
      const els = [...document.querySelectorAll('audio, video')].filter((e) => typeof e.setSinkId === 'function');
      if (els.length) {
        const devices = await navigator.mediaDevices.enumerateDevices();
        if (!devices.length) notes.push("enumerateDevices empty");
        else if (!devices.some((d) => d.kind === 'audiooutput')) notes.push("no audiooutput devices (mic perm?)");
        const bh = devices.find((d) => d.kind === 'audiooutput' && /\bBlackHole\s+2ch\b/i.test(d.label || ''))
                || devices.find((d) => d.kind === 'audiooutput' && /\bBlackHole\b/i.test(d.label || ''));
        if (bh && bh.deviceId) {
          for (const e of els) {
            if (e.sinkId !== bh.deviceId) { try { await e.setSinkId(bh.deviceId); } catch (_) {} }
          }
          audioRouted = els.some((e) => e.sinkId === bh.deviceId);
          audioDevice = bh.label || "BlackHole 2ch";
        } else if (devices.some((d) => d.kind === 'audiooutput' && d.label)) {
          notes.push("BlackHole output not visible to Meet");
        }
      }
    } catch (e) { notes.push("route err: " + (e && e.message || e)); }
  }

  let reason, message;
  if (!inCall && (host === "accounts.google.com" || /use your google account|to continue to google meet|choose an account|sign in to (join|continue)/i.test(pageText))) {
    reason = "google-login-required";
    message = "Sign in to Google in Mia's browser profile (mia-chrome), then rejoin.";
  } else if (lobbyWaiting) {
    reason = "meet-admission-required";
    message = "Admit Mia in the Meet; she joins when let in.";
  } else if (permissionNeeded) {
    reason = "meet-permission-required";
    message = "Allow or skip microphone/camera permission in Mia's browser, then rejoin.";
  }

  return {
    inCall,
    micMuted: mic ? /turn on microphone/i.test(label(mic)) : undefined,
    cameraOff: cam ? /turn on camera/i.test(label(cam)) : undefined,
    clickedJoin: Boolean(join),
    lobbyWaiting,
    nameFilled,
    manualActionRequired: Boolean(reason),
    manualActionReason: reason,
    manualActionMessage: message,
    audioRouted,
    audioDevice,
    title: document.title,
    url: location.href,
    notes
  };
})()`

// meetLeaveScript clicks the in-call "Leave call" control. Idempotent.
const meetLeaveScript = `(() => {
  const text = (n) => ((n && (n.innerText || n.textContent)) || "").trim();
  const buttons = [...document.querySelectorAll('button')];
  const label = (b) => [b.getAttribute("aria-label"), b.getAttribute("data-tooltip"), text(b)].filter(Boolean).join(" ");
  const leave = buttons.find((b) => /leave call/i.test(label(b)) && !b.disabled);
  if (leave) { leave.click(); return { left: true }; }
  return { left: false };
})()`

// --- local API (driven by the companion UI Real-Time tab) ---

func (s *UIServer) apiCallJoin(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		writeJSONErr(w, http.StatusMethodNotAllowed, "POST only")
		return
	}
	var req struct {
		MeetURL string `json:"meet_url"`
	}
	_ = json.NewDecoder(r.Body).Decode(&req)
	if err := GlobalCall.Join(req.MeetURL); err != nil {
		writeJSONErr(w, http.StatusBadRequest, err.Error())
		return
	}
	writeJSON(w, http.StatusOK, GlobalCall.Snapshot())
}

func (s *UIServer) apiCallLeave(w http.ResponseWriter, r *http.Request) {
	GlobalCall.Leave()
	writeJSON(w, http.StatusOK, GlobalCall.Snapshot())
}

func (s *UIServer) apiCallStatus(w http.ResponseWriter, r *http.Request) {
	writeJSON(w, http.StatusOK, GlobalCall.Snapshot())
}
