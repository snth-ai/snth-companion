package tools

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"

	"github.com/snth-ai/snth-companion/internal/approval"
)

// messages.go — Apple Messages (iMessage/SMS) via two paths.
//
// Send: `tell application "Messages"` in AppleScript. Works without
// Full Disk Access — the Messages app itself is the authorised agent.
// Handles are phone numbers ("+15551234567") or iMessage emails.
//
// Read: direct sqlite3 query against ~/Library/Messages/chat.db. This
// path does require Full Disk Access granted to whatever is spawning
// `sqlite3` — the companion binary (or the shell it was launched
// from). If FDA isn't granted, sqlite3 will fail with a "unable to
// open database" error and we surface a clear message pointing the
// user at System Settings → Privacy & Security → Full Disk Access.
//
// We deliberately don't try to read via AppleScript — Messages.app
// exposes very little inspection surface through its dictionary, and
// what it does expose is unreliable (phantom buddy objects,
// stale group data). Direct DB read is what every other integration
// (pi-ai, openclaw) settles on.

func RegisterMessages() {
	Register(Descriptor{
		Name:        "remote_messages_send",
		Description: "Send an iMessage or SMS from the paired Mac via the Messages app. `to` is a phone number (E.164) or iMessage email. Prompts for user approval on every send (messages are visible externally).",
		DangerLevel: "always-prompt",
	}, messagesSendHandler)
	Register(Descriptor{
		Name:        "remote_messages_recent",
		Description: "Read recent messages from the paired Mac's Messages history. Requires Full Disk Access granted to the shell/app that launched the companion. Prompts for user approval.",
		DangerLevel: "always-prompt",
	}, messagesRecentHandler)
}

// --- send --------------------------------------------------------------------

type messagesSendArgs struct {
	To   string `json:"to"`
	Text string `json:"text"`
	// Service hints Messages which protocol to prefer: "iMessage"
	// or "SMS". Empty lets Messages decide (usually iMessage if
	// handle is known to iCloud, falls back to SMS).
	Service string `json:"service,omitempty"`
}

func messagesSendHandler(ctx context.Context, raw json.RawMessage) (any, error) {
	var a messagesSendArgs
	if err := json.Unmarshal(raw, &a); err != nil {
		return nil, fmt.Errorf("bad args: %w", err)
	}
	a.To = strings.TrimSpace(a.To)
	a.Text = strings.TrimRight(a.Text, "\n")
	if a.To == "" || a.Text == "" {
		return nil, fmt.Errorf("to and text are required")
	}

	preview := a.Text
	if len(preview) > 180 {
		preview = preview[:180] + "…"
	}
	summary := fmt.Sprintf("Send iMessage to %s:\n    %s", a.To, preview)
	ok, err := approval.Request(ctx, approval.Request_{Summary: summary, Danger: "always-prompt"})
	if err != nil {
		return nil, fmt.Errorf("approval: %w", err)
	}
	if !ok {
		return nil, fmt.Errorf("user denied")
	}

	service := `"iMessage"`
	if strings.EqualFold(a.Service, "SMS") {
		service = `"SMS"`
	}

	src := fmt.Sprintf(`
tell application "Messages"
    set svc to first service whose service type is %s
    set buddyRef to buddy %s of svc
    send %s to buddyRef
    return "ok"
end tell
`,
		strings.Trim(service, `"`),
		EscapeAppleScriptString(a.To),
		EscapeAppleScriptString(a.Text),
	)
	if _, err := RunAppleScript(ctx, src); err != nil {
		// Common first-run failure: Messages hasn't been launched or
		// the user hasn't granted Automation permission. Surface a
		// hint the LLM can relay.
		return nil, fmt.Errorf("%w — make sure Messages.app is signed in and the companion has Automation permission for Messages (System Settings → Privacy & Security → Automation)", err)
	}
	return map[string]any{"ok": true, "to": a.To, "service": strings.Trim(service, `"`)}, nil
}

// --- recent ------------------------------------------------------------------

type messagesRecentArgs struct {
	Chat  string `json:"chat,omitempty"` // handle (phone/email) or chat_id; empty = all
	Limit int    `json:"limit,omitempty"`
}

type messageRow struct {
	Date      string `json:"date"`    // RFC3339 local
	Text      string `json:"text"`
	From      string `json:"from"`    // handle or "me"
	ChatKey   string `json:"chat_key"` // chat_identifier (handle or iMessage;chat:...)
	IsFromMe  bool   `json:"is_from_me"`
	Service   string `json:"service,omitempty"`
}

