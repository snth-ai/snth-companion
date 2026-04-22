//go:build !darwin

package tools

// darwin_stubs_other.go — no-op Register functions on non-darwin so the
// companion binary still compiles on Windows/Linux. The corresponding
// real implementations (calendar, notes, reminders, contacts, messages,
// shortcuts, clipboard, notify) all shell out to osascript/pbcopy/pbpaste,
// which don't exist off macOS. On non-darwin these register nothing —
// the synth simply won't see `remote_calendar_*` etc. in the hello frame,
// which is the desired behavior.

func RegisterCalendar()  {}
func RegisterNotes()     {}
func RegisterReminders() {}
func RegisterContacts()  {}
func RegisterMessages()  {}
func RegisterShortcut()  {}
func RegisterClipboard() {}
func RegisterNotify()    {}
