package browser

// playwright_worker.go — Go side of the Playwright Node worker.
//
// On first browser-tool call we spawn `node worker.js` and keep it
// alive for the life of the companion process. Each request is a JSON
// line on the worker's stdin; each response is a JSON line on stdout.
// Mutex-serialized — Playwright is happy with one in-flight call at a
// time per page, and we don't gain anything from concurrency since
// Mia issues one tool call at a time.
//
// The action surface is intentionally tiny: `eval`, `eval_bundle`,
// `goto`, `press`, `screenshot`, `wait_*`, `tabs`, `version`. Anything
// DOM-shaped (click, type, ref resolution) is just `eval` with the
// same JS the CDP backend already uses, so we don't maintain DOM
// logic in two places.
//
// Why a Node subprocess and not playwright-go: playwright-go bundles
// its own Node driver under the hood, lags upstream by 2-4 months,
// and adds a Go-binding maintenance surface. Mac already has Node;
// `npx playwright install chromium` ships a Chromium binary the user
// runs once. Same shell-out pattern as remote_yt_dlp.

import (
	"bufio"
	"context"
	"crypto/rand"
	_ "embed"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"sync"
	"time"
)

//go:embed playwright/worker.js
var playwrightWorkerJS []byte

// Stable script path materialized on first run. We write the embedded
// worker.js out to disk so node can require("playwright") relative to
// wherever Playwright is installed.

// pwRequest / pwResponse — wire types matching worker.js exactly.
type pwRequest struct {
	ID     string         `json:"id"`
	Action string         `json:"action"`
	Args   map[string]any `json:"args,omitempty"`
}

type pwResponse struct {
	ID     string          `json:"id"`
	OK     bool            `json:"ok"`
	Result json.RawMessage `json:"result,omitempty"`
	Error  string          `json:"error,omitempty"`
}

// readLine is one line off the worker's stdout, or the terminal read error.
type readLine struct {
	line []byte
	err  error
}

// PWWorker is the long-lived Node child process. Zero-value invalid;
// use DefaultPWWorker.
type PWWorker struct {
	mu      sync.Mutex // serializes one request at a time
	cmd     *exec.Cmd
	stdin   io.WriteCloser
	stdoutR *bufio.Reader
	bootErr error

	// lines is fed by ONE long-lived reader goroutine (readLoop). F2: the
	// old code spawned a fresh goroutine on the shared bufio.Reader per
	// call and abandoned it on timeout — the next call then had TWO
	// goroutines racing ReadBytes on the same Reader (data race + a late
	// response stolen by the wrong caller). A single reader owns the
	// Reader; callers select on this channel with a timeout, so a late
	// line is still consumed here (by the next call's read) rather than
	// leaking a blocked goroutine.
	lines   chan readLine
	readErr error // sticky terminal read error (set by readLoop)
}

// readLoop is the SOLE reader of stdoutR. It runs for the life of the
// worker, pushing every line (and the final error) onto w.lines.
func (w *PWWorker) readLoop() {
	for {
		line, err := w.stdoutR.ReadBytes('\n')
		if len(line) > 0 {
			w.lines <- readLine{line: line}
		}
		if err != nil {
			w.lines <- readLine{err: err}
			return
		}
	}
}

var (
	defaultPWWorker     *PWWorker
	defaultPWWorkerOnce sync.Once
)

// DefaultPWWorker returns the package-global Playwright worker. Lazy
// init — first call spawns the Node process. Idempotent + safe across
// goroutines.
func DefaultPWWorker() (*PWWorker, error) {
	defaultPWWorkerOnce.Do(func() {
		w, err := newPWWorker()
		if err != nil {
			defaultPWWorker = &PWWorker{bootErr: err}
			return
		}
		defaultPWWorker = w
	})
	if defaultPWWorker.bootErr != nil {
		return nil, defaultPWWorker.bootErr
	}
	return defaultPWWorker, nil
}

