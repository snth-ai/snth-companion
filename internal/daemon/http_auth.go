package daemon

import (
	"bytes"
	"crypto/rand"
	"crypto/subtle"
	"encoding/hex"
	"net"
	"net/http"
	"net/url"
	"strings"
)

// http_auth.go — local control-surface authentication + CSRF defense (B1).
//
// The UI HTTP server binds 127.0.0.1:<random>. Loopback alone is NOT a
// trust boundary: any other local process, and any web page in the user's
// browser (classic CSRF against loopback), could previously POST
// /api/trust/master (flip to auto-approve), /api/pair/* (unpair/re-pair),
// /api/listen/start (mic), /api/codex-login/*.
//
// Defense, both required for a mutation:
//  1. A per-launch random token the served UI embeds (meta tag + a
//     same-origin cookie). A blind local process never loaded a page, so
//     it has no token → rejected.
//  2. Same-origin Origin/Referer check. A browser sets Origin on
//     cross-site POSTs and JS cannot forge it, so evil.com's page is
//     rejected even though the cookie would ride along.
//
// GET/HEAD of read-only pages + /health + /api/status stay open (menubar
// liveness, page loads).

const uiTokenCookie = "snth_companion_ui_token"
const uiTokenHeader = "X-Companion-UI-Token"
const uiTokenField = "_ui_token"

// uiTokenMeta is the meta-tag name the SPA/pages read the token from.
const uiTokenMeta = "snth-companion-ui-token"

func newUIToken() (string, error) {
	b := make([]byte, 32)
	if _, err := rand.Read(b); err != nil {
		return "", err
	}
	return hex.EncodeToString(b), nil
}

// openReadPaths are GET endpoints readable without a token (liveness).
var openReadPaths = map[string]bool{
	"/health":     true,
	"/api/status": true,
}

// guard wraps the mux with the loopback + token + same-origin checks.
func (s *UIServer) guard(h http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		host, _, err := net.SplitHostPort(r.RemoteAddr)
		if err != nil {
			host = r.RemoteAddr
		}
		if host != "127.0.0.1" && host != "::1" && host != "localhost" {
			http.Error(w, "localhost only", http.StatusForbidden)
			return
		}

		// Reads (GET/HEAD) are open — they can't change state and the SPA
		// needs to load. We still set the token cookie on GET so the SPA's
		// subsequent fetch()es carry it.
		if r.Method == http.MethodGet || r.Method == http.MethodHead {
			s.setTokenCookie(w)
			h.ServeHTTP(w, r)
			return
		}

		// Explicit read-only APIs stay open even for odd methods.
		if openReadPaths[r.URL.Path] {
			h.ServeHTTP(w, r)
			return
		}

		// --- mutation: require same-origin AND a valid token ---
		if !s.sameOrigin(r) {
			http.Error(w, "cross-origin request rejected", http.StatusForbidden)
			return
		}
		if !s.validToken(r) {
			http.Error(w, "missing or invalid UI token", http.StatusForbidden)
			return
		}
		h.ServeHTTP(w, r)
	})
}

// setTokenCookie sets the per-launch token as a same-origin, HTTP-only-off
// cookie (the SPA JS needs to read the meta tag; the cookie is what rides
// on fetch). SameSite=Strict so it never leaves the origin.
func (s *UIServer) setTokenCookie(w http.ResponseWriter) {
	http.SetCookie(w, &http.Cookie{
		Name:     uiTokenCookie,
		Value:    s.Token,
		Path:     "/",
		SameSite: http.SameSiteStrictMode,
	})
}

// validToken accepts the token from the header, a form field, or the
// same-origin cookie. Constant-time compare.
func (s *UIServer) validToken(r *http.Request) bool {
	if tokenEqual(r.Header.Get(uiTokenHeader), s.Token) {
		return true
	}
	if c, err := r.Cookie(uiTokenCookie); err == nil && tokenEqual(c.Value, s.Token) {
		return true
	}
	// Form field (legacy form POSTs). ParseForm is safe to call here; the
	// downstream handler calls it again idempotently.
	if err := r.ParseForm(); err == nil {
		if tokenEqual(r.PostFormValue(uiTokenField), s.Token) {
			return true
		}
	}
	return false
}

