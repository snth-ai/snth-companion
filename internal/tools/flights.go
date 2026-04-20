package tools

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"os/exec"
	"strings"
	"time"
)

// flights.go — ports the old synth-side search_flights tool into the
// companion. It wraps the `letsfg` CLI, which scrapes a handful of
// OTAs and returns a ranked list of offers.
//
// Why here and not on the synth: letsfg isn't in the synth Docker
// image, and the search needs to run a bunch of concurrent browser
// sessions — better to keep that on the user's Mac (where Chrome
// is already warmed up and they can observe if anything goes
// sideways) than multiply containers on Hetzner.
//
// No approval prompt — the search is read-only (no bookings, no
// emails sent). If we ever add "book this one" semantics we'll
// gate that separately.

func RegisterFlights() {
	if _, err := exec.LookPath("letsfg"); err != nil {
		// CLI not installed — register anyway so the LLM sees the
		// capability; first call surfaces a friendly error with
		// install instructions.
	}
	Register(Descriptor{
		Name:        "remote_flight_search",
		Description: "Search flights between IATA airports using the letsfg CLI on the paired Mac. Scrapes several OTAs, returns prices/airlines/durations/stops/booking links sorted by price. TAKES 1-2 MINUTES per call (heavy scraping). Warn the user before invoking. Always include the booking URLs in your reply.",
		DangerLevel: "safe",
	}, flightsHandler)
}

type flightsArgs struct {
	Origin      string `json:"origin"`
	Destination string `json:"destination"`
	Date        string `json:"date"`
	ReturnDate  string `json:"return_date,omitempty"`
	Adults      int    `json:"adults,omitempty"`
	Cabin       string `json:"cabin,omitempty"`
	DirectOnly  bool   `json:"direct_only,omitempty"`
	MaxStops    int    `json:"max_stops,omitempty"`
	Currency    string `json:"currency,omitempty"`
	Limit       int    `json:"limit,omitempty"`
}

func flightsHandler(ctx context.Context, raw json.RawMessage) (any, error) {
	var a flightsArgs
	if err := json.Unmarshal(raw, &a); err != nil {
		return nil, fmt.Errorf("bad args: %w", err)
	}
	a.Origin = strings.TrimSpace(strings.ToUpper(a.Origin))
	a.Destination = strings.TrimSpace(strings.ToUpper(a.Destination))
	if a.Origin == "" || a.Destination == "" || a.Date == "" {
		return nil, fmt.Errorf("origin, destination and date are required")
	}

	if _, err := exec.LookPath("letsfg"); err != nil {
		return nil, fmt.Errorf("letsfg CLI not found in PATH on the paired Mac — install it first (https://github.com/letsfg/letsfg or via the distribution your operator uses)")
	}

	cmdArgs := []string{
		"search",
		a.Origin, a.Destination, a.Date,
		"--json",
		"--sort", "price",
	}
	if a.ReturnDate != "" {
		cmdArgs = append(cmdArgs, "--return", a.ReturnDate)
	}
	if a.Adults > 1 {
		cmdArgs = append(cmdArgs, "--adults", fmt.Sprintf("%d", a.Adults))
	}
	if a.Cabin != "" {
		cmdArgs = append(cmdArgs, "--cabin", a.Cabin)
	}
	if a.DirectOnly {
		cmdArgs = append(cmdArgs, "--direct")
	}
	if a.MaxStops > 0 {
		cmdArgs = append(cmdArgs, "--max-stops", fmt.Sprintf("%d", a.MaxStops))
	}
	currency := "EUR"
	if a.Currency != "" {
		currency = strings.ToUpper(a.Currency)
	}
	cmdArgs = append(cmdArgs, "--currency", currency)
	limit := 10
	if a.Limit > 0 && a.Limit <= 50 {
		limit = a.Limit
	}
	cmdArgs = append(cmdArgs, "--limit", fmt.Sprintf("%d", limit))
	cmdArgs = append(cmdArgs, "--max-browsers", "8")

	// Cap at 5 minutes — letsfg times out itself around 3 min, this is
	// belt-and-suspenders.
	cctx, cancel := context.WithTimeout(ctx, 5*time.Minute)
	defer cancel()

	cmd := exec.CommandContext(cctx, "letsfg", cmdArgs...)
	var stdout, stderr bytes.Buffer
	cmd.Stdout = &stdout
	cmd.Stderr = &stderr
	if err := cmd.Run(); err != nil {
		if cctx.Err() == context.DeadlineExceeded {
			return nil, fmt.Errorf("letsfg timeout after 5m — the scraper is probably stuck, retry later")
		}
		return nil, fmt.Errorf("letsfg: %w (stderr: %s)", err, truncate(strings.TrimSpace(stderr.String()), 400))
	}

	var parsed struct {
		TotalResults int           `json:"total_results"`
		Offers       []flightOffer `json:"offers"`
	}
	if err := json.Unmarshal(stdout.Bytes(), &parsed); err != nil {
		// Non-JSON output — return raw head so the LLM can diagnose.
		return nil, fmt.Errorf("letsfg returned non-JSON: %s", truncate(stdout.String(), 600))
	}
	if len(parsed.Offers) > limit {
		parsed.Offers = parsed.Offers[:limit]
	}
	return map[string]any{
		"origin":        a.Origin,
		"destination":   a.Destination,
		"date":          a.Date,
		"return_date":   a.ReturnDate,
		"currency":      currency,
		"total_results": parsed.TotalResults,
		"offers":        parsed.Offers,
		"formatted":     formatOffers(parsed.Offers, a.Origin, a.Destination, a.Date, limit),
	}, nil
}

