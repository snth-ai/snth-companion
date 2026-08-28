package daemon

// listen_connector.go - the companion half of the Real-Time Synth "listen"
// surface. Captures the Mac's call audio from a BlackHole loopback device
// (via ffmpeg) and streams it to the hub's realtime transcribe provider
// (wss .../api/realtime/ws?provider=transcribe), surfacing the live
// transcript. The hub runs the gpt-realtime-whisper STT and ships finals to
// the synth. Chrome-join (routing the call audio into BlackHole) is a
// separate piece; this connector just captures whatever the BlackHole device
// carries.

import (
	"bufio"
	"context"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"io"
	"log"
	"net/http"
	"os/exec"
	"regexp"
	"strings"
	"sync"
	"time"

	"github.com/gorilla/websocket"

	"github.com/snth-ai/snth-companion/internal/config"
)

const defaultListenDevice = "BlackHole 2ch"

// ListenConnector is a singleton; the UI drives it via /api/listen/*.
type ListenConnector struct {
	mu        sync.Mutex
	running   bool
	cancel    context.CancelFunc
	device    string
	status    string // idle | starting | listening | stopped | error
	lastErr   string
	finals    []string
	partial   string
	startedAt time.Time
	// gen is bumped on every Start/Stop transition (F4). A run() captures
	// its gen at launch and only mutates shared session state (running,
	// status) when the current gen still matches — so a Start() that races
	// the old run's unwind can't have its fresh session clobbered by the
	// old run's deferred cleanup.
	gen uint64
}

var GlobalListen = &ListenConnector{status: "idle", device: defaultListenDevice}

type ListenStatus struct {
	Running    bool      `json:"running"`
	Status     string    `json:"status"`
	Device     string    `json:"device"`
	LastError  string    `json:"last_error,omitempty"`
	Transcript string    `json:"transcript"`
	Partial    string    `json:"partial,omitempty"`
	StartedAt  time.Time `json:"started_at,omitempty"`
}

func (l *ListenConnector) Snapshot() ListenStatus {
	l.mu.Lock()
	defer l.mu.Unlock()
	return ListenStatus{
		Running:    l.running,
		Status:     l.status,
		Device:     l.device,
		LastError:  l.lastErr,
		Transcript: strings.TrimSpace(strings.Join(l.finals, " ")),
		Partial:    l.partial,
		StartedAt:  l.startedAt,
	}
}

func (l *ListenConnector) Start(device string) error {
	l.mu.Lock()
	if l.running {
		l.mu.Unlock()
		return fmt.Errorf("already listening")
	}
	cfg := config.Get()
	if cfg == nil || strings.TrimSpace(cfg.CompanionToken) == "" {
		l.mu.Unlock()
		return fmt.Errorf("not paired (no companion token)")
	}
	if strings.TrimSpace(device) != "" {
		l.device = strings.TrimSpace(device)
	}
	ctx, cancel := context.WithCancel(context.Background())
	l.gen++
	gen := l.gen
	l.cancel = cancel
	l.running = true
	l.status = "starting"
	l.lastErr = ""
	l.finals = nil
	l.partial = ""
	l.startedAt = time.Now()
	dev, token := l.device, cfg.CompanionToken
	l.mu.Unlock()

	go l.run(ctx, gen, dev, token)
	return nil
}

func (l *ListenConnector) Stop() {
	l.mu.Lock()
	l.gen++ // invalidate the current run's session (F4)
	c := l.cancel
	l.cancel = nil
	l.running = false
	l.status = "stopped"
	l.mu.Unlock()
	if c != nil {
		c()
	}
}

// setStatus updates status/lastErr only when gen still matches the caller's
// run generation — a stale run must not overwrite a newer session's status.
func (l *ListenConnector) setStatus(gen uint64, st, errMsg string) {
	l.mu.Lock()
	if l.gen == gen {
		l.status = st
		if errMsg != "" {
			l.lastErr = errMsg
		}
	}
	l.mu.Unlock()
}

// finishRun performs a run's deferred cleanup. F4: it clears session state
// ONLY when this run still owns the current generation. A Start() (or
// Stop()+Start()) that raced this run's unwind bumped gen, so an unguarded
// cleanup would stomp the new session's running=true / status.
func (l *ListenConnector) finishRun(gen uint64) {
	l.mu.Lock()
	if l.gen == gen {
		l.running = false
		if l.status == "listening" || l.status == "starting" {
			l.status = "stopped"
		}
	}
	l.mu.Unlock()
}

