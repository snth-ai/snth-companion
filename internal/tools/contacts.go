package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"
)

// contacts.go — Apple Contacts via AppleScript.
//
// Single tool: search. The synth calls it when the user says "text
// Mia" or "find Katya's email"; we return name + phones + emails +
// org. Contacts.app grants Automation permission on first invocation
// (standard macOS flow, no extra entitlements).
//
// Search is client-side: we fetch all people matching a first-pass
// AppleScript filter (name contains <q>), then in Go further match
// against phones/emails/org so "+7 916" style queries work.

func RegisterContacts() {
	Register(Descriptor{
		Name:        "remote_contacts_search",
		Description: "Search the paired Mac's Apple Contacts address book. Returns name, phone numbers, emails, and organization for each match. Matches first name, last name, full name, organization, emails, and phone numbers (digits only).",
		DangerLevel: "prompt",
	}, contactsSearchHandler)
}

type contactsSearchArgs struct {
	Query string `json:"query"`
	Limit int    `json:"limit,omitempty"`
}

type contactEntry struct {
	Name   string   `json:"name"`
	Org    string   `json:"org,omitempty"`
	Phones []string `json:"phones,omitempty"`
	Emails []string `json:"emails,omitempty"`
}

func contactsSearchHandler(ctx context.Context, raw json.RawMessage) (any, error) {
	var a contactsSearchArgs
	if err := json.Unmarshal(raw, &a); err != nil {
		return nil, fmt.Errorf("bad args: %w", err)
	}
	a.Query = strings.TrimSpace(a.Query)
	if a.Query == "" {
		return nil, fmt.Errorf("query is required")
	}
	if a.Limit <= 0 {
		a.Limit = 10
	}

	// Two passes. First: AppleScript `whose name contains q or
	// organization contains q` — server-side filter, fast on large
	// address books. Second (only when the first returns nothing AND
	// the query has digits): bulk iterate and filter by phone/email
	// in Go. Bulk path is slow for 500+ contacts but it's the
	// fallback for phone-number lookups.
	src := fmt.Sprintf(`
set out to ""
set PSEP to (ASCII character 1)
set MSEP to (ASCII character 2)
set RSEP to (ASCII character 3)
set q to %s
tell application "Contacts"
    set hits to (every person whose name contains q or organization contains q or first name contains q or last name contains q)
    repeat with p in hits
        set nm to name of p as string
        set org to ""
        try
            set org to organization of p as string
        end try
        set ph to ""
        try
            set phList to value of every phone of p
            set ph to my joinList(phList, MSEP)
        end try
        set em to ""
        try
            set emList to value of every email of p
            set em to my joinList(emList, MSEP)
        end try
        set out to out & nm & PSEP & org & PSEP & ph & PSEP & em & RSEP
    end repeat
end tell
return out

on joinList(xs, sep)
    if (count of xs) = 0 then return ""
    set AppleScript's text item delimiters to sep
    set s to xs as string
    set AppleScript's text item delimiters to ""
    return s
end joinList
`, EscapeAppleScriptString(a.Query))

	out, err := RunAppleScript(ctx, src)
	if err != nil {
		return nil, err
	}
	records := strings.Split(out, string(rune(3)))
	q := strings.ToLower(a.Query)
	qDigits := onlyDigits(a.Query)

	var matches []contactEntry
	for _, rec := range records {
		if strings.TrimSpace(rec) == "" {
			continue
		}
		parts := strings.Split(rec, string(rune(1)))
		if len(parts) < 4 {
			continue
		}
		name := normalizeField(parts[0])
		org := normalizeField(parts[1])
		var phones, emails []string
		if s := normalizeField(parts[2]); s != "" {
			phones = splitAndFilter(s)
		}
		if s := normalizeField(parts[3]); s != "" {
			emails = splitAndFilter(s)
		}

		hay := strings.ToLower(name + " " + org + " " + strings.Join(emails, " "))
		hayDigits := ""
		for _, p := range phones {
			hayDigits += onlyDigits(p)
		}

		hit := false
		if strings.Contains(hay, q) {
			hit = true
		}
		if !hit && qDigits != "" && strings.Contains(hayDigits, qDigits) {
			hit = true
		}
		if !hit {
			continue
		}
		matches = append(matches, contactEntry{
			Name:   name,
			Org:    org,
			Phones: phones,
			Emails: emails,
		})
		if len(matches) >= a.Limit {
			break
		}
	}
	return map[string]any{"contacts": matches, "query": a.Query}, nil
}

// normalizeField strips AppleScript's "missing value" sentinel and
// trims whitespace. AppleScript returns `missing value` (coerced to
// the literal string "missing value") for unset fields like org on
// personal contacts.
func normalizeField(s string) string {
	s = strings.TrimSpace(s)
	if s == "missing value" {
		return ""
	}
	return s
}

// splitAndFilter splits on ASCII 2 and drops any "missing value" or
// empty entries from the result.
func splitAndFilter(s string) []string {
	parts := strings.Split(s, string(rune(2)))
	out := make([]string, 0, len(parts))
	for _, p := range parts {
		if v := normalizeField(p); v != "" {
			out = append(out, v)
		}
	}
	return out
}

func onlyDigits(s string) string {
	var b strings.Builder
	for _, r := range s {
		if r >= '0' && r <= '9' {
			b.WriteRune(r)
		}
	}
	return b.String()
}