func tokenEqual(got, want string) bool {
	if got == "" || want == "" {
		return false
	}
	return subtle.ConstantTimeCompare([]byte(got), []byte(want)) == 1
}

// sameOrigin verifies the request's Origin (or, absent Origin, Referer)
// host:port matches the server's own. A request with NEITHER header is
// allowed to fall through to the token check (non-browser callers don't
// set them; browsers always set Origin on cross-site state-changing
// requests, so a CSRF page is caught here).
func (s *UIServer) sameOrigin(r *http.Request) bool {
	origin := r.Header.Get("Origin")
	if origin == "" {
		origin = r.Header.Get("Referer")
	}
	if origin == "" {
		// No Origin/Referer — rely on the token check. A cross-site
		// browser POST always carries Origin, so this path is only hit by
		// same-machine non-browser tools, which still need the token.
		return true
	}
	u, err := url.Parse(origin)
	if err != nil {
		return false
	}
	return hostPortMatches(u.Host, s.Addr)
}

// hostPortMatches compares a URL host against the server addr, treating
// localhost and 127.0.0.1 as equivalent and requiring the port to match.
func hostPortMatches(urlHost, serverAddr string) bool {
	_, serverPort, err := net.SplitHostPort(serverAddr)
	if err != nil {
		return false
	}
	uh, up, err := net.SplitHostPort(urlHost)
	if err != nil {
		// No port in the URL host — cannot be our host:port.
		return false
	}
	if up != serverPort {
		return false
	}
	switch uh {
	case "127.0.0.1", "localhost", "::1", "[::1]":
		return true
	default:
		return false
	}
}

// spaHandler wraps the embedded SPA file server so index.html gets the UI
// token injected as a meta tag before it reaches the browser.
func (s *UIServer) spaHandler(inner http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		p := strings.TrimPrefix(r.URL.Path, "/")
		isIndex := p == "" || p == "index.html" || !strings.Contains(p, ".")
		if !isIndex {
			inner.ServeHTTP(w, r)
			return
		}
		// Capture the inner response so we can inject the meta tag.
		rec := &bufferingWriter{header: http.Header{}, status: 200, body: &bytes.Buffer{}}
		inner.ServeHTTP(rec, r)
		body := rec.body.Bytes()
		if bytes.Contains(body, []byte("<head>")) {
			body = bytes.Replace(body, []byte("<head>"),
				[]byte("<head>"+s.tokenMetaTag()), 1)
		}
		for k, vs := range rec.header {
			for _, v := range vs {
				w.Header().Add(k, v)
			}
		}
		w.Header().Del("Content-Length")
		w.WriteHeader(rec.status)
		_, _ = w.Write(body)
	})
}

func (s *UIServer) tokenMetaTag() string {
	return `<meta name="` + uiTokenMeta + `" content="` + s.Token + `">`
}

// injectFormToken inserts a hidden _ui_token input immediately after every
// POST <form> opening tag in the given HTML body, so legacy form
// submissions carry the token even if the cookie is somehow absent.
func (s *UIServer) injectFormToken(body string) string {
	hidden := `<input type="hidden" name="` + uiTokenField + `" value="` + s.Token + `">`
	var out strings.Builder
	rest := body
	for {
		i := strings.Index(rest, `<form`)
		if i < 0 {
			out.WriteString(rest)
			break
		}
		// Find the end of the opening tag.
		close := strings.IndexByte(rest[i:], '>')
		if close < 0 {
			out.WriteString(rest)
			break
		}
		tagEnd := i + close + 1
		tag := rest[i:tagEnd]
		out.WriteString(rest[:tagEnd])
		if strings.Contains(strings.ToLower(tag), `method="post"`) {
			out.WriteString(hidden)
		}
		rest = rest[tagEnd:]
	}
	return out.String()
}

// bufferingWriter captures a handler's response for post-processing.
type bufferingWriter struct {
	header http.Header
	status int
	body   *bytes.Buffer
}

func (b *bufferingWriter) Header() http.Header { return b.header }
func (b *bufferingWriter) WriteHeader(code int) { b.status = code }
func (b *bufferingWriter) Write(p []byte) (int, error) { return b.body.Write(p) }
