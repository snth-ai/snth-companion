package browser

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"
)

// Snapshot returns a compact, LLM-friendly view of the active page:
// a flat numbered list of interactive elements + short text blocks.
// Each entry has a numeric `ref` the agent uses to click, type into,
// or otherwise target the element — we then resolve `ref` back to a
// DOM node via the per-ref script window._snth_refs array inside the
// page.
//
// Why numbered refs over CSS selectors: CSS selectors are brittle
// (break on every Tailwind rebuild), opaque to the LLM, and waste
// tokens. A numbered tree matches OpenClaw / Playwright's best
// practice — the agent says "click 7" and we resolve.
//
// The JS we inject is inlined as a const string — no embed.FS
// needed for one page and it keeps snapshot.go self-contained.

const snapshotJS = `
(() => {
  const refs = [];
  const push = (el, entry) => {
    refs.push(el);
    entry.ref = refs.length;
    return entry;
  };

  const isVisible = (el) => {
    if (!el) return false;
    const rect = el.getBoundingClientRect();
    if (rect.width === 0 || rect.height === 0) return false;
    const style = getComputedStyle(el);
    if (style.visibility === 'hidden' || style.display === 'none' || style.opacity === '0') return false;
    return true;
  };

  const label = (el) => {
    const ariaLabel = el.getAttribute('aria-label');
    if (ariaLabel) return ariaLabel.trim();
    const title = el.getAttribute('title');
    if (title) return title.trim();
    const placeholder = el.getAttribute('placeholder');
    if (placeholder) return placeholder.trim();
    const value = el.value;
    if (value && typeof value === 'string' && value.length < 120) return value.trim();
    const text = (el.textContent || '').trim().replace(/\s+/g, ' ');
    if (text && text.length < 160) return text;
    if (text) return text.slice(0, 160) + '…';
    return '';
  };

  const out = { title: document.title, url: location.href, elements: [] };
  const sel = 'a, button, input:not([type=hidden]), textarea, select, [role=button], [role=link], [role=textbox], [role=checkbox], [role=combobox], [role=menuitem]';
  for (const el of document.querySelectorAll(sel)) {
    if (!isVisible(el)) continue;
    const role = el.getAttribute('role') ||
      (el.tagName === 'A' ? 'link'
      : el.tagName === 'BUTTON' ? 'button'
      : el.tagName === 'INPUT' ? ('input:' + (el.type || 'text'))
      : el.tagName === 'TEXTAREA' ? 'textarea'
      : el.tagName === 'SELECT' ? 'select'
      : el.tagName.toLowerCase());
    const entry = { role, name: label(el) };
    if (el.href) entry.href = el.href;
    if (el.value && typeof el.value === 'string' && el.value.length < 120) entry.value = el.value;
    if (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA') {
      entry.placeholder = el.getAttribute('placeholder') || '';
      entry.disabled = !!el.disabled;
    }
    out.elements.push(push(el, entry));
  }

  // Headings and prominent text — anchor the page so the LLM knows
  // what it's looking at even if the interactive set is sparse.
  for (const el of document.querySelectorAll('h1, h2, h3')) {
    if (!isVisible(el)) continue;
    const text = (el.textContent || '').trim().replace(/\s+/g, ' ');
    if (!text) continue;
    out.elements.push(push(el, { role: el.tagName.toLowerCase(), name: text.slice(0, 200) }));
  }

  window._snth_refs = refs;
  return JSON.stringify(out);
})();
`

// ElementRef is the numbered entry the LLM sees.
type ElementRef struct {
	Ref         int    `json:"ref"`
	Role        string `json:"role"`
	Name        string `json:"name"`
	Href        string `json:"href,omitempty"`
	Value       string `json:"value,omitempty"`
	Placeholder string `json:"placeholder,omitempty"`
	Disabled    bool   `json:"disabled,omitempty"`
}

// SnapshotResult is what we hand back to the tool.
type SnapshotResult struct {
	Title    string       `json:"title"`
	URL      string       `json:"url"`
	Elements []ElementRef `json:"elements"`
}

// Snapshot runs snapshotJS on the attached target, parses the result,
// and also stores the DOM-ref mapping on the page (window._snth_refs)
// so subsequent action calls can resolve `ref → DOM node` without
// re-scanning.
func Snapshot(ctx context.Context, c *CDP) (*SnapshotResult, error) {
	if err := enableRuntime(ctx, c); err != nil {
		return nil, err
	}
	var resp evalResult
	err := c.Send(ctx, "Runtime.evaluate", map[string]any{
		"expression":    snapshotJS,
		"returnByValue": true,
		"awaitPromise":  false,
	}, &resp)
	if err != nil {
		return nil, fmt.Errorf("snapshot eval: %w", err)
	}
	if resp.ExceptionDetails != nil {
		return nil, fmt.Errorf("snapshot eval exception: %s", resp.ExceptionDetails.Text)
	}
	raw, ok := resp.Result.Value.(string)
	if !ok {
		// It might be a JSON object already — try marshaling.
		blob, err := json.Marshal(resp.Result.Value)
		if err != nil {
			return nil, fmt.Errorf("snapshot result: unexpected type %T", resp.Result.Value)
		}
		raw = string(blob)
	}
	var out SnapshotResult
	if err := json.Unmarshal([]byte(raw), &out); err != nil {
		return nil, fmt.Errorf("snapshot decode: %w (raw head: %s)", err, head(raw, 200))
	}
	return &out, nil
}

// evalResult is the subset of Runtime.evaluate's response we care
// about.
type evalResult struct {
	Result struct {
		Type        string      `json:"type"`
		Subtype     string      `json:"subtype,omitempty"`
		Value       interface{} `json:"value,omitempty"`
		Description string      `json:"description,omitempty"`
		ObjectID    string      `json:"objectId,omitempty"`
	} `json:"result"`
	ExceptionDetails *struct {
		Text      string `json:"text"`
		Exception *struct {
			Description string `json:"description"`
		} `json:"exception,omitempty"`
	} `json:"exceptionDetails,omitempty"`
}

var runtimeEnabled = map[string]bool{} // cdp.url → enabled
var runtimeMu = make(chan struct{}, 1)

// enableRuntime issues Runtime.enable once per CDP. Cheap to call
// repeatedly, but we gate it to avoid wasted round-trips on hot path.
func enableRuntime(ctx context.Context, c *CDP) error {
	runtimeMu <- struct{}{}
	defer func() { <-runtimeMu }()
	if runtimeEnabled[c.url] {
		return nil
	}
	if err := c.Send(ctx, "Runtime.enable", nil, nil); err != nil {
		return fmt.Errorf("Runtime.enable: %w", err)
	}
	runtimeEnabled[c.url] = true
	return nil
}

func head(s string, n int) string {
	if len(s) <= n {
		return s
	}
	return s[:n-1] + "…"
}

// quoteJSString escapes a Go string for inclusion inside a JS source
// literal. Used by actions that build one-off JS snippets referencing
// user-supplied text (click-by-text, type value, etc.).
func quoteJSString(s string) string {
	var b strings.Builder
	b.WriteByte('"')
	for _, r := range s {
		switch r {
		case '"':
			b.WriteString(`\"`)
		case '\\':
			b.WriteString(`\\`)
		case '\n':
			b.WriteString(`\n`)
		case '\r':
			b.WriteString(`\r`)
		case '\t':
			b.WriteString(`\t`)
		default:
			if r < 0x20 {
				fmt.Fprintf(&b, `\u%04x`, r)
			} else {
				b.WriteRune(r)
			}
		}
	}
	b.WriteByte('"')
	return b.String()
}
