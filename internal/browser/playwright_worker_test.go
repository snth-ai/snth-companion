package browser

// playwright_worker_test.go — F1 (ref keyspace) + F2 (single reader
// goroutine, no leak / no stolen response).

import (
	"bufio"
	"encoding/json"
	"io"
	"strings"
	"sync"
	"testing"
	"time"
)

// F1 — the click/type JS must resolve refs by scanning for
// highlightIndex === N (like the CDP path), NOT by indexing map[N]. We
// assert on the generated snippet since running a real browser isn't
// available in CI.
func TestPWRefResolutionScansHighlightIndex(t *testing.T) {
	js := pwFindByHighlightJS(7)
	if !strings.Contains(js, "highlightIndex === 7") {
		t.Fatalf("ref-resolution JS does not scan by highlightIndex:\n%s", js)
	}
	if strings.Contains(js, "map[7]") {
		t.Fatalf("ref-resolution JS still indexes map[N] directly:\n%s", js)
	}
	if !strings.Contains(js, "Object.keys") {
		t.Fatalf("ref-resolution JS does not iterate map keys:\n%s", js)
	}
}

// pipeWorker wires a PWWorker with pipe-backed stdin/stdout and the single
// reader goroutine running. reqIDs receives every request id the worker
// writes to stdin, so the test can answer with a matching id.
func pipeWorker(t *testing.T) (*PWWorker, *io.PipeWriter, <-chan string) {
	t.Helper()
	stdoutR, stdoutW := io.Pipe()
	stdinR, stdinW := io.Pipe()
	w := &PWWorker{
		stdin:   stdinW,
		stdoutR: bufio.NewReaderSize(stdoutR, 1<<20),
		lines:   make(chan readLine, 8),
	}
	go w.readLoop()

	reqIDs := make(chan string, 16)
	go func() {
		sc := bufio.NewScanner(stdinR)
		sc.Buffer(make([]byte, 1<<20), 1<<20)
		for sc.Scan() {
			var req pwRequest
			if json.Unmarshal(sc.Bytes(), &req) == nil {
				reqIDs <- req.ID
			}
		}
	}()
	t.Cleanup(func() { _ = stdoutW.Close(); _ = stdinW.Close() })
	return w, stdoutW, reqIDs
}

func writeResp(t *testing.T, pw *io.PipeWriter, id string, ok bool) {
	t.Helper()
	b, _ := json.Marshal(pwResponse{ID: id, OK: ok, Result: json.RawMessage(`{"result":"x"}`)})
	b = append(b, '\n')
	if _, err := pw.Write(b); err != nil {
		t.Errorf("pipe write: %v", err)
	}
}

// F2 — a call that TIMES OUT must not leak a goroutine racing the shared
// reader, and its late response must not be stolen by the next call. Run
// under -race: pre-fix (a per-call goroutine on the shared bufio.Reader)
// this trips the race detector and/or hands the next call the wrong data.
func TestPWReaderNoLeakNoStolenResponse(t *testing.T) {
	w, stdoutW, reqIDs := pipeWorker(t)

	// Call 1 times out (no response fed within the window).
	if _, err := w.call("slow", nil, 150*time.Millisecond); err == nil {
		t.Fatal("expected call 1 to time out")
	}
	<-reqIDs // consume call 1's id

	// Drive call 2 concurrently.
	var wg sync.WaitGroup
	wg.Add(1)
	var call2Res json.RawMessage
	var call2Err error
	go func() {
		defer wg.Done()
		call2Res, call2Err = w.call("fast", nil, 3*time.Second)
	}()

	// Feed a STALE late response for call 1 first (wrong id for call 2) …
	writeResp(t, stdoutW, "STALE_ID_FROM_CALL1", true)
	// … then the real response for call 2, keyed on the id it just sent.
	id2 := <-reqIDs
	writeResp(t, stdoutW, id2, true)

	wg.Wait()
	if call2Err != nil {
		t.Fatalf("call 2 failed (stale response stolen or race): %v", call2Err)
	}
	if string(call2Res) != `{"result":"x"}` {
		t.Fatalf("call 2 got wrong data: %s", call2Res)
	}
}
