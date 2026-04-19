package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"

	"github.com/snth-ai/snth-companion/internal/approval"
)

// notes.go — Apple Notes via AppleScript.
//
// Three tools: list (optionally scoped to a folder), create, read.
// Notes.app stores bodies as HTML; when we read we strip tags and
// hand back plain text — synths don't need markup for reasoning and
// HTML blows the context budget. When we create we pass a plain
// string and Notes wraps it in a default-styled body; basic newlines
// are preserved.
//
// Note IDs are Notes' opaque `id of note` — stable-ish but Apple
// reserves the right to rotate. For cross-session persistence prefer
// looking up by title.

func RegisterNotes() {
	Register(Descriptor{
		Name:        "remote_notes_list",
		Description: "List Apple Notes on the paired Mac (optionally within a folder).",
		DangerLevel: "safe",
	}, notesListHandler)
	Register(Descriptor{
		Name:        "remote_notes_create",
		Description: "Create a new note in Apple Notes on the paired Mac. Prompts for user approval.",
		DangerLevel: "prompt",
	}, notesCreateHandler)
	Register(Descriptor{
		Name:        "remote_notes_read",
		Description: "Read the body of an Apple Note by title (first match) or id.",
		DangerLevel: "prompt",
	}, notesReadHandler)
}

// --- list --------------------------------------------------------------------

type notesListArgs struct {
	Folder string `json:"folder,omitempty"`
	Limit  int    `json:"limit,omitempty"`
}

type noteSummary struct {
	ID       string `json:"id"`
	Title    string `json:"title"`
	Folder   string `json:"folder"`
	Modified string `json:"modified"`
}

func notesListHandler(ctx context.Context, raw json.RawMessage) (any, error) {
	var a notesListArgs
	if err := json.Unmarshal(raw, &a); err != nil {
		return nil, fmt.Errorf("bad args: %w", err)
	}
	if a.Limit <= 0 {
		a.Limit = 50
	}

	scope := ""
	if a.Folder != "" {
		scope = fmt.Sprintf("in folder %s", EscapeAppleScriptString(a.Folder))
	}

	// Notes' AppleScript dictionary doesn't give a nice iso date; we
	// format ourselves.
	src := fmt.Sprintf(`
set out to {}
tell application "Notes"
    set nts to (every note %s)
    repeat with n in nts
        set end of out to ¬
            "{" & ¬
            "\"id\":\"" & (id of n) & "\"," & ¬
            "\"title\":\"" & (my esc(name of n)) & "\"," & ¬
            "\"folder\":\"" & (my esc((name of (container of n)) as string)) & "\"," & ¬
            "\"modified\":\"" & (my iso(modification date of n)) & "\"" & ¬
            "}"
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
`, scope)

	out, err := RunAppleScript(ctx, src)
	if err != nil {
		return nil, err
	}
	if out == "" || out == "[]" {
		return map[string]any{"notes": []noteSummary{}, "folder": a.Folder}, nil
	}
	var notes []noteSummary
	if err := json.Unmarshal([]byte(out), &notes); err != nil {
		return nil, fmt.Errorf("decode: %w (raw: %s)", err, truncate(out, 200))
	}
	if len(notes) > a.Limit {
		notes = notes[:a.Limit]
	}
	return map[string]any{"notes": notes, "folder": a.Folder}, nil
}

// --- create ------------------------------------------------------------------

type notesCreateArgs struct {
	Title  string `json:"title"`
	Body   string `json:"body"`
	Folder string `json:"folder,omitempty"`
}

func notesCreateHandler(ctx context.Context, raw json.RawMessage) (any, error) {
	var a notesCreateArgs
	if err := json.Unmarshal(raw, &a); err != nil {
		return nil, fmt.Errorf("bad args: %w", err)
	}
	a.Title = strings.TrimSpace(a.Title)
	if a.Title == "" {
		return nil, fmt.Errorf("title is required")
	}
	if len(a.Body) > 1*1024*1024 {
		return nil, fmt.Errorf("body too large: %d (max 1 MiB)", len(a.Body))
	}

	summary := fmt.Sprintf("Create Apple Note %q", a.Title)
	if a.Folder != "" {
		summary += "\n    in folder: " + a.Folder
	}
	if a.Body != "" {
		preview := a.Body
		if len(preview) > 180 {
			preview = preview[:180] + "…"
		}
		summary += "\n    body preview: " + preview
	}
	ok, err := approval.Request(ctx, approval.Request_{Summary: summary, Danger: "prompt"})
	if err != nil {
		return nil, fmt.Errorf("approval: %w", err)
	}
	if !ok {
		return nil, fmt.Errorf("user denied")
	}

	bodyHTML := plainToHTML(a.Body)
	scope := ""
	if a.Folder != "" {
		scope = fmt.Sprintf(`in folder %s`, EscapeAppleScriptString(a.Folder))
	}
	src := fmt.Sprintf(`
tell application "Notes"
    set n to make new note with properties {name:%s, body:%s} %s
    return id of n
end tell
`,
		EscapeAppleScriptString(a.Title),
		EscapeAppleScriptString(bodyHTML),
		scope,
	)
	out, err := RunAppleScript(ctx, src)
	if err != nil {
		return nil, err
	}
	return map[string]any{"id": out, "title": a.Title}, nil
}

