# connect-host

The channel between the SNTH Vault browser extension and the RUNNING SNTH
Connect app. Small on purpose: it carries messages and knows nothing about
what they mean (that is `connect-core/src/browser.rs`).

```
extension (service worker)
   |  chrome.runtime.connectNative("ai.snth.connect")
   v
helper process (`host`)              started by the browser, one per profile
   |  JSON lines over the per-user endpoint (`endpoint`, `transport`)
   v
the app (connect-core `browser`)     the only place with policy
```

| Module | What |
|---|---|
| `lib.rs` | `HOST_NAME` (`ai.snth.connect`), `EXTENSION_IDS` (who may call), `origin_allowed` |
| `frame` | the two wire forms of one message (native messaging: 32-bit length + JSON; the endpoint: one JSON object per line), the shape every message must have, the size limits |
| `host` | the helper: `host::main()`. Checks the origin the browser passed, relays both ways, says `{"type":"host","app":"not_running"}` while the app is not there and reconnects by itself |
| `endpoint` | where the app listens: a Unix socket under the user's home (macOS, Linux), a named pipe with the user's SID in its name (Windows) |
| `transport` | `Listener` (the app) and `connect` (the helper) over that endpoint, same user only |
| `install` | the native messaging host manifests: per-user files on macOS and Linux, per-user registry keys on Windows, only for browsers that are installed |
| `win` | Windows: the user's SID, a pipe access list with that one user, who owns a pipe, HKCU registry values |

## The helper

The same function is built twice: `connect-host` (this crate's own program,
for tests and `connectd selftest host`) and `snth-connect-host` (a second
program of the app's package, `app/src-tauri/src/bin/`, so the bundler ships
and signs it next to the app). A browser starts it with the calling
extension's origin as the first argument (`chrome-extension://<id>/`).

- Origin not in `EXTENSION_IDS`: one message `{"type":"host","error":"origin_not_allowed"}`, exit code 3.
- App not running: `{"type":"host","app":"not_running"}` at once and in
  answer to anything the extension sends; the helper stays and tries the
  endpoint every 1 to 10 seconds; `{"type":"host","app":"connected"}` when
  the app is there. Nothing sent while it was down is replayed.
- Connected: the first line to the app is
  `{"type":"host.open","v":1,"origin":"..."}`, then every well-formed
  message is passed on, both ways.
- What is not a JSON object with a string `type` is answered with
  `{"type":"host","error":"malformed"}` and never reaches the app. Types
  `host` and `host.*` are the helper's own: neither side can send them
  through it.
- A message over 2 MiB from the browser ends the helper (the stream cannot be
  resynchronized). A line from the app the browser would refuse (over 64 KiB)
  is dropped.
- The browser closes its end: the helper exits.

It holds no state, reads no file, opens no network connection and never
starts the app.

A DEBUG build takes the endpoint from `SNTH_CONNECT_HOST_ENDPOINT` when it is
set (so a test can point it at a test app). A release build ignores it.

## Who can reach the app through this

| Who | What stops it |
|---|---|
| a web page | native messaging is not available to pages; the extension declares no `externally_connectable` |
| another extension | the browser starts a host only for the origins in its manifest (`allowed_origins`), and the helper checks the origin again |
| another user of the computer | Unix: the socket is in a 0700 folder the user owns, mode 0600, and both ends compare the peer's uid. Windows: the pipe's access list holds the one user, remote clients are refused, and the helper hangs up on a pipe another account created |
| a program of the SAME user | nothing here: it can run the helper or open the endpoint itself. The app is built to give such a peer nothing (see `connect-core/src/browser.rs`) |

## Host manifests

`install::Locations::system(app_data).install(helper, EXTENSION_IDS)` writes,
for each installed browser (Chrome, Edge, Brave, Chromium):

- macOS: `~/Library/Application Support/<browser>/NativeMessagingHosts/ai.snth.connect.json`
- Linux: `~/.config/<browser>/NativeMessagingHosts/ai.snth.connect.json`
- Windows: `HKCU\Software\<browser>\NativeMessagingHosts\ai.snth.connect` =
  the path of one manifest file in the app's data folder

It writes only what differs, so the app calls it at every start (the
manifests follow an updated or moved app), and `remove()` takes ours away
from every browser. No administrator rights. `write_manifest(dir, ...)` puts
one manifest into a given folder: a throwaway browser profile in a test
(Chromium reads `<user data dir>/NativeMessagingHosts`).

Firefox is not registered: see the app README, "Firefox".

## Tests

`cargo test -p connect-host`: framing and its limits, the helper against
in-memory streams (relay, app down, app restart, malformed input, oversized
input), the endpoint (permissions, a second listener, a stale socket, a
folder that is not ours), the manifests (only installed browsers, a second
install writes nothing, a moved app, removal). `connectd selftest host` runs
the real helper process against a real endpoint and, on Windows, writes and
removes real HKCU keys under a host name of its own; the Windows CI job runs
it, once with the test helper and once with the helper the installer ships.
