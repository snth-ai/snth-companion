// Package codexlogin runs the OpenAI Codex (ChatGPT subscription) OAuth
// flow and returns the credential JSON that hub's /keys/upsert accepts
// for kind=oauth_subscription.
//
// Ported from snth-hub/cmd/codex-login — identical client_id, scopes,
// and redirect_uri. The redirect URI is pinned to localhost:1455 at
// OpenAI's end, so we bind a dedicated listener on that port for the
// duration of the flow.
package codexlogin

import (
	"context"
	"crypto/rand"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strings"
	"time"
)

const (
	clientID     = "app_EMoamEEZ73f0CkXaXp7hrann"
	authorizeURL = "https://auth.openai.com/oauth/authorize"
	tokenURL     = "https://auth.openai.com/oauth/token"
	redirectURI  = "http://localhost:1455/auth/callback"
	scope        = "openid profile email offline_access"
	jwtAuthClaim = "https://api.openai.com/auth"
	originator   = "openpaw"
	listenAddr   = "127.0.0.1:1455"
)

// Output is the JSON blob hub's /keys/upsert expects for
// kind=oauth_subscription.
type Output struct {
	AccessToken  string `json:"access_token"`
	RefreshToken string `json:"refresh_token"`
	ExpiresAt    int64  `json:"expires_at_unix_ms"`
	AccountID    string `json:"account_id"`
}

// Flow carries state between Start (generates auth URL, starts
// listener) and Finish (waits for callback, exchanges code).
type Flow struct {
	AuthURL  string
	verifier string
	state    string
	codeCh   chan string
	errCh    chan error
	srv      *http.Server
}

// Start generates PKCE + state, boots the localhost callback listener,
// and returns the auth URL the user should open in a browser.
func Start() (*Flow, error) {
	verifier, challenge, err := generatePKCE()
	if err != nil {
		return nil, err
	}
	state, err := randomHex(16)
	if err != nil {
		return nil, err
	}
	f := &Flow{
		verifier: verifier,
		state:    state,
		codeCh:   make(chan string, 1),
		errCh:    make(chan error, 1),
	}
	f.AuthURL = buildAuthURL(challenge, state)

	mux := http.NewServeMux()
	mux.HandleFunc("/auth/callback", func(w http.ResponseWriter, r *http.Request) {
		gotState := r.URL.Query().Get("state")
		gotCode := r.URL.Query().Get("code")
		if gotState != state {
			http.Error(w, "state mismatch", http.StatusBadRequest)
			select {
			case f.errCh <- errors.New("state mismatch"):
			default:
			}
			return
		}
		if gotCode == "" {
			http.Error(w, "missing code", http.StatusBadRequest)
			select {
			case f.errCh <- errors.New("missing code"):
			default:
			}
			return
		}
		fmt.Fprint(w, `<!doctype html><html><body style="font-family:system-ui;padding:40px;text-align:center;background:#0f172a;color:#e2e8f0">
<h2>OpenAI authentication complete.</h2>
<p>You can close this tab and return to SNTH Companion.</p>
</body></html>`)
		select {
		case f.codeCh <- gotCode:
		default:
		}
	})

	f.srv = &http.Server{Addr: listenAddr, Handler: mux}
	ready := make(chan error, 1)
	go func() {
		ready <- nil
		if err := f.srv.ListenAndServe(); err != nil && !errors.Is(err, http.ErrServerClosed) {
			select {
			case f.errCh <- fmt.Errorf("local server: %w", err):
			default:
			}
		}
	}()
	<-ready
	// Give the OS a beat to actually bind.
	time.Sleep(50 * time.Millisecond)
	return f, nil
}

// Finish blocks until the OAuth callback arrives (or timeout), then
// exchanges the code for tokens and returns the credential JSON. Shuts
// down the listener before returning.
func (f *Flow) Finish(timeout time.Duration) (*Output, error) {
	defer func() {
		ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
		defer cancel()
		_ = f.srv.Shutdown(ctx)
	}()

	var code string
	select {
	case code = <-f.codeCh:
	case err := <-f.errCh:
		return nil, err
	case <-time.After(timeout):
		return nil, errors.New("timed out waiting for OAuth callback")
	}

	tok, err := exchangeCode(code, f.verifier)
	if err != nil {
		return nil, err
	}
	accountID, err := extractAccountID(tok.AccessToken)
	if err != nil {
		return nil, err
	}
	return &Output{
		AccessToken:  tok.AccessToken,
		RefreshToken: tok.RefreshToken,
		ExpiresAt:    time.Now().Add(time.Duration(tok.ExpiresIn) * time.Second).UnixMilli(),
		AccountID:    accountID,
	}, nil
}

