// Package ui embeds the built React SPA (ui/dist) and serves it from
// the companion's HTTP root. The bundle is built by `cd ui && npm run
// build`, output lands in internal/ui/dist, Go embed.FS ships it into
// the single binary at compile time.
//
// Served at "/ui/*" and "/" (index.html + SPA fallback). Legacy
// server-rendered pages stay reachable at their old paths
// (/channels, /keys, /logs, /login/codex, etc.) during the
// port — the new React router exposes each of them as a Placeholder
// card linking back. Once every page is ported, the legacy handlers
// get deleted.
package ui

import (
	"embed"
	"io/fs"
	"net/http"
	"strings"
)

//go:embed all:dist
var distFS embed.FS

// Handler returns an http.Handler that serves the React bundle. GET
// on any path that's not a concrete file (and doesn't match a legacy
// /channels, /keys, /login, ... route) falls back to index.html so
// HashRouter's client-side routes resolve.
//
// The server should mount this on a prefix that does NOT intercept
// legacy handlers. Recommended: wrap with a 404-as-index fallback
// matcher that only kicks in for paths not otherwise registered.
func Handler() http.Handler {
	sub, err := fs.Sub(distFS, "dist")
	if err != nil {
		panic("ui: cannot open dist subfs: " + err.Error())
	}
	fileServer := http.FileServer(http.FS(sub))

	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		// Serve /assets/* + /favicon.svg + /icons.svg directly.
		// Anything else: fall through to index.html so the SPA's
		// client router takes over.
		p := strings.TrimPrefix(r.URL.Path, "/")
		if p == "" {
			p = "index.html"
		}
		if _, err := fs.Stat(sub, p); err != nil {
			r.URL.Path = "/index.html"
		}
		fileServer.ServeHTTP(w, r)
	})
}