func newPWWorker() (*PWWorker, error) {
	home, err := os.UserHomeDir()
	if err != nil {
		return nil, fmt.Errorf("home dir: %w", err)
	}
	scriptDir := filepath.Join(home, "Library", "Application Support",
		"snth-companion", "playwright")
	if err := os.MkdirAll(scriptDir, 0o755); err != nil {
		return nil, fmt.Errorf("mkdir %s: %w", scriptDir, err)
	}
	scriptPath := filepath.Join(scriptDir, "worker.js")
	if err := os.WriteFile(scriptPath, playwrightWorkerJS, 0o644); err != nil {
		return nil, fmt.Errorf("write worker.js: %w", err)
	}

	// node lookup. launchd-started companions inherit only /usr/bin:/bin,
	// so we explicitly check brew paths.
	nodeBin := ""
	for _, candidate := range []string{
		"/opt/homebrew/bin/node",
		"/usr/local/bin/node",
	} {
		if st, err := os.Stat(candidate); err == nil && !st.IsDir() {
			nodeBin = candidate
			break
		}
	}
	if nodeBin == "" {
		if found, err := exec.LookPath("node"); err == nil {
			nodeBin = found
		}
	}
	if nodeBin == "" {
		return nil, fmt.Errorf(
			"node not found — install Node + Playwright:\n  brew install node\n  npx playwright install chromium",
		)
	}

	// Augment PATH so npx / playwright dependencies can find their
	// bundled binaries (Chromium under ~/Library/Caches/ms-playwright).
	pathEnv := os.Getenv("PATH")
	for _, p := range []string{"/opt/homebrew/bin", "/usr/local/bin"} {
		if pathEnv == "" {
			pathEnv = p
		} else {
			pathEnv = pathEnv + ":" + p
		}
	}

	cmd := exec.Command(nodeBin, scriptPath)
	cmd.Env = append(os.Environ(), "PATH="+pathEnv)
	stdin, err := cmd.StdinPipe()
	if err != nil {
		return nil, fmt.Errorf("stdin pipe: %w", err)
	}
	stdout, err := cmd.StdoutPipe()
	if err != nil {
		return nil, fmt.Errorf("stdout pipe: %w", err)
	}
	cmd.Stderr = os.Stderr
	if err := cmd.Start(); err != nil {
		return nil, fmt.Errorf("start node: %w (probably missing playwright — run `npx playwright install chromium`)", err)
	}

	w := &PWWorker{
		cmd:     cmd,
		stdin:   stdin,
		stdoutR: bufio.NewReaderSize(stdout, 4<<20), // 4MB — screenshots can be big
		lines:   make(chan readLine, 8),
	}
	go w.readLoop()

	// Wait for the "ready" greeting.
	resp, err := w.readResponse(20 * time.Second)
	if err != nil {
		_ = cmd.Process.Kill()
		return nil, fmt.Errorf("worker boot: %w", err)
	}
	if resp.ID != "ready" || !resp.OK {
		_ = cmd.Process.Kill()
		return nil, fmt.Errorf("worker boot bad greeting: %+v", resp)
	}
	return w, nil
}

func (w *PWWorker) call(action string, args map[string]any, timeout time.Duration) (json.RawMessage, error) {
	if w == nil {
		return nil, errors.New("worker not initialized")
	}
	w.mu.Lock()
	defer w.mu.Unlock()

	id := randID()
	req := pwRequest{ID: id, Action: action, Args: args}
	buf, err := json.Marshal(req)
	if err != nil {
		return nil, fmt.Errorf("marshal: %w", err)
	}
	buf = append(buf, '\n')
	if _, err := w.stdin.Write(buf); err != nil {
		return nil, fmt.Errorf("write stdin: %w", err)
	}

	// Read until we see the response for THIS request. A previous call that
	// timed out may have left its (late) response queued on w.lines; skip
	// those stale IDs instead of failing on a mismatch or, worse, returning
	// another call's data. Bounded by the overall timeout via a deadline.
	deadline := time.Now().Add(timeout)
	for {
		remaining := time.Until(deadline)
		if remaining <= 0 {
			return nil, fmt.Errorf("worker timeout after %s", timeout)
		}
		resp, err := w.readResponse(remaining)
		if err != nil {
			return nil, err
		}
		if resp.ID != id {
			// Stale response from an earlier timed-out call — drop it.
			continue
		}
		if !resp.OK {
			return nil, errors.New(resp.Error)
		}
		return resp.Result, nil
	}
}

