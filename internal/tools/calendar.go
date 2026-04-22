//go:build darwin

package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"
	"time"

	"github.com/snth-ai/snth-companion/internal/approval"
)

// calendar.go — Apple Calendar via AppleScript.
//
// Three tools: list (today / range), create, search. Each tool reads
// or writes via `tell application "Calendar"`. The AppleScript
// fragments return JSON strings (we build them manually since
// AppleScript has no JSON encoder). Calendar access needs the user
// to grant Automation permission on first invocation; macOS handles
// that prompt automatically. No extra entitlements.
//
// Event IDs are returned as the AppleScript `uid` of each event —
// stable across runs, usable as a key for future update/delete tools.

func RegisterCalendar() {
	Register(Descriptor{
		Name:        "remote_calendar_list",
		Description: "List upcoming or today's events from Apple Calendar on the paired Mac.",
		DangerLevel: "safe",
	}, calendarListHandler)
	Register(Descriptor{
		Name:        "remote_calendar_create",
		Description: "Create a new event in Apple Calendar on the paired Mac. Prompts for user approval.",
		DangerLevel: "prompt",
	}, calendarCreateHandler)
	Register(Descriptor{
		Name:        "remote_calendar_search",
		Description: "Search Apple Calendar events by summary text.",
		DangerLevel: "safe",
	}, calendarSearchHandler)
}

// --- list --------------------------------------------------------------------

type calendarListArgs struct {
	Scope    string `json:"scope,omitempty"`     // "today" (default) | "range"
	Start    string `json:"start,omitempty"`     // RFC3339; required when scope=range
	End      string `json:"end,omitempty"`       // RFC3339; required when scope=range
	Calendar string `json:"calendar,omitempty"`  // optional: filter by calendar name
	Limit    int    `json:"limit,omitempty"`     // default 50
}

type calendarEvent struct {
	UID      string `json:"uid"`
	Title    string `json:"title"`
	Start    string `json:"start"`
	End      string `json:"end"`
	Calendar string `json:"calendar"`
	Location string `json:"location,omitempty"`
	Notes    string `json:"notes,omitempty"`
	AllDay   bool   `json:"all_day"`
}

