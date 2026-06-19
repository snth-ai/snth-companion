package daemon

// call_tool.go — exposes the Real-Time Synth call surface to the synth as
// remote tools, so she can join a video call HERSELF when someone drops a
// meeting link in chat (no human at the Mac required). Registered from the
// daemon package (which already imports tools) so the handlers can close over
// GlobalCall. DangerLevel "safe": joining a link the user shared is not a
// destructive op and must not block on a Mac-side approval nobody will answer.

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"

	"github.com/snth-ai/snth-companion/internal/tools"
)

// RegisterCallTools wires remote_join_call / remote_leave_call /
// remote_call_status. Call once at startup from cmd/companion/main.go.
func RegisterCallTools() {
	tools.Register(tools.Descriptor{
		Name: "remote_join_call",
		Description: "Join a Google Meet video call on YOUR paired Mac to listen and take notes. " +
			"Use this when someone shares a Google Meet link with you (https://meet.google.com/xxx-xxxx-xxx) " +
			"and wants you on the call, or you decide to sit in. You join as a muted participant named after " +
			"yourself (mic + camera off) — you listen, you do not speak. Pass the meeting URL. After the call " +
			"ends you automatically get the transcript and can recap it. One call at a time.",
		DangerLevel: "safe",
	}, joinCallHandler)

	tools.Register(tools.Descriptor{
		Name:        "remote_leave_call",
		Description: "Leave the video call you are currently in (hang up). Use when the conversation is over or the user asks you to drop off.",
		DangerLevel: "safe",
	}, leaveCallHandler)

	tools.Register(tools.Descriptor{
		Name:        "remote_call_status",
		Description: "Check the current video call: whether you are in a call, its status, and the live transcript so far.",
		DangerLevel: "safe",
	}, callStatusHandler)
}

func joinCallHandler(ctx context.Context, raw json.RawMessage) (any, error) {
	var args struct {
		MeetURL string `json:"meet_url"`
		URL     string `json:"url"` // tolerate either key
	}
	_ = json.Unmarshal(raw, &args)
	url := strings.TrimSpace(args.MeetURL)
	if url == "" {
		url = strings.TrimSpace(args.URL)
	}
	if url == "" {
		return nil, fmt.Errorf("meet_url is required (a https://meet.google.com/... link)")
	}
	if err := GlobalCall.Join(url); err != nil {
		return nil, err
	}
	s := GlobalCall.Snapshot()
	return map[string]any{
		"ok":      true,
		"status":  s.Status,
		"meet_url": s.MeetURL,
		"note":    "Joining now. You will be admitted from the lobby by the host; poll remote_call_status to see when you are in.",
	}, nil
}

func leaveCallHandler(ctx context.Context, raw json.RawMessage) (any, error) {
	GlobalCall.Leave()
	s := GlobalCall.Snapshot()
	return map[string]any{"ok": true, "status": s.Status}, nil
}

func callStatusHandler(ctx context.Context, raw json.RawMessage) (any, error) {
	s := GlobalCall.Snapshot()
	return map[string]any{
		"in_call":    s.Status == "in_call",
		"status":     s.Status,
		"meet_url":   s.MeetURL,
		"transcript": s.Listen.Transcript,
		"manual_action": func() any {
			if s.ManualReason == "" {
				return nil
			}
			return map[string]string{"reason": s.ManualReason, "message": s.ManualMessage}
		}(),
	}, nil
}