// readResponse waits up to timeout for the next line from the shared
// reader goroutine. On timeout it returns WITHOUT leaking a goroutine —
// the reader keeps running and the (late) line will be delivered to the
// next readResponse caller. Callers hold w.mu, so at most one readResponse
// is in flight, guaranteeing the late line reaches the right owner.
func (w *PWWorker) readResponse(timeout time.Duration) (*pwResponse, error) {
	if w.readErr != nil {
		return nil, fmt.Errorf("worker closed: %w", w.readErr)
	}
	select {
	case got := <-w.lines:
		if got.err != nil {
			w.readErr = got.err
			return nil, fmt.Errorf("read stdout: %w", got.err)
		}
		var resp pwResponse
		if err := json.Unmarshal(got.line, &resp); err != nil {
			return nil, fmt.Errorf("parse response: %w (head: %.200s)", err, got.line)
		}
		return &resp, nil
	case <-time.After(timeout):
		return nil, fmt.Errorf("worker timeout after %s", timeout)
	}
}

func randID() string {
	var b [6]byte
	_, _ = rand.Read(b[:])
	return hex.EncodeToString(b[:])
}

// --- High-level wrappers (mirror the existing CDP-shaped functions) ---

// PWEval is the public wrapper around pwEval — used by tools/browser.go
// for the "eval" action.
func PWEval(ctx context.Context, expr string) (string, error) {
	return pwEval(ctx, expr)
}

// PWPredicateOnce evaluates a JS predicate ONCE and returns whether it
// was truthy. Used by tools/browser.go's "wait predicate" path as a
// poll loop. (Worker doesn't expose Playwright's waitForFunction yet;
// simple poll is good enough for our usage.)
func PWPredicateOnce(ctx context.Context, predicate string) (bool, error) {
	expr := fmt.Sprintf("(() => !!(%s))()", predicate)
	r, err := pwEval(ctx, expr)
	if err != nil {
		return false, err
	}
	// pwEval returns JSON-stringified result; "true" means truthy.
	return r == "true", nil
}

// pwEval runs a JS expression in the active page. Returns the
// JSON-stringified result (Playwright auto-encodes return values).
func pwEval(ctx context.Context, expr string) (string, error) {
	w, err := DefaultPWWorker()
	if err != nil {
		return "", err
	}
	r, err := w.call("eval", map[string]any{"expr": expr}, 30*time.Second)
	if err != nil {
		return "", err
	}
	var out struct {
		Result string `json:"result"`
	}
	if err := json.Unmarshal(r, &out); err != nil {
		return "", fmt.Errorf("parse eval: %w", err)
	}
	return out.Result, nil
}

// pwEvalBundle runs a self-invoking JS bundle that returns a string
// (typically JSON.stringify'd). Used for the snapshot path.
func pwEvalBundle(ctx context.Context, bundle string) (string, error) {
	w, err := DefaultPWWorker()
	if err != nil {
		return "", err
	}
	r, err := w.call("eval_bundle", map[string]any{"bundle": bundle}, 60*time.Second)
	if err != nil {
		return "", err
	}
	var out struct {
		Raw string `json:"raw"`
	}
	if err := json.Unmarshal(r, &out); err != nil {
		return "", fmt.Errorf("parse eval_bundle: %w", err)
	}
	return out.Raw, nil
}

