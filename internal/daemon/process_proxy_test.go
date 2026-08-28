package daemon

import (
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
)

func TestProcessProxyBindingAndFailures(t *testing.T) {
	calls := 0
	hub := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		calls++
		if r.Header.Get("Authorization") != "Bearer test-server-token" || r.Header.Get("X-Owner-Session") != "" {
			t.Error("untrusted credentials forwarded")
		}
		if r.URL.RequestURI() != "/api/my/processes?id=one%3Atwo" {
			t.Errorf("bad route: %s", r.URL.RequestURI())
		}
		b, _ := io.ReadAll(r.Body)
		if string(b) != `{"action":"pause"}` {
			t.Errorf("body: %s", b)
		}
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(409)
		_, _ = w.Write([]byte(`{"error":"revision conflict"}`))
	}))
	defer hub.Close()
	s := &UIServer{}
	for _, tc := range []struct {
		method, expected, body string
		status                 int
	}{
		{"POST", "syn1", `{"action":"pause"}`, 409},
		{"POST", "syn2", `{}`, 409},
		{"GET", "", "", 409},
		{"DELETE", "syn1", "", 405},
		{"POST", "syn1", strings.Repeat("x", 65537), 413},
	} {
		r := httptest.NewRequest(tc.method, "/api/hub/processes?id=one%3Atwo", strings.NewReader(tc.body))
		r.Header.Set("X-Snth-Expected-Synth-Id", tc.expected)
		r.Header.Set("X-Owner-Session", "attacker")
		r.Header.Set("Authorization", "Bearer attacker")
		w := httptest.NewRecorder()
		s.proxyProcesses(w, r, "syn1", hub.URL, "test-server-token")
		if w.Code != tc.status {
			t.Fatalf("%s %s: %d != %d", tc.method, tc.expected, w.Code, tc.status)
		}
		if w.Header().Get("Cache-Control") != "private, no-store" {
			t.Fatal("private response cached")
		}
	}
	if calls != 1 {
		t.Fatalf("unexpected upstream calls %d", calls)
	}
}
