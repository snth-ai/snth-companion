package browser

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"regexp"
	"strings"
	"time"
)

// Actions — the verbs the remote_browser tool exposes. Each action
// resolves the current attached target internally; callers don't
// need to manage CDP sessions. The Session field is what enables
// find-or-reconnect on every call.

// Navigate tells the active page to go to url + waits for the load
// event. Returns the final URL (after any redirects).
func Navigate(ctx context.Context, c *CDP, url string) (string, error) {
	var resp struct {
		FrameID  string `json:"frameId"`
		LoaderID string `json:"loaderId"`
		Err      string `json:"errorText,omitempty"`
	}
	if err := c.Send(ctx, "Page.enable", nil, nil); err != nil {
		return "", fmt.Errorf("Page.enable: %w", err)
	}
	if err := c.Send(ctx, "Page.navigate", map[string]any{"url": url}, &resp); err != nil {
		return "", fmt.Errorf("Page.navigate: %w", err)
	}
	if resp.Err != "" {
		return "", fmt.Errorf("navigation error: %s", resp.Err)
	}
	// Wait for Page.loadEventFired (or a short timeout — most SPAs
	// don't fire this reliably, so we also bail after 15 s and let
	// snapshot reveal the state).
	_ = WaitForLoad(ctx, c, 15*time.Second)
	// After navigation the snapshot refs are stale; clear them.
	if err := enableRuntime(ctx, c); err == nil {
		_ = c.Send(ctx, "Runtime.evaluate", map[string]any{
			"expression":    "window._snth_refs = [];",
			"returnByValue": true,
		}, nil)
	}
	return currentURL(ctx, c), nil
}

// WaitForLoad blocks until Page.loadEventFired or deadline. Errors
// are swallowed — load events on SPAs are unreliable and snapshot
// is authoritative.
func WaitForLoad(ctx context.Context, c *CDP, timeout time.Duration) error {
	doneCh := make(chan struct{}, 1)
	c.On(func(method string, params json.RawMessage) {
		if method == "Page.loadEventFired" {
			select {
			case doneCh <- struct{}{}:
			default:
			}
		}
	})
	select {
	case <-doneCh:
		return nil
	case <-time.After(timeout):
		return fmt.Errorf("loadEventFired timeout after %s", timeout)
	case <-ctx.Done():
		return ctx.Err()
	}
}

func currentURL(ctx context.Context, c *CDP) string {
	var resp evalResult
	_ = c.Send(ctx, "Runtime.evaluate", map[string]any{
		"expression":    "location.href",
		"returnByValue": true,
	}, &resp)
	if s, ok := resp.Result.Value.(string); ok {
		return s
	}
	return ""
}

// Click clicks the element at snapshot-ref `ref`. We re-enter the
// page's JS context, look up window._snth_refs[ref-1], compute its
// centre, and dispatch a pair of mousedown/mouseup events via CDP
// Input.dispatchMouseEvent (which cleanly triggers real onclick
// handlers, unlike just calling .click()).
func Click(ctx context.Context, c *CDP, ref int) error {
	x, y, err := refCentre(ctx, c, ref)
	if err != nil {
		return err
	}
	return clickAt(ctx, c, x, y)
}

// Type focuses the element at `ref` and sends `text`. For inputs we
// also set .value first so clearing is simple (caller passes the
// desired final value; pass "" to clear).
func Type(ctx context.Context, c *CDP, ref int, text string) error {
	if err := focusRef(ctx, c, ref); err != nil {
		return err
	}
	// Clear existing value if the element supports it.
	_ = c.Send(ctx, "Runtime.evaluate", map[string]any{
		"expression": fmt.Sprintf(`
(() => {
  const el = window._snth_refs[%d - 1];
  if (!el) return;
  if ('value' in el) {
    el.value = '';
    el.dispatchEvent(new Event('input', {bubbles: true}));
  } else if (el.isContentEditable) {
    el.textContent = '';
  }
})();`, ref),
		"returnByValue": true,
	}, nil)
	// Insert the new text. Input.insertText dispatches real input
	// events so React/Vue/etc. notice.
	if err := c.Send(ctx, "Input.insertText", map[string]any{"text": text}, nil); err != nil {
		return fmt.Errorf("Input.insertText: %w", err)
	}
	return nil
}

// Press dispatches a named key press (Enter, Tab, Escape, ArrowDown,
// etc). Used after Type when a form wants a submission.
func Press(ctx context.Context, c *CDP, key string) error {
	if err := c.Send(ctx, "Input.dispatchKeyEvent", map[string]any{
		"type": "keyDown",
		"key":  key,
	}, nil); err != nil {
		return err
	}
	return c.Send(ctx, "Input.dispatchKeyEvent", map[string]any{
		"type": "keyUp",
		"key":  key,
	}, nil)
}

// Screenshot returns a PNG of the viewport as base64. format can be
// "png" (default) or "jpeg".
func Screenshot(ctx context.Context, c *CDP, format string) (string, string, error) {
	if format == "" {
		format = "png"
	}
	if err := c.Send(ctx, "Page.enable", nil, nil); err != nil {
		return "", "", err
	}
	var resp struct {
		Data string `json:"data"`
	}
	params := map[string]any{"format": format}
	if format == "jpeg" {
		params["quality"] = 80
	}
	if err := c.Send(ctx, "Page.captureScreenshot", params, &resp); err != nil {
		return "", "", fmt.Errorf("Page.captureScreenshot: %w", err)
	}
	// Sanity: make sure it's decodable.
	if _, err := base64.StdEncoding.DecodeString(resp.Data); err != nil {
		return "", "", fmt.Errorf("captureScreenshot returned garbage: %w", err)
	}
	return resp.Data, format, nil
}

