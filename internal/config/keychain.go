package config

// keychain.go — H8: keep the companion bearer token(s) in the macOS login
// Keychain instead of plaintext in config.json.
//
// Design (minimal-blast-radius): the in-memory config.Config API is
// UNCHANGED — ActivePair().Token / CompanionToken still return the real
// token, so ws_client / tasks_worker / listen / upload need no edits. Only
// the on-disk projection changes:
//
//   Save():  write each token to the Keychain (account = pair ID, and a
//            fixed account for the legacy scalar), and marshal a REDACTED
//            copy of the config where every stored token is replaced by
//            keychainMarker. Keychain I/O happens OUTSIDE the config mutex.
//   Load():  after unmarshal, any token equal to keychainMarker (or empty
//            with a matching Keychain entry) is re-hydrated from the
//            Keychain.
//
// Fallback: if the Keychain is unavailable (headless / no GUI session —
// `security` returns "User interaction is not allowed", or any error), we
// FALL BACK to writing the plaintext token in the JSON, so nothing breaks
// (mirrors snth-cli's env -> keychain -> file chain). Reads then just use
// the plaintext already in the JSON.

import (
	"errors"
	"os/exec"
	"strings"
)

// keychainMarker replaces the plaintext token in config.json when the
// token has been stored in the Keychain. Load treats this (or empty) as
// "look in the Keychain".
const keychainMarker = "keychain:"

// keychainService is the generic-password service name.
const keychainService = "ai.snth.companion"

// legacyKeychainAccount is the account used for the deprecated scalar
// CompanionToken (which has no pair ID of its own).
const legacyKeychainAccount = "__legacy_companion_token__"

// keychainStore is the seam. The default impl shells out to `security`;
// tests inject a fake map so they never touch the real login keychain.
type keychainStore interface {
	// Set stores secret under account. Returns an error when the Keychain
	// is unavailable (headless etc.) so callers can fall back to plaintext.
	Set(account, secret string) error
	// Get returns the secret for account. ErrKeychainMiss when absent.
	Get(account string) (string, error)
	// Delete removes the entry for account (no error if already absent).
	Delete(account string) error
}

// ErrKeychainMiss is returned by Get when no entry exists for the account.
var ErrKeychainMiss = errors.New("keychain: no entry")

// ErrKeychainUnavailable is returned when the Keychain can't be reached at
// all (no GUI session, user interaction not allowed). Signals "fall back
// to plaintext".
var ErrKeychainUnavailable = errors.New("keychain: unavailable")

// kc is the active backend. Overridden in tests. Default = `security` CLI.
var kc keychainStore = securityKeychain{}

// securityKeychain implements keychainStore via the macOS `security` CLI.
// Preferred over cgo Security.framework for testability + no extra linkage.
type securityKeychain struct{}

func (securityKeychain) Set(account, secret string) error {
	// -U updates an existing item in place instead of erroring on dup.
	cmd := exec.Command("security", "add-generic-password",
		"-a", account, "-s", keychainService, "-w", secret, "-U")
	out, err := cmd.CombinedOutput()
	if err != nil {
		if isKeychainUnavailable(string(out)) {
			return ErrKeychainUnavailable
		}
		return err
	}
	return nil
}

func (securityKeychain) Get(account string) (string, error) {
	cmd := exec.Command("security", "find-generic-password",
		"-a", account, "-s", keychainService, "-w")
	out, err := cmd.CombinedOutput()
	if err != nil {
		s := string(out)
		if isKeychainUnavailable(s) {
			return "", ErrKeychainUnavailable
		}
		// find-generic-password exits 44 / "could not be found" when absent.
		if strings.Contains(s, "could not be found") || strings.Contains(s, "SecKeychainSearch") {
			return "", ErrKeychainMiss
		}
		return "", ErrKeychainMiss
	}
	// -w prints the password + trailing newline.
	return strings.TrimRight(string(out), "\n"), nil
}

func (securityKeychain) Delete(account string) error {
	cmd := exec.Command("security", "delete-generic-password",
		"-a", account, "-s", keychainService)
	out, err := cmd.CombinedOutput()
	if err != nil {
		s := string(out)
		if strings.Contains(s, "could not be found") {
			return nil // already gone
		}
		if isKeychainUnavailable(s) {
			return ErrKeychainUnavailable
		}
		return err
	}
	return nil
}

func isKeychainUnavailable(out string) bool {
	return strings.Contains(out, "User interaction is not allowed") ||
		strings.Contains(out, "-25308") // errSecInteractionNotAllowed
}

// isMarker reports whether a stored token value means "the real token is
// in the Keychain" rather than being the token itself.
func isMarker(tok string) bool {
	return tok == "" || tok == keychainMarker
}