// PWNavigate navigates the active page.
func PWNavigate(ctx context.Context, url string) (string, error) {
	w, err := DefaultPWWorker()
	if err != nil {
		return "", err
	}
	r, err := w.call("goto", map[string]any{"url": url}, 60*time.Second)
	if err != nil {
		return "", err
	}
	var out struct {
		FinalURL string `json:"final_url"`
	}
	_ = json.Unmarshal(r, &out)
	return out.FinalURL, nil
}

// pwFindByHighlightJS builds a JS snippet that scans window.__snth_tree.map
// for the node whose highlightIndex === n and binds it to `node`, returning
// "NOT_FOUND" when absent. F1: the map is keyed by sequential DOM ids over
// ALL nodes (incl. text nodes), NOT by highlightIndex — so map[N] was the
// wrong node (or a text node with no .ref). We must scan for the
// highlightIndex the same way the CDP backend does (actions.go:270).
func pwFindByHighlightJS(n int) string {
	return fmt.Sprintf(`
  let node = null;
  const __tree = window.__snth_tree;
  if (__tree && __tree.map) {
    for (const __id of Object.keys(__tree.map)) {
      const __n = __tree.map[__id];
      if (__n && __n.highlightIndex === %d) { node = __n.ref; break; }
    }
  }`, n)
}

// PWClick re-uses the same highlightIndex scan as the CDP backend to
// resolve the ref, then dispatches a click on the resolved DOM node. The
// snapshot must have happened first to populate the tree.
func PWClick(ctx context.Context, ref int) error {
	expr := fmt.Sprintf(
		`(() => {%s if (!node) return "NOT_FOUND"; node.scrollIntoView({block:"center",inline:"center"}); node.click(); return "OK"; })()`,
		pwFindByHighlightJS(ref),
	)
	r, err := pwEval(ctx, expr)
	if err != nil {
		return err
	}
	if r == `"NOT_FOUND"` || r == "NOT_FOUND" {
		return fmt.Errorf("ref %d not found in snapshot — re-snapshot and try again", ref)
	}
	return nil
}

// PWType fills a form field at ref with text. Uses native input event
// dispatch so React/Vue controlled inputs see the value (page-agent
// path does the same).
func PWType(ctx context.Context, ref int, text string) error {
	// Embed the text via JSON encoding so quotes/backslashes are safe.
	textJSON, _ := json.Marshal(text)
	expr := fmt.Sprintf(`(() => {%s
  if (!node) return "NOT_FOUND";
  node.focus();
  if (node.tagName === "INPUT" || node.tagName === "TEXTAREA") {
    const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, "value")?.set
      || Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, "value")?.set;
    if (setter) setter.call(node, %s);
    else node.value = %s;
    node.dispatchEvent(new Event("input", { bubbles: true }));
    node.dispatchEvent(new Event("change", { bubbles: true }));
  } else if (node.isContentEditable) {
    node.textContent = %s;
    node.dispatchEvent(new Event("input", { bubbles: true }));
  } else {
    return "NOT_AN_INPUT";
  }
  return "OK";
})()`, pwFindByHighlightJS(ref), string(textJSON), string(textJSON), string(textJSON))
	r, err := pwEval(ctx, expr)
	if err != nil {
		return err
	}
	if r == `"NOT_FOUND"` {
		return fmt.Errorf("ref %d not found — re-snapshot", ref)
	}
	if r == `"NOT_AN_INPUT"` {
		return fmt.Errorf("ref %d is not an input/textarea/contenteditable", ref)
	}
	return nil
}

// PWPress sends a key press to the focused element.
func PWPress(ctx context.Context, key string) error {
	w, err := DefaultPWWorker()
	if err != nil {
		return err
	}
	_, err = w.call("press", map[string]any{"key": key}, 5*time.Second)
	return err
}