// EvalJS runs an arbitrary expression in page context and returns
// the stringified result. Power-user escape hatch; most synth flows
// should use snapshot → click → type instead.
func EvalJS(ctx context.Context, c *CDP, expr string) (string, error) {
	if err := enableRuntime(ctx, c); err != nil {
		return "", err
	}
	var resp evalResult
	if err := c.Send(ctx, "Runtime.evaluate", map[string]any{
		"expression":    expr,
		"returnByValue": true,
		"awaitPromise":  true,
	}, &resp); err != nil {
		return "", fmt.Errorf("Runtime.evaluate: %w", err)
	}
	if resp.ExceptionDetails != nil {
		return "", fmt.Errorf("eval exception: %s", resp.ExceptionDetails.Text)
	}
	if resp.Result.Value == nil {
		return "", nil
	}
	switch v := resp.Result.Value.(type) {
	case string:
		return v, nil
	default:
		return fmt.Sprintf("%v", v), nil
	}
}

// WaitForURL polls location.href against the given regex until it
// matches or timeout. Returns the matching URL.
func WaitForURL(ctx context.Context, c *CDP, pattern string, timeout time.Duration) (string, error) {
	re, err := regexp.Compile(pattern)
	if err != nil {
		return "", fmt.Errorf("bad pattern: %w", err)
	}
	deadline := time.Now().Add(timeout)
	for time.Now().Before(deadline) {
		u := currentURL(ctx, c)
		if re.MatchString(u) {
			return u, nil
		}
		select {
		case <-ctx.Done():
			return "", ctx.Err()
		case <-time.After(300 * time.Millisecond):
		}
	}
	return "", fmt.Errorf("timeout after %s waiting for URL %s", timeout, pattern)
}

// WaitForJS polls an arbitrary JS predicate. Predicate should return
// truthy when the page is ready. Timeout bounded.
func WaitForJS(ctx context.Context, c *CDP, predicate string, timeout time.Duration) error {
	if err := enableRuntime(ctx, c); err != nil {
		return err
	}
	expr := "(() => { try { return !!(" + predicate + "); } catch (e) { return false; } })()"
	deadline := time.Now().Add(timeout)
	for time.Now().Before(deadline) {
		var resp evalResult
		_ = c.Send(ctx, "Runtime.evaluate", map[string]any{
			"expression":    expr,
			"returnByValue": true,
		}, &resp)
		if v, ok := resp.Result.Value.(bool); ok && v {
			return nil
		}
		select {
		case <-ctx.Done():
			return ctx.Err()
		case <-time.After(300 * time.Millisecond):
		}
	}
	return fmt.Errorf("timeout after %s waiting for predicate", timeout)
}

// --- low-level helpers ------------------------------------------------------

// refCentre resolves a snapshot ref to its element's viewport-centre
// coordinates, scrolling into view if necessary.
func refCentre(ctx context.Context, c *CDP, ref int) (x, y float64, err error) {
	if err := enableRuntime(ctx, c); err != nil {
		return 0, 0, err
	}
	expr := fmt.Sprintf(`
(() => {
  const el = window._snth_refs[%d - 1];
  if (!el) return null;
  el.scrollIntoView({block: 'center', inline: 'center'});
  const r = el.getBoundingClientRect();
  return JSON.stringify({x: r.left + r.width/2, y: r.top + r.height/2});
})();`, ref)
	var resp evalResult
	if err := c.Send(ctx, "Runtime.evaluate", map[string]any{
		"expression":    expr,
		"returnByValue": true,
	}, &resp); err != nil {
		return 0, 0, err
	}
	if resp.ExceptionDetails != nil {
		return 0, 0, fmt.Errorf("refCentre exception: %s", resp.ExceptionDetails.Text)
	}
	s, ok := resp.Result.Value.(string)
	if !ok || s == "" {
		return 0, 0, fmt.Errorf("ref %d not found — snapshot may be stale; re-snapshot", ref)
	}
	var coord struct {
		X float64 `json:"x"`
		Y float64 `json:"y"`
	}
	if err := jsonUnmarshal(s, &coord); err != nil {
		return 0, 0, err
	}
	return coord.X, coord.Y, nil
}

func clickAt(ctx context.Context, c *CDP, x, y float64) error {
	for _, t := range []string{"mousePressed", "mouseReleased"} {
		if err := c.Send(ctx, "Input.dispatchMouseEvent", map[string]any{
			"type":       t,
			"x":          x,
			"y":          y,
			"button":     "left",
			"clickCount": 1,
		}, nil); err != nil {
			return fmt.Errorf("dispatchMouseEvent %s: %w", t, err)
		}
	}
	return nil
}

func focusRef(ctx context.Context, c *CDP, ref int) error {
	if err := enableRuntime(ctx, c); err != nil {
		return err
	}
	expr := fmt.Sprintf(`
(() => {
  const el = window._snth_refs[%d - 1];
  if (!el) return "NOT_FOUND";
  el.focus();
  return "OK";
})();`, ref)
	var resp evalResult
	if err := c.Send(ctx, "Runtime.evaluate", map[string]any{
		"expression":    expr,
		"returnByValue": true,
	}, &resp); err != nil {
		return err
	}
	if s, _ := resp.Result.Value.(string); s != "OK" {
		return fmt.Errorf("focus ref %d: %s (snapshot may be stale)", ref, s)
	}
	return nil
}

// jsonUnmarshal hides the stdlib import for call-sites that don't
// need their own encoding/json. Kept here to keep actions.go's
// import list symmetrical with snapshot.go's approach.
func jsonUnmarshal(raw string, out any) error {
	return json.Unmarshal([]byte(raw), out)
}

// Stubs to keep imports alive when tool grows.
var _ = strings.TrimSpace
var _ = quoteJSString
