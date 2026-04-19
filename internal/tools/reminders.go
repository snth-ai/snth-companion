package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"
	"time"

	"github.com/snth-ai/snth-companion/internal/approval"
)

// reminders.go — Apple Reminders via AppleScript.
//
// Model: Reminders groups items into "lists" (the Calendar-app sibling
// concept, not AppleScript's list type). Every reminder has a unique
// `id` property — globally stable across sessions, safe to pass to
// `complete`. Default list used for `create` when none is specified
// is whatever the user has configured as default in Reminders.app.
//
// Reminders' AppleScript dictionary is shallower than Calendar's —
// there's no bulk fetch-by-predicate-date-range, so we iterate. Still
// fast because reminder counts are tiny compared to calendar events.

func RegisterReminders() {
	Register(Descriptor{
		Name:        "remote_reminders_list",
		Description: "List reminders from Apple Reminders on the paired Mac. Optionally scoped to one list; defaults to every list. Excludes completed reminders unless include_completed=true.",
		DangerLevel: "safe",
	}, remindersListHandler)
	Register(Descriptor{
		Name:        "remote_reminders_create",
		Description: "Create a new reminder in Apple Reminders. Prompts for user approval.",
		DangerLevel: "prompt",
	}, remindersCreateHandler)
	Register(Descriptor{
		Name:        "remote_reminders_complete",
		Description: "Mark a reminder as completed by id. Prompts for user approval.",
		DangerLevel: "prompt",
	}, remindersCompleteHandler)
}

// --- list --------------------------------------------------------------------

type remindersListArgs struct {
	List              string `json:"list,omitempty"`
	IncludeCompleted  bool   `json:"include_completed,omitempty"`
	Limit             int    `json:"limit,omitempty"`
}

type reminderEntry struct {
	ID        string `json:"id"`
	Title     string `json:"title"`
	List      string `json:"list"`
	Due       string `json:"due,omitempty"`     // RFC3339 or ""
	Completed bool   `json:"completed"`
	Body      string `json:"body,omitempty"`
}

func remindersListHandler(ctx context.Context, raw json.RawMessage) (any, error) {
	var a remindersListArgs
	if err := json.Unmarshal(raw, &a); err != nil {
		return nil, fmt.Errorf("bad args: %w", err)
	}
	if a.Limit <= 0 {
		a.Limit = 200
	}

	listFilter := ""
	if a.List != "" {
		listFilter = fmt.Sprintf(`whose name is %s`, EscapeAppleScriptString(a.List))
	}
	completedClause := "whose completed is false"
	if a.IncludeCompleted {
		completedClause = ""
	}

	src := fmt.Sprintf(`
set out to {}
tell application "Reminders"
    set lists to (every list %s)
    repeat with lst in lists
        set listName to name of lst
        set rms to (every reminder of lst %s)
        repeat with r in rms
            set dueStr to ""
            try
                set dueStr to (my iso(due date of r))
            end try
            set end of out to ¬
                "{" & ¬
                "\"id\":\"" & (id of r) & "\"," & ¬
                "\"title\":\"" & (my esc(name of r as string)) & "\"," & ¬
                "\"list\":\"" & (my esc(listName)) & "\"," & ¬
                "\"due\":\"" & dueStr & "\"," & ¬
                "\"completed\":" & (my bl(completed of r)) & "," & ¬
                "\"body\":\"" & (my esc((body of r) as string)) & "\"" & ¬
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
`, listFilter, completedClause)

	out, err := RunAppleScript(ctx, src)
	if err != nil {
		return nil, err
	}
	if out == "" || out == "[]" {
		return map[string]any{"reminders": []reminderEntry{}, "list": a.List}, nil
	}
	var rems []reminderEntry
	if err := json.Unmarshal([]byte(out), &rems); err != nil {
		return nil, fmt.Errorf("decode: %w (raw: %s)", err, truncate(out, 200))
	}
	if len(rems) > a.Limit {
		rems = rems[:a.Limit]
	}
	return map[string]any{"reminders": rems, "list": a.List}, nil
}

// --- create ------------------------------------------------------------------

