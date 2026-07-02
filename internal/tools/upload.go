package tools

// upload_to_synth — HTTPS data-plane upload of a local file to the paired
// synth (openpaw report #130). The synth registers a server-owned
// upload_id and calls this verb over the WS control-plane; we ACK fast
// ("started" / early error) and stream 8 MB chunks in our own goroutine so
// a long upload never blocks other companion verbs. Strict-offset
// contract: a 409 from the synth carries bytes_received — we seek there
// and continue (resume after either side restarts). sha256 is computed
// while streaming and validated at complete.

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"log"
	"net/http"
	"os"
	"strings"
	"time"

	"github.com/snth-ai/snth-companion/internal/config"
)

func RegisterUpload() {
	Register(Descriptor{
		Name:        "upload_to_synth",
		Description: "Stream a local file to the synth workspace over HTTPS (resumable, GB-scale). Called by the synth's companion:// send flow.",
		DangerLevel: "safe", // read-only on the Mac; destination is the synth's own workspace
	}, uploadHandler)
}

type uploadArgs struct {
	Path      string `json:"path"`
	UploadID  string `json:"upload_id"`
	ChunkSize int64  `json:"chunk_size"`
	MaxBytes  int64  `json:"max_bytes"`
}

func uploadHandler(ctx context.Context, args json.RawMessage) (any, error) {
	var a uploadArgs
	if err := json.Unmarshal(args, &a); err != nil {
		return nil, fmt.Errorf("bad args: %w", err)
	}
	if a.Path == "" || a.UploadID == "" {
		return nil, fmt.Errorf("path and upload_id required")
	}
	if a.ChunkSize <= 0 {
		a.ChunkSize = 8 << 20
	}

	cfg := config.Get()
	base := strings.TrimRight(cfg.PairedSynthURL, "/")
	if base == "" || cfg.CompanionToken == "" {
		return nil, fmt.Errorf("not paired with a synth")
	}

	// Early errors belong in the ACK: missing file / over cap fail here,
	// BEFORE any bytes move (TZ acceptance 5-6).
	fi, err := os.Stat(a.Path)
	if err != nil {
		return nil, fmt.Errorf("file not found on this Mac: %s", a.Path)
	}
	if fi.IsDir() {
		return nil, fmt.Errorf("%s is a directory", a.Path)
	}
	if a.MaxBytes > 0 && fi.Size() > a.MaxBytes {
		return nil, fmt.Errorf("file is %d bytes, over the synth's %d-byte cap", fi.Size(), a.MaxBytes)
	}

	go streamUpload(base, cfg.CompanionToken, a, fi.Size())

	return map[string]any{"started": true, "total_size": fi.Size()}, nil
}

// streamUpload runs detached: chunk loop with strict-offset resume, then
// complete. On unrecoverable errors it just stops — the synth's stall
// watchdog (60s without progress) fails the upload on its side.
func streamUpload(base, token string, a uploadArgs, total int64) {
	url := base + "/api/companion/upload/" + a.UploadID
	client := &http.Client{Timeout: 5 * time.Minute}

	f, err := os.Open(a.Path)
	if err != nil {
		log.Printf("[upload] open %s: %v", a.Path, err)
		return
	}
	defer f.Close()

	// sha256 rides along: hash exactly the bytes we sent, in order. On a
	// resume-seek we rehash from the file start to the new offset so the
	// running hash stays aligned with the server's bytes.
	offset := int64(0)
	hash := sha256.New()
	buf := make([]byte, a.ChunkSize)
	consecutiveErrs := 0

	rehashTo := func(target int64) bool {
		if _, err := f.Seek(0, io.SeekStart); err != nil {
			return false
		}
		hash.Reset()
		if _, err := io.CopyN(hash, f, target); err != nil && target > 0 {
			return false
		}
		offset = target
		return true
	}

	for offset < total {
		n, rerr := f.ReadAt(buf, offset)
		if n == 0 {
			log.Printf("[upload] %s read at %d: %v", a.UploadID, offset, rerr)
			return
		}
		chunk := buf[:n]

		req, _ := http.NewRequest(http.MethodPut, url, bytes.NewReader(chunk))
		req.Header.Set("Authorization", "Bearer "+token)
		req.Header.Set("X-Chunk-Offset", fmt.Sprintf("%d", offset))
		req.Header.Set("X-Total-Size", fmt.Sprintf("%d", total))
		req.Header.Set("Content-Type", "application/octet-stream")
		resp, herr := client.Do(req)
		if herr != nil {
			consecutiveErrs++
			if consecutiveErrs > 5 {
				log.Printf("[upload] %s giving up after 5 transport errors: %v", a.UploadID, herr)
				return
			}
			time.Sleep(time.Duration(consecutiveErrs) * 2 * time.Second)
			continue
		}
		body, _ := io.ReadAll(io.LimitReader(resp.Body, 4096))
		resp.Body.Close()

		switch resp.StatusCode {
		case http.StatusOK:
			consecutiveErrs = 0
			hash.Write(chunk)
			offset += int64(n)
		case http.StatusConflict:
			// Strict-offset contract: the server tells us where it is.
			var st struct {
				BytesReceived int64 `json:"bytes_received"`
			}
			if json.Unmarshal(body, &st) != nil || !rehashTo(st.BytesReceived) {
				log.Printf("[upload] %s conflict resync failed: %s", a.UploadID, string(body))
				return
			}
			log.Printf("[upload] %s resumed at %d", a.UploadID, offset)
		default:
			log.Printf("[upload] %s chunk at %d: HTTP %d %s", a.UploadID, offset, resp.StatusCode, strings.TrimSpace(string(body)))
			return // 4xx/5xx beyond conflict: unrecoverable (cap, failed upload)
		}
	}

	// Complete: name + hash validation server-side, atomic landing.
	payload, _ := json.Marshal(map[string]string{
		"file_name": fileBase(a.Path),
		"sha256":    hex.EncodeToString(hash.Sum(nil)),
	})
	for attempt := 0; attempt < 3; attempt++ {
		req, _ := http.NewRequest(http.MethodPost, url+"/complete", bytes.NewReader(payload))
		req.Header.Set("Authorization", "Bearer "+token)
		req.Header.Set("Content-Type", "application/json")
		resp, herr := client.Do(req)
		if herr != nil {
			time.Sleep(2 * time.Second)
			continue
		}
		body, _ := io.ReadAll(io.LimitReader(resp.Body, 4096))
		resp.Body.Close()
		if resp.StatusCode == http.StatusOK {
			log.Printf("[upload] %s complete (%d bytes)", a.UploadID, total)
			return
		}
		log.Printf("[upload] %s complete: HTTP %d %s", a.UploadID, resp.StatusCode, strings.TrimSpace(string(body)))
		if resp.StatusCode == http.StatusConflict {
			return // size/hash mismatch — synth marked it failed
		}
	}
}

func fileBase(p string) string {
	if i := strings.LastIndexByte(p, '/'); i >= 0 {
		return p[i+1:]
	}
	return p
}
