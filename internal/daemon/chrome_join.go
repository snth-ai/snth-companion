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
	output        string // capture output device currently forced
	origOutput    string // output device to restore on leave
	startedAt     time.Time
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

	// 1. Force system output to a BlackHole-reaching device so the listen
	//    connector's BlackHole-input capture carries the call.
	out := pickCaptureOutput()
	if orig, err := currentAudioOutput(ctx); err == nil {
		c.mu.Lock()
		c.origOutput = orig
		c.mu.Unlock()
	}
	if err := switchAudioOutput(ctx, out); err != nil {
		c.restoreOutput(ctx)
		c.setStatus("error", "audio output: "+err.Error())
		return
	}
	c.mu.Lock()
	c.output = out
	c.mu.Unlock()
	log.Printf("[call] output -> %q, joining %s", out, meetURL)

	// 2. Navigate the headed Chromium (mia-chrome profile) to the Meet link.
	if _, err := browser.PWNavigate(ctx, meetURL); err != nil {
		c.restoreOutput(ctx)
		c.setStatus("error", "chrome navigate: "+err.Error())
		return
	}

	c.setStatus("joining", "")

	// 3. Poll the join probe until we're in the call, or a human-only step
	//    blocks us (Google login / admission). Idempotent re-clicks.
	deadline := time.Now().Add(150 * time.Second)
	evalErrs := 0
	for {
		select {
		case <-ctx.Done():
			c.restoreOutput(ctx)
			c.setStatus("left", "")
			return
		default:
		}
		if time.Now().After(deadline) {
			c.restoreOutput(ctx)
			c.mu.Lock()
			if c.status == "waiting_admission" {
				c.manualReason = "meet-admission-timeout"
				c.manualMessage = "Nobody admitted Mia within the wait window. Admit her in the Meet and rejoin."
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
				c.restoreOutput(ctx)
				c.setStatus("error", "join probe: "+err.Error())
				return
			}
			time.Sleep(1500 * time.Millisecond)
			continue
		}
		evalErrs = 0

		summary := fmt.Sprintf("inCall=%v lobby=%v reason=%q name=%v title=%q url=%q notes=%v",
			probe.InCall, probe.LobbyWaiting, probe.ManualActionReason, probe.NameFilled, probe.Title, probe.URL, probe.Notes)
		c.mu.Lock()
		c.lastProbe = summary
		c.mu.Unlock()
		log.Printf("[call] probe: %s", summary)

		if probe.InCall {
			// 4. We're in. Start the listen capture.
			if err := GlobalListen.Start(""); err != nil {
				c.restoreOutput(ctx)
				c.setStatus("error", "listen start: "+err.Error())
				return
			}
			c.setStatus("in_call", "")
			log.Printf("[call] in call %s — listen started", meetURL)
			return
		}

		// Terminal human-only blockers (login / permission). Lobby/admission
		// is NOT terminal — keep waiting, the host may admit.
		if probe.ManualActionRequired &&
			probe.ManualActionReason != "meet-admission-required" {
			c.restoreOutput(ctx)
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
}

// Leave stops the listen capture, leaves the Meet, and restores output.
func (c *CallState) Leave() {
	c.mu.Lock()
	cancel := c.cancel
	c.cancel = nil
	c.mu.Unlock()
	if cancel != nil {
		cancel()
	}
	GlobalListen.Stop()
	// Best-effort: click the in-call "Leave call" button.
	ctx, cancelTO := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancelTO()
	if _, err := browser.PWEval(ctx, meetLeaveScript); err != nil {
		log.Printf("[call] leave click: %v", err)
	}
	c.restoreOutput(ctx)
	c.mu.Lock()
	c.running = false
	c.status = "left"
	c.output = ""
	c.mu.Unlock()
}

func (c *CallState) restoreOutput(ctx context.Context) {
	c.mu.Lock()
	orig := c.origOutput
	c.mu.Unlock()
	if strings.TrimSpace(orig) == "" {
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
	}
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
// guest name "Mia"). Ported from OpenClaw google-meet meetStatusScript. An
// EXPRESSION (called IIFE) returning a plain object; the Playwright worker
// JSON-stringifies it. Re-running is safe: it only clicks join/mute/camera
// when the corresponding control is present and in the wrong state.
const meetJoinScript = `(() => {
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