type remindersCreateArgs struct {
	Title string `json:"title"`
	Due   string `json:"due,omitempty"`  // RFC3339
	List  string `json:"list,omitempty"` // default: system default list
	Body  string `json:"body,omitempty"`
}

func remindersCreateHandler(ctx context.Context, raw json.RawMessage) (any, error) {
	var a remindersCreateArgs
	if err := json.Unmarshal(raw, &a); err != nil {
		return nil, fmt.Errorf("bad args: %w", err)
	}
	a.Title = strings.TrimSpace(a.Title)
	if a.Title == "" {
		return nil, fmt.Errorf("title is required")
	}
	var due time.Time
	hasDue := false
	if a.Due != "" {
		t, err := time.Parse(time.RFC3339, a.Due)
		if err != nil {
			return nil, fmt.Errorf("invalid due: %w", err)
		}
		due = t
		hasDue = true
	}

	summary := fmt.Sprintf("Create reminder %q", a.Title)
	if hasDue {
		summary += "\n    due: " + due.Format("2006-01-02 15:04")
	}
	if a.List != "" {
		summary += "\n    list: " + a.List
	}
	ok, err := approval.Request(ctx, approval.Request_{Summary: summary, Danger: "prompt"})
	if err != nil {
		return nil, fmt.Errorf("approval: %w", err)
	}
	if !ok {
		return nil, fmt.Errorf("user denied")
	}

	// Build the property list. AppleScript's Reminders supports
	// {name, body, due date}. Body is optional.
	props := []string{
		fmt.Sprintf("name:%s", EscapeAppleScriptString(a.Title)),
	}
	if a.Body != "" {
		props = append(props, fmt.Sprintf("body:%s", EscapeAppleScriptString(a.Body)))
	}

	dueBlock := ""
	if hasDue {
		dueBlock = fmt.Sprintf(`
set dueDate to (current date)
set dueDate's year to %d
set dueDate's month to %d
set dueDate's day to %d
set dueDate's hours to %d
set dueDate's minutes to %d
set dueDate's seconds to %d
`, due.Year(), int(due.Month()), due.Day(), due.Hour(), due.Minute(), due.Second())
		props = append(props, "due date:dueDate")
	}

	scope := ""
	if a.List != "" {
		scope = fmt.Sprintf(`of (list %s)`, EscapeAppleScriptString(a.List))
	}

	src := fmt.Sprintf(`
%s
tell application "Reminders"
    set r to make new reminder %s with properties {%s}
    return (id of r) & "|" & (name of r)
end tell
`, dueBlock, scope, strings.Join(props, ", "))

	out, err := RunAppleScript(ctx, src)
	if err != nil {
		return nil, err
	}
	parts := strings.SplitN(out, "|", 2)
	if len(parts) != 2 {
		return nil, fmt.Errorf("unexpected response: %s", truncate(out, 200))
	}
	return map[string]any{"id": parts[0], "title": parts[1], "list": a.List}, nil
}

// --- complete ----------------------------------------------------------------

type remindersCompleteArgs struct {
	ID string `json:"id"`
}

func remindersCompleteHandler(ctx context.Context, raw json.RawMessage) (any, error) {
	var a remindersCompleteArgs
	if err := json.Unmarshal(raw, &a); err != nil {
		return nil, fmt.Errorf("bad args: %w", err)
	}
	if a.ID == "" {
		return nil, fmt.Errorf("id is required")
	}

	ok, err := approval.Request(ctx, approval.Request_{
		Summary: "Mark reminder " + a.ID + " as completed",
		Danger:  "prompt",
	})
	if err != nil {
		return nil, fmt.Errorf("approval: %w", err)
	}
	if !ok {
		return nil, fmt.Errorf("user denied")
	}

	src := fmt.Sprintf(`
tell application "Reminders"
    set r to reminder id %s
    set completed of r to true
    return (id of r) & "|" & (name of r)
end tell
`, EscapeAppleScriptString(a.ID))

	out, err := RunAppleScript(ctx, src)
	if err != nil {
		return nil, err
	}
	parts := strings.SplitN(out, "|", 2)
	if len(parts) != 2 {
		return nil, fmt.Errorf("unexpected response: %s", truncate(out, 200))
	}
	return map[string]any{"id": parts[0], "title": parts[1], "completed": true}, nil
}
