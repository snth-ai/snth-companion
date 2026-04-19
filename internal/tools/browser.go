package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"
	"sync"
	"time"

	"github.com/snth-ai/snth-companion/internal/approval"
	"github.com/snth-ai/snth-companion/internal/browser"
)

// browser.go — one composite tool `remote_browser` with an action
// enum. We chose single-tool over per-verb (remote_browser_navigate,
// remote_browser_click, etc.) for three reasons:
//
//   1. Tool Shed on the synth side already picks small subsets; one
//      meta-tool doesn't crowd the catalog.
//   2. LLMs reason about the browser as a single surface — "open
//      maps, search for the restaurant, click it" is cleaner as a
//      chain of `action`s than 5 distinct tools.
//   3. Matches the convention OpenClaw / Chrome DevTools MCP landed
//      on: one tool, many actions.
//
// Approval gates: navigate / act / type — every call prompts, because
// the agent is touching a real user session. Snapshot / screenshot /
// tabs are safe (read-only).

// sessionSingleton lazily binds the RelayServer + Session. We start
// the relay on first use so a fresh companion can be quiet on 18792
// until someone actually reaches for the browser tool.
var (
	sessionOnce sync.Once
	sessionPtr  *browser.Session
)

func browserSession() *browser.Session {
	sessionOnce.Do(func() {
		relay := browser.NewRelayServer(0) // 0 → default 18792
		if err := relay.Start(); err != nil {
			// Non-fatal: the Session still works via direct
			// --remote-debugging-port attach if the user launched
			// Chrome that way.
			// Log is emitted by relay itself; we just skip binding.
			sessionPtr = browser.NewSession()
			return
		}
		sessionPtr = browser.NewSession().WithRelay(relay)
	})
	return sessionPtr
}

func RegisterBrowser() {
	Register(Descriptor{
		Name:        "remote_browser",
		Description: "Control Chrome on the paired Mac. Chrome must be running with --remote-debugging-port=9222 (see snth-companion README). One tool, many actions: navigate, snapshot, screenshot, click, type, press, wait, tabs, eval. Prompts for user approval on write actions (navigate/click/type/press). Only available when companion is online.",
		DangerLevel: "prompt",
	}, browserHandler)
}

type browserArgs struct {
	Action string `json:"action"`

	URL       string `json:"url,omitempty"`        // navigate
	Ref       int    `json:"ref,omitempty"`        // click / type / press
	Text      string `json:"text,omitempty"`       // type
	Key       string `json:"key,omitempty"`        // press
	Expr      string `json:"expr,omitempty"`       // eval
	Pattern   string `json:"pattern,omitempty"`    // wait (URL regex)
	Predicate string `json:"predicate,omitempty"`  // wait (JS predicate)
	Format    string `json:"format,omitempty"`     // screenshot: png|jpeg
	TimeoutMs int    `json:"timeout_ms,omitempty"` // wait
}