// --- types (identical wire shape to openpaw_server/tools/flights.go) --------

type flightOffer struct {
	ID             string       `json:"id"`
	Price          float64      `json:"price"`
	Currency       string       `json:"currency"`
	PriceFormatted string       `json:"price_formatted"`
	Outbound       *flightRoute `json:"outbound"`
	Inbound        *flightRoute `json:"inbound"`
	Airlines       []string     `json:"airlines"`
	OwnerAirline   string       `json:"owner_airline"`
	Source         string       `json:"source"`
	BookingURL     string       `json:"booking_url"`
}

type flightRoute struct {
	Segments             []flightSegment `json:"segments"`
	TotalDurationSeconds int             `json:"total_duration_seconds"`
	Stopovers            int             `json:"stopovers"`
}

type flightSegment struct {
	Airline     string `json:"airline"`
	AirlineName string `json:"airline_name"`
	FlightNo    string `json:"flight_no"`
	Origin      string `json:"origin"`
	Destination string `json:"destination"`
	Departure   string `json:"departure"`
	Arrival     string `json:"arrival"`
	CabinClass  string `json:"cabin_class"`
	Aircraft    string `json:"aircraft"`
}

// formatOffers produces the same markdown block the old synth-side
// tool returned, so the LLM sees consistent output regardless of
// whether the tool runs remotely or locally.
func formatOffers(offers []flightOffer, origin, dest, date string, limit int) string {
	if len(offers) == 0 {
		return fmt.Sprintf("No flights found %s → %s on %s", origin, dest, date)
	}
	var sb strings.Builder
	sb.WriteString(fmt.Sprintf("✈️ %s → %s on %s (%d offers)\n\n", origin, dest, date, len(offers)))
	for i, o := range offers {
		if i >= limit {
			break
		}
		sb.WriteString(fmt.Sprintf("**%d. %s %.2f**", i+1, o.Currency, o.Price))
		if len(o.Airlines) > 0 {
			sb.WriteString(fmt.Sprintf(" — %s", strings.Join(o.Airlines, ", ")))
		}
		sb.WriteString("\n")

		if o.Outbound != nil {
			dur := formatFlightDuration(o.Outbound.TotalDurationSeconds)
			stops := "direct"
			if o.Outbound.Stopovers > 0 {
				stops = fmt.Sprintf("%d stop(s)", o.Outbound.Stopovers)
			}
			route := buildRoute(o.Outbound.Segments)
			sb.WriteString(fmt.Sprintf("  → %s | %s | %s\n", route, dur, stops))
			for _, seg := range o.Outbound.Segments {
				sb.WriteString(fmt.Sprintf("    %s: %s → %s\n", seg.FlightNo, formatFlightTime(seg.Departure), formatFlightTime(seg.Arrival)))
			}
		}
		if o.Inbound != nil {
			dur := formatFlightDuration(o.Inbound.TotalDurationSeconds)
			stops := "direct"
			if o.Inbound.Stopovers > 0 {
				stops = fmt.Sprintf("%d stop(s)", o.Inbound.Stopovers)
			}
			route := buildRoute(o.Inbound.Segments)
			sb.WriteString(fmt.Sprintf("  ← %s | %s | %s\n", route, dur, stops))
			for _, seg := range o.Inbound.Segments {
				sb.WriteString(fmt.Sprintf("    %s: %s → %s\n", seg.FlightNo, formatFlightTime(seg.Departure), formatFlightTime(seg.Arrival)))
			}
		}
		if o.BookingURL != "" {
			sb.WriteString(fmt.Sprintf("  🔗 %s\n", o.BookingURL))
		}
		sb.WriteString("\n")
	}
	return sb.String()
}

func buildRoute(segs []flightSegment) string {
	if len(segs) == 0 {
		return ""
	}
	parts := []string{segs[0].Origin}
	for _, s := range segs {
		parts = append(parts, s.Destination)
	}
	return strings.Join(parts, "→")
}

func formatFlightDuration(seconds int) string {
	if seconds <= 0 {
		return "?"
	}
	h := seconds / 3600
	m := (seconds % 3600) / 60
	if h > 0 {
		return fmt.Sprintf("%dh%02dm", h, m)
	}
	return fmt.Sprintf("%dm", m)
}

func formatFlightTime(iso string) string {
	layouts := []string{time.RFC3339, "2006-01-02T15:04:05"}
	for _, l := range layouts {
		if t, err := time.Parse(l, iso); err == nil {
			return t.Format("15:04")
		}
	}
	return iso
}