// --- read --------------------------------------------------------------------

type notesReadArgs struct {
	ID    string `json:"id,omitempty"`
	Title string `json:"title,omitempty"`
}

func notesReadHandler(ctx context.Context, raw json.RawMessage) (any, error) {
	var a notesReadArgs
	if err := json.Unmarshal(raw, &a); err != nil {
		return nil, fmt.Errorf("bad args: %w", err)
	}
	if a.ID == "" && a.Title == "" {
		return nil, fmt.Errorf("id or title required")
	}

	summary := "Read Apple Note"
	if a.Title != "" {
		summary += " titled " + a.Title
	} else {
		summary += " id " + a.ID
	}
	ok, err := approval.Request(ctx, approval.Request_{Summary: summary, Danger: "prompt"})
	if err != nil {
		return nil, fmt.Errorf("approval: %w", err)
	}
	if !ok {
		return nil, fmt.Errorf("user denied")
	}

	var ref string
	if a.ID != "" {
		ref = fmt.Sprintf(`note id %s`, EscapeAppleScriptString(a.ID))
	} else {
		ref = fmt.Sprintf(`first note whose name is %s`, EscapeAppleScriptString(a.Title))
	}

	src := fmt.Sprintf(`
tell application "Notes"
    set n to %s
    return (id of n) & (ASCII character 1) & (name of n) & (ASCII character 1) & (body of n)
end tell
`, ref)
	out, err := RunAppleScript(ctx, src)
	if err != nil {
		return nil, err
	}
	parts := strings.SplitN(out, string(rune(1)), 3)
	if len(parts) != 3 {
		return nil, fmt.Errorf("unexpected response: %s", truncate(out, 200))
	}
	return map[string]any{
		"id":    parts[0],
		"title": parts[1],
		"body":  htmlToPlain(parts[2]),
	}, nil
}

// --- HTML ↔ plain helpers ---------------------------------------------------
//
// Notes stores bodies as HTML. We convert both ways with the
// absolute minimum: strip tags for reading, wrap in <div> for writing.
// Not a full HTML parser — attempting one in-repo would be a tar pit;
// if a note has complex formatting we drop it. Synths don't care.

func htmlToPlain(s string) string {
	// Strip tags greedily. <br> and <div> become newlines.
	s = strings.ReplaceAll(s, "<br>", "\n")
	s = strings.ReplaceAll(s, "<br/>", "\n")
	s = strings.ReplaceAll(s, "<br />", "\n")
	s = strings.ReplaceAll(s, "</div>", "\n")
	s = strings.ReplaceAll(s, "</p>", "\n")
	var b strings.Builder
	inTag := false
	for _, r := range s {
		switch r {
		case '<':
			inTag = true
		case '>':
			inTag = false
		default:
			if !inTag {
				b.WriteRune(r)
			}
		}
	}
	// Collapse runs of newlines.
	out := b.String()
	for strings.Contains(out, "\n\n\n") {
		out = strings.ReplaceAll(out, "\n\n\n", "\n\n")
	}
	// Decode a tiny set of HTML entities.
	out = strings.ReplaceAll(out, "&amp;", "&")
	out = strings.ReplaceAll(out, "&lt;", "<")
	out = strings.ReplaceAll(out, "&gt;", ">")
	out = strings.ReplaceAll(out, "&quot;", `"`)
	out = strings.ReplaceAll(out, "&#39;", "'")
	out = strings.ReplaceAll(out, "&nbsp;", " ")
	return strings.TrimSpace(out)
}

func plainToHTML(s string) string {
	// Minimal: escape entities, wrap each line in <div>, blank lines
	// become <br>.
	s = strings.ReplaceAll(s, "&", "&amp;")
	s = strings.ReplaceAll(s, "<", "&lt;")
	s = strings.ReplaceAll(s, ">", "&gt;")
	lines := strings.Split(s, "\n")
	var b strings.Builder
	for _, ln := range lines {
		if ln == "" {
			b.WriteString("<div><br></div>")
		} else {
			b.WriteString("<div>")
			b.WriteString(ln)
			b.WriteString("</div>")
		}
	}
	return b.String()
}