func browserHandler(ctx context.Context, raw json.RawMessage) (any, error) {
	var a browserArgs
	if err := json.Unmarshal(raw, &a); err != nil {
		return nil, fmt.Errorf("bad args: %w", err)
	}
	a.Action = strings.ToLower(strings.TrimSpace(a.Action))
	if a.Action == "" {
		return nil, fmt.Errorf("action is required (navigate|snapshot|screenshot|click|type|press|wait|tabs|eval)")
	}

	sess := browserSession()

	switch a.Action {
	case "tabs":
		targets, err := sess.Targets(ctx)
		if err != nil {
			return nil, err
		}
		pages := make([]browser.Target, 0, len(targets))
		for _, t := range targets {
			if t.Type == "page" {
				pages = append(pages, t)
			}
		}
		return map[string]any{"tabs": pages}, nil

	case "version":
		v, err := sess.Version(ctx)
		if err != nil {
			return nil, err
		}
		return v, nil

	case "snapshot":
		_, c, err := sess.AttachActive(ctx)
		if err != nil {
			return nil, err
		}
		snap, err := browser.Snapshot(ctx, c)
		if err != nil {
			return nil, err
		}
		return snap, nil

	case "screenshot":
		_, c, err := sess.AttachActive(ctx)
		if err != nil {
			return nil, err
		}
		data, format, err := browser.Screenshot(ctx, c, a.Format)
		if err != nil {
			return nil, err
		}
		return map[string]any{"data_base64": data, "format": format}, nil

	case "navigate":
		if a.URL == "" {
			return nil, fmt.Errorf("url required for navigate")
		}
		if err := browserApprove(ctx, fmt.Sprintf("Navigate Chrome to:\n    %s", a.URL)); err != nil {
			return nil, err
		}
		_, c, err := sess.AttachActive(ctx)
		if err != nil {
			return nil, err
		}
		final, err := browser.Navigate(ctx, c, a.URL)
		if err != nil {
			return nil, err
		}
		return map[string]any{"final_url": final}, nil

	case "click":
		// ref 0 is valid (page-agent numbers from 0). The ref-resolver
		// in actions.go returns NOT_FOUND if the snapshot doesn't
		// have that index, so we rely on that for validation rather
		// than a naive zero-check here.
		_ = a.Ref
		if err := browserApprove(ctx, fmt.Sprintf("Click browser element #%d", a.Ref)); err != nil {
			return nil, err
		}
		_, c, err := sess.AttachActive(ctx)
		if err != nil {
			return nil, err
		}
		if err := browser.Click(ctx, c, a.Ref); err != nil {
			return nil, err
		}
		return map[string]any{"ok": true, "ref": a.Ref}, nil

	case "type":
		_ = a.Ref // ref=0 valid; resolver validates
		preview := a.Text
		if len(preview) > 80 {
			preview = preview[:80] + "…"
		}
		if err := browserApprove(ctx, fmt.Sprintf("Type into browser element #%d:\n    %s", a.Ref, preview)); err != nil {
			return nil, err
		}
		_, c, err := sess.AttachActive(ctx)
		if err != nil {
			return nil, err
		}
		if err := browser.Type(ctx, c, a.Ref, a.Text); err != nil {
			return nil, err
		}
		return map[string]any{"ok": true, "ref": a.Ref, "bytes": len(a.Text)}, nil

	case "press":
		if a.Key == "" {
			return nil, fmt.Errorf("key required for press")
		}
		if err := browserApprove(ctx, fmt.Sprintf("Press browser key: %s", a.Key)); err != nil {
			return nil, err
		}
		_, c, err := sess.AttachActive(ctx)
		if err != nil {
			return nil, err
		}
		if err := browser.Press(ctx, c, a.Key); err != nil {
			return nil, err
		}
		return map[string]any{"ok": true, "key": a.Key}, nil

	case "wait":
		_, c, err := sess.AttachActive(ctx)
		if err != nil {
			return nil, err
		}
		timeout := 15 * time.Second
		if a.TimeoutMs > 0 {
			timeout = time.Duration(a.TimeoutMs) * time.Millisecond
		}
		switch {
		case a.Pattern != "":
			u, err := browser.WaitForURL(ctx, c, a.Pattern, timeout)
			if err != nil {
				return nil, err
			}
			return map[string]any{"matched_url": u}, nil
		case a.Predicate != "":
			if err := browser.WaitForJS(ctx, c, a.Predicate, timeout); err != nil {
				return nil, err
			}
			return map[string]any{"ok": true}, nil
		default:
			return nil, fmt.Errorf("wait needs pattern (URL regex) or predicate (JS expression)")
		}

	case "eval":
		if a.Expr == "" {
			return nil, fmt.Errorf("expr required for eval")
		}
		if err := browserApprove(ctx, "Eval arbitrary JS in active tab"); err != nil {
			return nil, err
		}
		_, c, err := sess.AttachActive(ctx)
		if err != nil {
			return nil, err
		}
		out, err := browser.EvalJS(ctx, c, a.Expr)
		if err != nil {
			return nil, err
		}
		return map[string]any{"result": out}, nil

	default:
		return nil, fmt.Errorf("unknown action %q (valid: navigate|snapshot|screenshot|click|type|press|wait|tabs|version|eval)", a.Action)
	}
}

// browserApprove wraps the approval helper with a shorter
// always-prompt variant — browser actions touch the user's real
// session so we default to always prompting.
func browserApprove(ctx context.Context, summary string) error {
	ok, err := approval.Request(ctx, approval.Request_{Summary: summary, Danger: "always-prompt"})
	if err != nil {
		return fmt.Errorf("approval: %w", err)
	}
	if !ok {
		return fmt.Errorf("user denied")
	}
	return nil
}