// Cancel tears down the listener early, e.g. if the user navigates
// away before completing the flow.
func (f *Flow) Cancel() {
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	_ = f.srv.Shutdown(ctx)
}

func generatePKCE() (verifier, challenge string, err error) {
	v := make([]byte, 32)
	if _, err := rand.Read(v); err != nil {
		return "", "", err
	}
	verifier = hex.EncodeToString(v)
	sum := sha256.Sum256([]byte(verifier))
	challenge = base64.RawURLEncoding.EncodeToString(sum[:])
	return verifier, challenge, nil
}

func randomHex(n int) (string, error) {
	b := make([]byte, n)
	if _, err := rand.Read(b); err != nil {
		return "", err
	}
	return hex.EncodeToString(b), nil
}

func buildAuthURL(challenge, state string) string {
	q := url.Values{}
	q.Set("response_type", "code")
	q.Set("client_id", clientID)
	q.Set("redirect_uri", redirectURI)
	q.Set("scope", scope)
	q.Set("code_challenge", challenge)
	q.Set("code_challenge_method", "S256")
	q.Set("state", state)
	q.Set("id_token_add_organizations", "true")
	q.Set("codex_cli_simplified_flow", "true")
	q.Set("originator", originator)
	return authorizeURL + "?" + q.Encode()
}

type tokenResponse struct {
	AccessToken  string `json:"access_token"`
	RefreshToken string `json:"refresh_token"`
	ExpiresIn    int    `json:"expires_in"`
	TokenType    string `json:"token_type"`
	IDToken      string `json:"id_token"`
}

func exchangeCode(code, verifier string) (*tokenResponse, error) {
	body := url.Values{}
	body.Set("grant_type", "authorization_code")
	body.Set("client_id", clientID)
	body.Set("code", code)
	body.Set("code_verifier", verifier)
	body.Set("redirect_uri", redirectURI)

	req, err := http.NewRequest("POST", tokenURL, strings.NewReader(body.Encode()))
	if err != nil {
		return nil, err
	}
	req.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return nil, fmt.Errorf("token request: %w", err)
	}
	defer resp.Body.Close()
	raw, _ := io.ReadAll(resp.Body)
	if resp.StatusCode != 200 {
		return nil, fmt.Errorf("token exchange failed: HTTP %d: %s", resp.StatusCode, string(raw))
	}
	var t tokenResponse
	if err := json.Unmarshal(raw, &t); err != nil {
		return nil, fmt.Errorf("decode token response: %w (body: %s)", err, string(raw))
	}
	if t.AccessToken == "" || t.RefreshToken == "" || t.ExpiresIn == 0 {
		return nil, fmt.Errorf("token response missing fields: %s", string(raw))
	}
	return &t, nil
}

func extractAccountID(accessToken string) (string, error) {
	parts := strings.Split(accessToken, ".")
	if len(parts) != 3 {
		return "", errors.New("access token is not a JWT")
	}
	raw, err := base64.RawURLEncoding.DecodeString(parts[1])
	if err != nil {
		raw, err = base64.URLEncoding.DecodeString(parts[1])
		if err != nil {
			return "", fmt.Errorf("decode JWT payload: %w", err)
		}
	}
	var claims map[string]any
	if err := json.Unmarshal(raw, &claims); err != nil {
		return "", fmt.Errorf("parse JWT claims: %w", err)
	}
	auth, ok := claims[jwtAuthClaim].(map[string]any)
	if !ok {
		return "", errors.New("JWT missing https://api.openai.com/auth claim")
	}
	id, ok := auth["chatgpt_account_id"].(string)
	if !ok || id == "" {
		return "", errors.New("JWT auth claim missing chatgpt_account_id")
	}
	return id, nil
}