func (l *ListenConnector) run(ctx context.Context, gen uint64, device, token string) {
	defer l.finishRun(gen)

	idx, err := avfoundationAudioIndex(device)
	if err != nil {
		l.setStatus(gen, "error", "device: "+err.Error())
		return
	}

	wsURL := strings.Replace(strings.Replace(hubBaseURLForListen(), "https://", "wss://", 1), "http://", "ws://", 1) +
		"/api/realtime/ws?provider=transcribe"
	hdr := http.Header{}
	hdr.Set("Authorization", "Bearer "+token)
	hdr.Set("User-Agent", "snth-companion/"+Version)
	dialer := *websocket.DefaultDialer
	dialer.HandshakeTimeout = 15 * time.Second
	conn, _, err := dialer.Dial(wsURL, hdr)
	if err != nil {
		l.setStatus(gen, "error", "hub dial: "+err.Error())
		return
	}
	defer conn.Close()

	// ffmpeg: BlackHole input -> raw PCM16 24kHz mono on stdout.
	cmd := exec.CommandContext(ctx, "ffmpeg", "-loglevel", "error",
		"-f", "avfoundation", "-i", ":"+idx,
		"-f", "s16le", "-ar", "24000", "-ac", "1", "-")
	stdout, err := cmd.StdoutPipe()
	if err != nil {
		l.setStatus(gen, "error", "ffmpeg pipe: "+err.Error())
		return
	}
	if err := cmd.Start(); err != nil {
		l.setStatus(gen, "error", "ffmpeg start: "+err.Error())
		return
	}
	defer func() {
		if cmd.Process != nil {
			_ = cmd.Process.Kill()
		}
		_ = cmd.Wait()
	}()

	l.setStatus(gen, "listening", "")
	log.Printf("[listen] streaming %q (avf:%s) -> %s", device, idx, wsURL)

	// Reader: hub frames -> transcript.
	go func() {
		for {
			var ev map[string]any
			if err := conn.ReadJSON(&ev); err != nil {
				return
			}
			switch t, _ := ev["type"].(string); t {
			case "transcript":
				text, _ := ev["text"].(string)
				final, _ := ev["final"].(bool)
				l.mu.Lock()
				// F4: a stale run's reader must not append to a newer
				// session's transcript.
				if l.gen == gen {
					if final {
						if strings.TrimSpace(text) != "" {
							l.finals = append(l.finals, text)
						}
						l.partial = ""
					} else {
						l.partial = text
					}
				}
				l.mu.Unlock()
			case "error":
				b, _ := json.Marshal(ev)
				log.Printf("[listen] hub error: %s", truncListen(string(b), 200))
			}
		}
	}()

	// Pump PCM from ffmpeg stdout -> audio_in frames. The hub's transcribe
	// provider commits on its own 4s cadence, so we just stream audio.
	buf := make([]byte, 4800) // 100ms of 24kHz s16 mono
	r := bufio.NewReader(stdout)
	for {
		select {
		case <-ctx.Done():
			return
		default:
		}
		n, rerr := io.ReadFull(r, buf)
		if n > 0 {
			frame, _ := json.Marshal(map[string]any{
				"type": "audio_in",
				"b64":  base64.StdEncoding.EncodeToString(buf[:n]),
			})
			_ = conn.SetWriteDeadline(time.Now().Add(10 * time.Second))
			if werr := conn.WriteMessage(websocket.TextMessage, frame); werr != nil {
				l.setStatus(gen, "error", "ws write: "+werr.Error())
				return
			}
		}
		if rerr != nil {
			if rerr == io.EOF || rerr == io.ErrUnexpectedEOF {
				return
			}
			l.setStatus(gen, "error", "ffmpeg read: "+rerr.Error())
			return
		}
	}
}

// avfoundationAudioIndex resolves an avfoundation audio input device NAME to
// its ffmpeg index (e.g. "BlackHole 2ch" -> "0"). Indices shift when devices
// (re)connect, so we always resolve by name.
func avfoundationAudioIndex(device string) (string, error) {
	out, _ := exec.Command("ffmpeg", "-f", "avfoundation", "-list_devices", "true", "-i", "").CombinedOutput()
	inAudio := false
	re := regexp.MustCompile(`\[(\d+)\]\s+(.*)`)
	for _, ln := range strings.Split(string(out), "\n") {
		if strings.Contains(ln, "AVFoundation audio devices") {
			inAudio = true
			continue
		}
		if !inAudio {
			continue
		}
		if m := re.FindStringSubmatch(ln); m != nil && strings.TrimSpace(m[2]) == device {
			return m[1], nil
		}
	}
	return "", fmt.Errorf("audio device %q not found (is BlackHole installed?)", device)
}

func hubBaseURLForListen() string {
	if cfg := config.Get(); cfg != nil && strings.TrimSpace(cfg.HubURL) != "" {
		return strings.TrimRight(cfg.HubURL, "/")
	}
	return "https://hub.snth.ai"
}

func truncListen(s string, n int) string {
	if len(s) <= n {
		return s
	}
	return s[:n]
}

// --- local API (driven by the companion UI Real-Time tab) ---

func (s *UIServer) apiListenStart(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		writeJSONErr(w, http.StatusMethodNotAllowed, "POST only")
		return
	}
	var req struct {
		Device string `json:"device"`
	}
	_ = json.NewDecoder(r.Body).Decode(&req)
	if err := GlobalListen.Start(req.Device); err != nil {
		writeJSONErr(w, http.StatusBadRequest, err.Error())
		return
	}
	writeJSON(w, http.StatusOK, GlobalListen.Snapshot())
}

func (s *UIServer) apiListenStop(w http.ResponseWriter, r *http.Request) {
	GlobalListen.Stop()
	writeJSON(w, http.StatusOK, GlobalListen.Snapshot())
}

func (s *UIServer) apiListenStatus(w http.ResponseWriter, r *http.Request) {
	writeJSON(w, http.StatusOK, GlobalListen.Snapshot())
}
