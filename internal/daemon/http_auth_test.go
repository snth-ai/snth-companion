package daemon

import (
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"testing"
)

func testGuard(t *testing.T) (*UIServer, http.Handler) {
	t.Helper()
	s := &UIServer{Token: "secrettoken123", Addr: "127.0.0.1:54321"}
	inner := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(200)
		_, _ = w.Write([]byte("ok"))
	})
	return s, s.guard(inner)
}

func TestGuardRejectsMutationWithoutToken(t *testing.T) {
	s, h := testGuard(t)
	r := httptest.NewRequest("POST", "http://127.0.0.1:54321/api/trust/master", strings.NewReader(`{}`))
	r.RemoteAddr = "127.0.0.1:5000"
	r.Header.Set("Origin", "http://127.0.0.1:54321") // same-origin, but no token
	w := httptest.NewRecorder()
	h.ServeHTTP(w, r)
	if w.Code != http.StatusForbidden {
		t.Fatalf("no-token mutation: want 403, got %d", w.Code)
	}
	_ = s
}

func TestGuardRejectsCrossOrigin(t *testing.T) {
	_, h := testGuard(t)
	r := httptest.NewRequest("POST", "http://127.0.0.1:54321/api/trust/master", strings.NewReader(`{}`))
	r.RemoteAddr = "127.0.0.1:5000"
	r.Header.Set("Origin", "http://evil.example.com") // cross-site CSRF
	r.Header.Set(uiTokenHeader, "secrettoken123")     // even WITH the token
	w := httptest.NewRecorder()
	h.ServeHTTP(w, r)
	if w.Code != http.StatusForbidden {
		t.Fatalf("cross-origin mutation: want 403, got %d", w.Code)
	}
}

func TestGuardAllowsMutationWithHeaderTokenSameOrigin(t *testing.T) {
	_, h := testGuard(t)
	r := httptest.NewRequest("POST", "http://127.0.0.1:54321/api/trust/master", strings.NewReader(`{}`))
	r.RemoteAddr = "127.0.0.1:5000"
	r.Header.Set("Origin", "http://127.0.0.1:54321")
	r.Header.Set(uiTokenHeader, "secrettoken123")
	w := httptest.NewRecorder()
	h.ServeHTTP(w, r)
	if w.Code != 200 {
		t.Fatalf("valid token + same-origin: want 200, got %d", w.Code)
	}
}

func TestGuardAllowsMutationWithCookie(t *testing.T) {
	_, h := testGuard(t)
	r := httptest.NewRequest("POST", "http://127.0.0.1:54321/api/trust/master", strings.NewReader(`{}`))
	r.RemoteAddr = "127.0.0.1:5000"
	r.Header.Set("Origin", "http://127.0.0.1:54321")
	r.AddCookie(&http.Cookie{Name: uiTokenCookie, Value: "secrettoken123"})
	w := httptest.NewRecorder()
	h.ServeHTTP(w, r)
	if w.Code != 200 {
		t.Fatalf("cookie token: want 200, got %d", w.Code)
	}
}

func TestGuardAllowsFormFieldToken(t *testing.T) {
	_, h := testGuard(t)
	form := url.Values{uiTokenField: {"secrettoken123"}, "on": {"true"}}
	r := httptest.NewRequest("POST", "http://127.0.0.1:54321/pair/save", strings.NewReader(form.Encode()))
	r.RemoteAddr = "127.0.0.1:5000"
	r.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	r.Header.Set("Origin", "http://127.0.0.1:54321")
	w := httptest.NewRecorder()
	h.ServeHTTP(w, r)
	if w.Code != 200 {
		t.Fatalf("form-field token: want 200, got %d", w.Code)
	}
}

func TestGuardOpenReadsStayOpen(t *testing.T) {
	_, h := testGuard(t)
	for _, p := range []string{"/health", "/api/status", "/"} {
		r := httptest.NewRequest("GET", "http://127.0.0.1:54321"+p, nil)
		r.RemoteAddr = "127.0.0.1:5000"
		w := httptest.NewRecorder()
		h.ServeHTTP(w, r)
		if w.Code != 200 {
			t.Fatalf("GET %s should be open, got %d", p, w.Code)
		}
	}
}

func TestGuardSetsTokenCookieOnGet(t *testing.T) {
	_, h := testGuard(t)
	r := httptest.NewRequest("GET", "http://127.0.0.1:54321/", nil)
	r.RemoteAddr = "127.0.0.1:5000"
	w := httptest.NewRecorder()
	h.ServeHTTP(w, r)
	got := w.Result().Cookies()
	found := false
	for _, c := range got {
		if c.Name == uiTokenCookie && c.Value == "secrettoken123" {
			found = true
		}
	}
	if !found {
		t.Fatal("GET did not set the UI token cookie")
	}
}

func TestGuardRejectsNonLoopback(t *testing.T) {
	_, h := testGuard(t)
	r := httptest.NewRequest("GET", "http://127.0.0.1:54321/", nil)
	r.RemoteAddr = "10.0.0.5:5000" // remote host
	w := httptest.NewRecorder()
	h.ServeHTTP(w, r)
	if w.Code != http.StatusForbidden {
		t.Fatalf("non-loopback: want 403, got %d", w.Code)
	}
}

func TestInjectFormToken(t *testing.T) {
	s := &UIServer{Token: "tok"}
	in := `<form method="POST" action="/x"><input name="a"></form>` +
		`<form method="GET" action="/y"></form>`
	out := s.injectFormToken(in)
	if !strings.Contains(out, `name="`+uiTokenField+`" value="tok"`) {
		t.Fatal("hidden token not injected into POST form")
	}
	// GET form must not get a token.
	getPart := out[strings.Index(out, `method="GET"`):]
	if strings.Contains(getPart, uiTokenField) {
		t.Fatal("token wrongly injected into GET form")
	}
}