func messagesRecentHandler(ctx context.Context, raw json.RawMessage) (any, error) {
	var a messagesRecentArgs
	if err := json.Unmarshal(raw, &a); err != nil {
		return nil, fmt.Errorf("bad args: %w", err)
	}
	if a.Limit <= 0 || a.Limit > 200 {
		a.Limit = 50
	}

	summary := fmt.Sprintf("Read %d recent messages from Messages.app history", a.Limit)
	if a.Chat != "" {
		summary += " (chat: " + a.Chat + ")"
	}
	ok, err := approval.Request(ctx, approval.Request_{Summary: summary, Danger: "always-prompt"})
	if err != nil {
		return nil, fmt.Errorf("approval: %w", err)
	}
	if !ok {
		return nil, fmt.Errorf("user denied")
	}

	home, err := os.UserHomeDir()
	if err != nil {
		return nil, err
	}
	dbPath := filepath.Join(home, "Library", "Messages", "chat.db")
	if _, err := os.Stat(dbPath); err != nil {
		return nil, fmt.Errorf("messages db at %s not readable: %w — grant Full Disk Access to the shell/app that launched the companion (System Settings → Privacy & Security → Full Disk Access)", dbPath, err)
	}

	// chat.db has message.date stored as Apple CFAbsoluteTime * 1e9
	// (nanoseconds since 2001-01-01 UTC). Convert to unix epoch in SQL.
	where := ""
	if a.Chat != "" {
		where = fmt.Sprintf(`WHERE h.id = %s OR c.chat_identifier = %s`,
			sqlQuote(a.Chat), sqlQuote(a.Chat))
	}
	query := fmt.Sprintf(`
SELECT
    strftime('%%Y-%%m-%%dT%%H:%%M:%%SZ', m.date / 1000000000 + 978307200, 'unixepoch') AS ts,
    COALESCE(m.text, '') AS text,
    COALESCE(h.id, 'me') AS handle,
    COALESCE(c.chat_identifier, COALESCE(h.id, '')) AS chat_key,
    m.is_from_me,
    COALESCE(m.service, '') AS service
FROM message m
LEFT JOIN handle h ON m.handle_id = h.ROWID
LEFT JOIN chat_message_join cmj ON cmj.message_id = m.ROWID
LEFT JOIN chat c ON cmj.chat_id = c.ROWID
%s
ORDER BY m.date DESC
LIMIT %d;
`, where, a.Limit)

	cctx, cancel := context.WithTimeout(ctx, 30*time.Second)
	defer cancel()
	cmd := exec.CommandContext(cctx, "sqlite3", "-separator", string(rune(1)), "-nullvalue", "", "file:"+dbPath+"?mode=ro", query)
	var stdout, stderr bytes.Buffer
	cmd.Stdout = &stdout
	cmd.Stderr = &stderr
	if err := cmd.Run(); err != nil {
		msg := strings.TrimSpace(stderr.String())
		if strings.Contains(msg, "unable to open database") || strings.Contains(msg, "authorization denied") {
			return nil, fmt.Errorf("cannot read chat.db — grant Full Disk Access to the shell/app that launched the companion (System Settings → Privacy & Security → Full Disk Access). Raw: %s", msg)
		}
		return nil, fmt.Errorf("sqlite3: %w (stderr: %s)", err, msg)
	}

	var rows []messageRow
	for _, line := range strings.Split(strings.TrimRight(stdout.String(), "\n"), "\n") {
		if line == "" {
			continue
		}
		parts := strings.SplitN(line, string(rune(1)), 6)
		if len(parts) != 6 {
			continue
		}
		rows = append(rows, messageRow{
			Date:     parts[0],
			Text:     parts[1],
			From:     parts[2],
			ChatKey:  parts[3],
			IsFromMe: parts[4] == "1",
			Service:  parts[5],
		})
	}
	return map[string]any{"messages": rows, "chat": a.Chat, "count": len(rows)}, nil
}

// sqlQuote escapes a string for safe literal inclusion in a SQLite
// query. Single-quote + double-up embedded single-quotes. Not a
// general SQL injection shield, just enough for our narrow chat-id
// / handle inputs (alphanumeric + '+ @ . _' in practice).
func sqlQuote(s string) string {
	return "'" + strings.ReplaceAll(s, "'", "''") + "'"
}