// PWSnapshot uses the SAME bundled snapshot script the CDP path uses.
// snapshot.go's wrapper.js + dom_tree.js bundle is built with the
// existing helper exposed by snapshot.go (snapshotBundle). Returns the
// same SnapshotResult shape so the tool layer is backend-agnostic.
func PWSnapshot(ctx context.Context) (*SnapshotResult, error) {
	bundle := buildSnapshotBundle()
	raw, err := pwEvalBundle(ctx, bundle)
	if err != nil {
		return nil, fmt.Errorf("snapshot eval: %w", err)
	}
	var tree flatTree
	if err := json.Unmarshal([]byte(raw), &tree); err != nil {
		return nil, fmt.Errorf("snapshot decode: %w (raw head: %s)", err, head(raw, 200))
	}
	if tree.Err != "" {
		return nil, fmt.Errorf("snapshot JS error: %s", tree.Err)
	}
	text, selectors := formatTree(&tree)
	return &SnapshotResult{
		Title:     tree.Title,
		URL:       tree.URL,
		Text:      text,
		Selectors: selectors,
	}, nil
}

// PWScreenshot returns base64-encoded PNG/JPEG of the viewport.
func PWScreenshot(ctx context.Context, format string) (string, string, error) {
	w, err := DefaultPWWorker()
	if err != nil {
		return "", "", err
	}
	r, err := w.call("screenshot", map[string]any{"format": format}, 30*time.Second)
	if err != nil {
		return "", "", err
	}
	var out struct {
		Data   string `json:"data_base64"`
		Format string `json:"format"`
	}
	if err := json.Unmarshal(r, &out); err != nil {
		return "", "", fmt.Errorf("parse screenshot: %w", err)
	}
	return out.Data, out.Format, nil
}

// PWWaitURL blocks until the page URL matches pattern.
func PWWaitURL(ctx context.Context, pattern string, timeout time.Duration) (string, error) {
	w, err := DefaultPWWorker()
	if err != nil {
		return "", err
	}
	r, err := w.call("wait_url", map[string]any{
		"pattern":    pattern,
		"timeout_ms": int(timeout / time.Millisecond),
	}, timeout+10*time.Second)
	if err != nil {
		return "", err
	}
	var out struct {
		URL string `json:"url"`
	}
	_ = json.Unmarshal(r, &out)
	return out.URL, nil
}

// PWWaitLoad blocks until the page emits load.
func PWWaitLoad(ctx context.Context, timeout time.Duration) error {
	w, err := DefaultPWWorker()
	if err != nil {
		return err
	}
	_, err = w.call("wait_load", map[string]any{
		"timeout_ms": int(timeout / time.Millisecond),
	}, timeout+10*time.Second)
	return err
}

// PWTabs returns open tabs as []Target — same shape the CDP path uses
// so tools/browser.go switch logic can be backend-agnostic.
func PWTabs(ctx context.Context) ([]Target, error) {
	w, err := DefaultPWWorker()
	if err != nil {
		return nil, err
	}
	r, err := w.call("tabs", nil, 5*time.Second)
	if err != nil {
		return nil, err
	}
	var out struct {
		Tabs []struct {
			URL   string `json:"url"`
			Title string `json:"title"`
		} `json:"tabs"`
	}
	if err := json.Unmarshal(r, &out); err != nil {
		return nil, fmt.Errorf("parse tabs: %w", err)
	}
	tabs := make([]Target, 0, len(out.Tabs))
	for _, t := range out.Tabs {
		tabs = append(tabs, Target{
			Type:  "page",
			URL:   t.URL,
			Title: t.Title,
		})
	}
	return tabs, nil
}

// PWVersion returns identity info — used by the "version" action.
func PWVersion(ctx context.Context) (map[string]any, error) {
	w, err := DefaultPWWorker()
	if err != nil {
		return nil, err
	}
	r, err := w.call("version", nil, 3*time.Second)
	if err != nil {
		return nil, err
	}
	var out map[string]any
	if err := json.Unmarshal(r, &out); err != nil {
		return nil, err
	}
	return out, nil
}