func calendarListHandler(ctx context.Context, raw json.RawMessage) (any, error) {
	var a calendarListArgs
	if err := json.Unmarshal(raw, &a); err != nil {
		return nil, fmt.Errorf("bad args: %w", err)
	}
	if a.Scope == "" {
		a.Scope = "today"
	}
	if a.Limit <= 0 {
		a.Limit = 50
	}

	var start, end time.Time
	switch a.Scope {
	case "today":
		now := time.Now()
		start = time.Date(now.Year(), now.Month(), now.Day(), 0, 0, 0, 0, now.Location())
		end = start.Add(24 * time.Hour)
	case "range":
		var err error
		start, err = time.Parse(time.RFC3339, a.Start)
		if err != nil {
			return nil, fmt.Errorf("invalid start: %w", err)
		}
		end, err = time.Parse(time.RFC3339, a.End)
		if err != nil {
			return nil, fmt.Errorf("invalid end: %w", err)
		}
	default:
		return nil, fmt.Errorf("scope must be 'today' or 'range'")
	}

	calFilter := ""
	if a.Calendar != "" {
		calFilter = fmt.Sprintf(`whose name is %s`, EscapeAppleScriptString(a.Calendar))
	}

	src := fmt.Sprintf(`
set startDate to (current date)
set startDate's year to %d
set startDate's month to %d
set startDate's day to %d
set startDate's hours to %d
set startDate's minutes to %d
set startDate's seconds to %d

set endDate to (current date)
set endDate's year to %d
set endDate's month to %d
set endDate's day to %d
set endDate's hours to %d
set endDate's minutes to %d
set endDate's seconds to %d

set out to {}
tell application "Calendar"
    set calList to (every calendar %s)
    repeat with cal in calList
        set calName to name of cal
        set evs to (every event of cal whose start date ≥ startDate and start date < endDate)
        repeat with ev in evs
            set end of out to ¬
                "{" & ¬
                "\"uid\":\"" & (uid of ev) & "\"," & ¬
                "\"title\":\"" & (my esc(summary of ev)) & "\"," & ¬
                "\"start\":\"" & (my iso(start date of ev)) & "\"," & ¬
                "\"end\":\"" & (my iso(end date of ev)) & "\"," & ¬
                "\"calendar\":\"" & (my esc(calName)) & "\"," & ¬
                "\"location\":\"" & (my esc(location of ev as string)) & "\"," & ¬
                "\"notes\":\"" & (my esc(description of ev as string)) & "\"," & ¬
                "\"all_day\":" & (my bl(allday event of ev)) & ¬
                "}"
        end repeat
    end repeat
end tell

set AppleScript's text item delimiters to ","
set payload to "[" & (out as string) & "]"
set AppleScript's text item delimiters to ""
return payload

on esc(s)
    set s to s as string
    set AppleScript's text item delimiters to "\\"
    set parts to every text item of s
    set AppleScript's text item delimiters to "\\\\"
    set s to parts as string
    set AppleScript's text item delimiters to "\""
    set parts to every text item of s
    set AppleScript's text item delimiters to "\\\""
    set s to parts as string
    set AppleScript's text item delimiters to (ASCII character 10)
    set parts to every text item of s
    set AppleScript's text item delimiters to "\\n"
    set s to parts as string
    set AppleScript's text item delimiters to ""
    return s
end esc

on iso(d)
    set y to year of d as string
    set m to text -2 thru -1 of ("0" & ((month of d as integer) as string))
    set dd to text -2 thru -1 of ("0" & (day of d as string))
    set hh to text -2 thru -1 of ("0" & (hours of d as string))
    set mm to text -2 thru -1 of ("0" & (minutes of d as string))
    set ss to text -2 thru -1 of ("0" & (seconds of d as string))
    return y & "-" & m & "-" & dd & "T" & hh & ":" & mm & ":" & ss
end iso

on bl(b)
    if b then
        return "true"
    else
        return "false"
    end if
end bl
`,
		start.Year(), int(start.Month()), start.Day(), start.Hour(), start.Minute(), start.Second(),
		end.Year(), int(end.Month()), end.Day(), end.Hour(), end.Minute(), end.Second(),
		calFilter,
	)

	out, err := RunAppleScript(ctx, src)
	if err != nil {
		return nil, err
	}
	var events []calendarEvent
	if out == "" {
		return map[string]any{"events": []calendarEvent{}, "scope": a.Scope}, nil
	}
	if err := json.Unmarshal([]byte(out), &events); err != nil {
		return nil, fmt.Errorf("decode: %w (raw: %s)", err, truncate(out, 200))
	}
	if len(events) > a.Limit {
		events = events[:a.Limit]
	}
	return map[string]any{"events": events, "scope": a.Scope}, nil
}

// --- create ------------------------------------------------------------------

type calendarCreateArgs struct {
	Title    string `json:"title"`
	Start    string `json:"start"`              // RFC3339
	End      string `json:"end"`                // RFC3339
	Calendar string `json:"calendar,omitempty"` // default: first writable calendar
	Location string `json:"location,omitempty"`
	Notes    string `json:"notes,omitempty"`
	AllDay   bool   `json:"all_day,omitempty"`
}

