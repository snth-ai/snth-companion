# Safari: the extension inside SNTH Connect.app

Safari gets what the Chromium browsers have: the SNTH Vault extension links
to the synth through the running app without a code, hands the app one
site's sign-in at a time, and has its row on the Logins page.

It cannot be done the Chromium way. Safari has no native messaging host: a
Safari web extension lives in an app extension (`.appex`) inside an app, and
`browser.runtime.sendNativeMessage` is delivered to that app extension's
native handler, one message in, one answer out. So the app carries the
extension, and the handler is the relay the Chromium helper is elsewhere.

```
extension JavaScript (Safari)            the SNTH Vault code, unchanged; src/safari.js
   |  browser.runtime.sendNativeMessage     makes the exchanges look like a port
   v
SafariWebExtensionHandler (this folder)  sandboxed, in the app group
   |  JSON lines over a Unix socket in the group's folder
   v
the app (connect-core `browser`)         the same room as for Chromium, a second door
```

## What is here

| Path | What |
|---|---|
| `Sources/ConnectRelay/Frame.swift` | the shape every message must have and its limits (the same rules as `crates/connect-host/src/frame.rs`) |
| `Sources/ConnectRelay/Socket.swift` | the handler's end of the app's socket, and the checks that it is the app of the same user |
| `Sources/ConnectRelay/Relay.swift` | the relay: sessions, the numbered list of what the app said, held answers, reconnecting |
| `Sources/ConnectRelay/Appex.swift` | the names (app group, bundle id, origin, socket) and the handler's entry point |
| `Extension/SafariWebExtensionHandler.swift` | the class Safari calls: 15 lines around `ConnectApp.answer` |
| `Extension/Info.plist`, `Extension.entitlements` | the app extension's plist and what it is signed with: the sandbox and the app group, nothing else |
| `Extension/SNTHVault.xcodeproj` | one target, the app extension. It lists the Swift files above; `scripts/build-safari-appex.sh` fails when one is missing from it |
| `Sources/connect-relay-harness` | a test host: the same relay outside Safari, one JSON line per native message |
| `Tests/ConnectRelayTests` | the relay against a stand-in for the app's socket |

The web extension itself is not in this repository. It comes from the
`snth-vault-extension` repository at build time (the committed tree), with
the manifest its `scripts/connect-safari.js` writes for this copy.

## The names

| What | Value | Also written in |
|---|---|---|
| app group | `CZAP38JFAE.ai.snth.connect` | `crates/connect-host/src/safari.rs`, `src-tauri/Entitlements.plist`, `Extension/Extension.entitlements` |
| extension bundle id | `ai.snth.connect.safari` | `safari.rs`, the Xcode project |
| what the handler names in its first line | `safari-web-extension://ai.snth.connect.safari/` | `safari.rs` |
| the socket | `~/Library/Group Containers/<group>/run/app.sock` | `safari.rs` |
| the app extension in the bundle | `Contents/PlugIns/SNTH Vault.appex` | `safari.rs`, `scripts/release-macos.sh` |
| the extension's name in Safari | `SNTH Vault (Connect)` | the extension's `scripts/connect-safari.js`, the app's strings |

A test in `connect-host` reads the Swift file and the entitlements and
fails when the names differ; the release script checks them again against
what was actually signed.

## How the handler reaches the app

An app group both programs are signed into, and a Unix socket in the
group's folder.

- The group id starts with the team id (`<team>.<name>`). macOS takes that
  form without a registration and without a provisioning profile, and lets
  only programs signed by that team into the group's folder.
- The app extension is sandboxed (Safari loads no other). Its entitlements
  are the sandbox and the group: no network, no files of the user's. A Unix
  socket in the group's folder needs neither.
- The app is not sandboxed. It names the group in its own entitlements so
  that macOS lets it create and open the folder without asking the owner.
- The app binds `run/app.sock` in a folder of mode 0700 (the socket 0600)
  and takes callers of the same user only. The handler checks the folder
  and the socket (owner, mode, a real folder and a real socket, not a link)
  and the user on the other end before it writes a byte.

`scripts/safari-sandbox-probe.sh` runs exactly this pair, signed the way the
release signs them, without Safari and without the app (a group of its own).
It was run on macOS 26.6: the sandboxed program reached the other one, and
macOS asked nothing.

## The exchange

```
extension -> handler   {"type":"relay","v":1,"sid":"<hex>","ack":7,"send":[<message>, ...],"wait":20000}
handler -> extension   {"type":"relay","v":1,"app":true,"messages":[{"seq":8,"msg":<message>}, ...]}
extension -> handler   {"type":"relay","v":1,"sid":"<hex>","close":true}
extension -> handler   {"type":"relay.ping"}        (on the keep-awake port; answered {"type":"relay.pong"})
```

- `sid` is the random id the extension made for its Safari profile. The
  relay keeps one connection to the app per sid, opened with
  `{"type":"host.open","v":1,"origin":"safari-web-extension://ai.snth.connect.safari/"}`.
- Messages in `send` go to the app as lines; what is not a typed object, or
  carries the type `host` or `host.*`, is answered with
  `{"type":"host","error":"malformed"}` and never reaches the app.
- What the app says is numbered and kept until the extension names the
  number in `ack`. An answer Safari never delivered is therefore sent again,
  and a background script Safari stopped and started again (it returns with
  `ack` 0) is handed what arrived meanwhile. At most 256 wait; beyond that
  the session ends.
- `wait` (at most 25 s) lets the handler hold an answer until the app has
  something to say. A request that carries messages uses 0.
- The relay's own words are the helper's: `{"type":"host","app":"connected"}`
  when it reached the app, `{"type":"host","app":"not_running"}` when it
  cannot (it tries again every 1 to 10 seconds by itself). `app` in every
  answer says where that stands right now.
- A session nobody asked about for 90 seconds is closed; the app then shows
  that browser as not connected.

Safari stops an idle background script after 30 seconds, and keeps one that
has an open port it wrote to in the last two minutes (read in WebKit's
source, `WebExtensionContext::unloadBackgroundContentIfPossible`; not
observed in Safari). The extension therefore keeps one `connectNative` port
open and writes a ping to it every 20 seconds, asks the browser for a timer
that restarts the script within a minute should Safari stop it anyway, and
keeps its requests under 30 seconds.

## Build, test

```
swift test --package-path app/safari                 # the relay (22 tests)
swift build --package-path app/safari                # the test host
cargo build -p connectd
target/debug/connectd selftest safari --relay app/safari/.build/debug/connect-relay-harness
scripts/safari-sandbox-probe.sh                      # the same, sandboxed and in an app group
scripts/build-safari-appex.sh /tmp/out               # the unsigned .appex (Xcode)
```

`connectd selftest safari` is the whole path without Safari: the test host
runs the relay, a real core serves the Safari door on a real socket, and the
test plays the extension's JavaScript. It also shows each side refusing a
peer of another user (each in turn is told to expect one).

The release (`scripts/release-macos.sh`) builds the app extension, puts it
into the app, signs it with its own entitlements and then the app around
it, and checks the result. Nothing else signs or installs it. A debug build
of the app carries no extension, shows no Safari row and does not open the
Safari door.

## What was not seen in Safari

Everything that needs a person: the extension in Safari's settings,
switching it on, the popup, Safari's prompt for a site, and what Safari's
`cookies` API returns. See "Not verified" in `app/README.md` for the list
and the order to try it in.