func calendarCreateHandler(ctx context.Context, raw json.RawMessage) (any, error) {
	var a calendarCreateArgs
	if err := json.Unmarshal(raw, &a); err != nil {
		return nil, fmt.Errorf("bad args: %w", err)
	}
	a.Title = strings.TrimSpace(a.Title)
	if a.Title == "" {
		return nil, fmt.Errorf("title is required")
	}
	start, err := time.Parse(time.RFC3339, a.Start)
	if err != nil {
		return nil, fmt.Errorf("invalid start: %w", err)
	}
	end, err := time.Parse(time.RFC3339, a.End)
	if err != nil {
		return nil, fmt.Errorf("invalid end: %w", err)
	}
	if !end.After(start) {
		return nil, fmt.Errorf("end must be after start")
	}

	summary := fmt.Sprintf("Create calendar event %q\n    %s — %s", a.Title,
		start.Format("2006-01-02 15:04"), end.Format("2006-01-02 15:04"))
	if a.Calendar != "" {
		summary += "\n    in calendar: " + a.Calendar
	}
	ok, err := approval.Request(ctx, approval.Request_{Summary: summary, Danger: "prompt"})
	if err != nil {
		return nil, fmt.Errorf("approval: %w", err)
	}
	if !ok {
		return nil, fmt.Errorf("user denied")
	}

	calRef := `first calendar whose writable is true`
	if a.Calendar != "" {
		calRef = fmt.Sprintf(`first calendar whose name is %s`, EscapeAppleScriptString(a.Calendar))
	}

	src := fmt.Sprintf(`
set startDate to (current date)
set startDate's year to %d
set startDate's month to %d
set startDate's day to %d
set startDate's hours to %d
set startDate's minutes to %d
set startDate's seconds to %d

set endDate to (current date)
set endDate's year to %d
set endDate's month to %d
set endDate's day to %d
set endDate's hours to %d
set endDate's minutes to %d
set endDate's seconds to %d

tell application "Calendar"
    tell %s
        set newEv to make new event with properties {summary:%s, start date:startDate, end date:endDate, location:%s, description:%s, allday event:%s}
        return (uid of newEv) & "|" & (summary of newEv) & "|" & (name of it)
    end tell
end tell
`,
		start.Year(), int(start.Month()), start.Day(), start.Hour(), start.Minute(), start.Second(),
		end.Year(), int(end.Month()), end.Day(), end.Hour(), end.Minute(), end.Second(),
		calRef,
		EscapeAppleScriptString(a.Title),
		EscapeAppleScriptString(a.Location),
		EscapeAppleScriptString(a.Notes),
		boolAS(a.AllDay),
	)

	out, err := RunAppleScript(ctx, src)
	if err != nil {
		return nil, err
	}
	parts := strings.SplitN(out, "|", 3)
	if len(parts) != 3 {
		return nil, fmt.Errorf("unexpected response: %s", truncate(out, 200))
	}
	return map[string]any{
		"uid":      parts[0],
		"title":    parts[1],
		"calendar": parts[2],
	}, nil
}

// --- search ------------------------------------------------------------------

type calendarSearchArgs struct {
	Query string `json:"query"`
	Limit int    `json:"limit,omitempty"`
}

func calendarSearchHandler(ctx context.Context, raw json.RawMessage) (any, error) {
	var a calendarSearchArgs
	if err := json.Unmarshal(raw, &a); err != nil {
		return nil, fmt.Errorf("bad args: %w", err)
	}
	a.Query = strings.TrimSpace(a.Query)
	if a.Query == "" {
		return nil, fmt.Errorf("query is required")
	}
	if a.Limit <= 0 {
		a.Limit = 20
	}
	// Calendar.app has no native search API; we fetch a rolling ±30 day
	// window and filter in Go. Good enough for most real-world queries.
	now := time.Now()
	start := now.Add(-30 * 24 * time.Hour)
	end := now.Add(60 * 24 * time.Hour)
	result, err := calendarListHandler(ctx, mustMarshal(calendarListArgs{
		Scope: "range",
		Start: start.Format(time.RFC3339),
		End:   end.Format(time.RFC3339),
		Limit: 500,
	}))
	if err != nil {
		return nil, err
	}
	m := result.(map[string]any)
	events := m["events"].([]calendarEvent)
	q := strings.ToLower(a.Query)
	var matches []calendarEvent
	for _, ev := range events {
		hay := strings.ToLower(ev.Title + " " + ev.Notes + " " + ev.Location)
		if strings.Contains(hay, q) {
			matches = append(matches, ev)
			if len(matches) >= a.Limit {
				break
			}
		}
	}
	return map[string]any{"events": matches, "query": a.Query}, nil
}

// --- small helpers -----------------------------------------------------------

func boolAS(b bool) string {
	if b {
		return "true"
	}
	return "false"
}

func mustMarshal(v any) json.RawMessage {
	raw, _ := json.Marshal(v)
	return raw
}
