# SNTH Connect: desktop app

The tray app that links a person's computer to their synth. A Tauri 2 shell
(`src-tauri/`, Rust) holds one `connect_core::Core` (`../crates/connect-core`)
and shows it through a React + TypeScript UI (`src/`).

Status: runs for real on macOS (one Mac linked to a live synth, the helper
installs itself, computer control works under the app's own permissions).
On Windows the app installs, pairs and links on a real machine, and the
computer-control path (driver install, our daemon on a named pipe, an MCP
session) passes a self-test on a Windows CI machine at every push. Linux has
not been used. What has never been exercised is listed under "Not verified"
at the end.

## Layout

```
app/
  index.html, approval.html   the two pages (main window, approval window)
  src/
    backend/    types.ts (the contract), tauri.ts (real), mock.ts (in-memory)
    i18n/       en.ts, ru.ts: every string the UI shows
    lib/        store.tsx (state), format.ts (journal lines, sizes, shortcuts)
    ui/         controls.tsx, icons.tsx
    screens/    Onboarding, Status, Internet, Computer, Files, Apps,
                Logins, Activity, General, HelperStatus
    approval/   ApprovalView.tsx (the request window)
    MockStage.tsx   browser-only frame + review controls
    styles/app.css
  src-tauri/
    src/main.rs        plugins, setup, link supervisor, event forwarder
    src/commands.rs    every command the webview may call
    src/approvals.rs   the Approver the core asks through
    src/actions.rs     pause / resume / quit / deep link / stop shortcut
    src/windows.rs     main + approval windows, navigation guard
    src/tray.rs        tray icon and menu
    src/updates.rs     automatic updates of the app itself
    src/menu.rs        macOS application menu (clean Cmd+Q, Edit menu)
    src/browsers.rs    where browsers keep cookies; the Chromium key source
    src/logins.rs      the Logins page: host manifests for the SNTH Vault
                       extension, the browser link, the vault list
    src/safari.rs      Safari (macOS): is the extension that ships in the
                       app switched on, open Safari's settings at it
    src/bin/snth-connect-host.rs   the helper the browsers start (see
                       "The SNTH Vault extension")
    windows/hooks.nsh  uninstall: remove the helper's registry keys
    src/state.rs       shared context and the UiState the UI renders
    src/prefs.rs       shell.json (onboarding, language, autostart, shortcut)
    build.rs           the command list (turns on per-command permissions)
    capabilities/      what each window may call
    tauri.conf.json, tauri.release.conf.json
  safari/              the Safari extension's native half (Swift): the relay
                       to the running app, its tests, the Xcode project of
                       the app extension (see safari/README.md)
```

## Review the UI in a browser (mock mode)

```
cd app
npm install
npm run dev          # http://127.0.0.1:1420
```

Outside Tauri (or with `VITE_MOCK=1`) every command and event goes to an
in-memory mock. The page shows the app inside a frame of the real window size
(760 x 560) with a strip of review buttons under it: simulate a request of
each kind, add activity, start a working stream, drop the link, open a pairing
link, switch language and theme.

URL parameters stage a screen or a state directly:

| Parameter | Values |
|---|---|
| `screen` | `onboarding`, `status`, `internet`, `logins` (`cookies` still works), `computer`, `files`, `apps`, `activity`, `general` |
| `logins` | what the synth's vault answers on the Logins page: `ready` (default: three passwords), `empty`, `asking` (never answers), `offline`, `outdated` (the synth is older than the feature), `novault`, `failed` |
| `ext` | the SNTH Vault extension in the browsers: `mixed` (default: Chrome linked with one site, Edge connected but not linked, Brave never heard from), `none`, `installed`, `linked`, `closed` (linked but the browser is closed, and one seen earlier), `nobrowser`, `busy` (another copy of the app holds the browser link) |
| `safari` | with `os=macos`, Safari's row (its extension ships inside the app): `off` (default: not switched on in Safari yet; "Open Safari settings" then plays the rest, switched on and then connected), `unknown` (Safari did not say), `on` (switched on, never connected), `moved` (the app runs from the disk image), `installed` (connected, not linked), `away` (connected earlier, not linked), `linked`, `closed` (linked, not connected right now), `allsites` (Safari lets the extension read every site), `both` (the stand-alone SNTH Vault app is installed too), `none` (this copy of the app has no extension: no row) |
| `link` | what "Link this browser" does: `ok` (default), `synth_offline`, `synth_outdated`, `other_synth`, `busy`, `failed` |
| `step` (with `screen=onboarding`) | `1` welcome, `2` code, `3` confirmation, `4` preset, `5` folders, `6` computer control, `7` done |
| `state` | `paused`, `working`, `connecting`, `offline`, `repair`, `empty`, `off`, `appsoff` (only Apps off), `metered` |
| `approval` | `net`, `cookies`, `cookiesext` (a sign-in a linked browser will ask about too: "Step 1 of 2"), `vault` (a browser asks to be linked: not the synth asking), `computer`, `foreground`, `files`, `other`, `queue` (shows the approval window alone); for the owner's apps: `message` (a message as the owner, with "always for this person"), `messageask` (the same when nothing may be remembered, with a hostile reason), `event`, `reminder`, `note`, `shortcut`, `shortcutinput`, `clipboard`, `openurl`, `openfile`, `openapp` |
| `apps` | the macOS permissions of the apps: `granted` (default), `noperm` (macOS has not asked yet), `denied`, `nodisk` (Messages may be controlled, its history cannot be read) |
| `messages` | `on`: the Messages switch, which is off by default |
| `preset` | `careful` (default), `balanced`, `full` |
| `cua` | the helper: `ready` (default), `checking`, `downloading` (frozen at 42%), `installing` (plays the whole automatic install, about 7 s), `verifying`, `failed` (with `why=offline`, `download`, `checksum`, `disk`, `verify`, `unsupported`, `timeout`); the macOS permissions: `noperm`, `half` |
| `update` | the app's own updates: `checking`, `downloading` (frozen at 40%), `ready` (downloaded, waiting for a quiet moment), `available` (the setting is off), `applying`, `applied` (first run after an update), `failed` (with `step=check`, `download`, `install`), `blocked` (with `block=translocated`, `read_only`, `package`), `flow` (plays check, download, ready), `off`, `unconfigured` |
| `pair` | a 6-digit code: behaves as if `snth://pair?code=...` was opened |
| `lang` | `en`, `ru` |
| `theme` | `light`, `dark` |
| `os` | `macos` (default), `windows`, `linux`. Apps follows it: the Mac apps are listed only with `macos` |

Examples: `?screen=logins`, `?screen=logins&logins=offline&ext=closed&os=windows`,
`?screen=logins&safari=off`, `?screen=logins&ext=linked&safari=closed&lang=ru`,
`?screen=logins&safari=allsites`, `?screen=logins&safari=both&lang=ru`,
`?screen=logins&logins=empty&state=empty&ext=none&lang=ru`, `?approval=vault`,
`?screen=onboarding&step=3`, `?screen=status&state=paused`,
`?approval=cookies`, `?screen=computer&cua=noperm&lang=ru&theme=dark`,
`?screen=apps&apps=noperm&messages=on`, `?screen=apps&os=windows`,
`?approval=message&lang=ru`.
`/approval.html` is the approval window's own page.

In the mock pairing step: any 6 digits work; `000000` is "not valid",
`111111` is "expired", `222222` is "server unreachable". In General, the
shortcut `Ctrl/Cmd+Shift+Q` is "taken by another app".

## Build

```
cd app
npm run build                                  # typecheck + bundle the UI
npm run tauri build -- --debug --bundles app   # unsigned debug .app (macOS)
npm run tauri build                            # release bundles for this OS
```

Rust commands from the repo root: `cargo test -p connect-core -p snth-connect`,
`cargo clippy --workspace`. `cargo run -p connectd -- selftest cua` runs the
computer-control path for real in a temp dir (it downloads the pinned driver
and starts a private daemon on the machine; the Windows CI job runs it on
every push). The workspace builds with Rust 1.85
(`rust-version` in the root `Cargo.toml`); `.cargo/config.toml` makes cargo
prefer dependency versions that support it, which is why `Cargo.lock` holds
Tauri 2.11 and `package.json` pins the matching `@tauri-apps/*` 2.11 packages.
Raise all three together.

A debug build keeps the pairing token in a 0600 file in the config dir; a
release build uses the OS keychain. Do not judge keychain behaviour from a
debug build.

## Releasing

Variant A: Windows and Linux are built by CI, macOS is built, signed and
notarized on the release Mac. A macOS update that is unsigned or signed by
someone else would make macOS forget the permissions a tester gave the app,
so the macOS half never comes from CI.

Every installed copy checks
`https://github.com/snth-ai/snth-companion/releases/latest/download/latest.json`
(shortly after launch, then about every six hours). "Latest" is the newest
PUBLISHED release of that repo that is not a pre-release, so a draft reaches
nobody, and no other kind of release may be published as "latest" there.

### The runbook

1. Bump the version in `app/package.json` (the only place; `tauri.conf.json`
   reads it). Commit.
2. Tag that commit `connect-v<version>` and push the tag.
3. Wait for the `connect-release` workflow to finish green. It creates a
   DRAFT release for the tag with the Windows installer, the Linux AppImage
   and .deb, their `.sig` files and `latest.json`.
4. On the release Mac, on that same commit:

   ```
   scripts/release-macos.sh <version>
   ```

   It builds the arm64 app signed with the Developer ID (hardened runtime),
   builds the Safari extension (the native handler with Xcode, the web
   extension from the committed tree of `../snth-vault-extension`), puts it
   into the app (`Contents/PlugIns/SNTH Vault.appex`) and signs inside out:
   the extension sandboxed and in the app group, then the app around it. It
   notarizes and staples it, checks it with `codesign` and `spctl`, makes a
   signed and notarized DMG (first install) and the `.app.tar.gz` of the
   stapled app with its updater signature, uploads all three to the draft
   release, and adds `darwin-aarch64` to the release's `latest.json`. It
   stops with an error if the release or `latest.json` is missing, if the
   built app is not `<version>`, or if any check fails. It can be run again.
   `--dry-run` does everything except notarization, upload and the
   `latest.json` edit; `--no-upload` keeps everything local;
   `--universal` makes one build for Apple silicon and Intel (needs
   `rustup target add x86_64-apple-darwin`, about twice the build time,
   never tried).
5. Check the draft: it must hold the `.dmg`, the `.app.tar.gz` and its
   `.sig`, the Windows `-setup.exe` (+ `.sig`), the `.AppImage` (+ `.sig`),
   the `.deb`, and a `latest.json` whose `platforms` lists `darwin-aarch64`,
   `windows-x86_64` and `linux-x86_64`. Install the DMG on a Mac once.
6. Publish the release (not as a pre-release). From that moment installed
   copies find it at their next check and update themselves.

If macOS is published late, Mac copies simply see no update for their
platform until `latest.json` has the darwin entry; nothing breaks.

What the script needs on the release Mac: the certificate `Developer ID
Application: Aleksandr Sabri (CZAP38JFAE)` and the notary profile
`SNTH_NOTARY` in the login keychain, the updater key at
`~/.tauri/snth-connect.key`, its password in the login keychain (service
`snth-connect-updater-key-password`), and `gh` signed in with write access
to the repo. Since the Safari extension: the full Xcode (`xcodebuild`; the
command line tools alone do not build an app extension) and the
`snth-vault-extension` repository next to this one, on the commit to ship
(`SNTH_VAULT_EXTENSION`, `SNTH_VAULT_REF`). Nothing new in the keychain and
no provisioning profile: the app group's id starts with the team id. The
header of the script lists the environment overrides.

What the script checks about the Safari extension before anything is
notarized: it is in the app, signed by the same identity with the hardened
runtime and a timestamp, sandboxed, in the app group
`CZAP38JFAE.ai.snth.connect` and entitled to nothing else; its bundle id is
`ai.snth.connect.safari`, under the app's own; its version is the app's; the
app is in the same group and kept the entitlements Tauri signed it with; and
the updater bundle, unpacked, still carries it signed and sandboxed. The
copies of the app the script makes are taken off the system's list of apps
at the end, so Safari on the release Mac does not show their extensions.

### What an installed copy does with an update

- With "Update automatically" on (the default): it downloads and verifies the
  new version in the background, then switches to it when nothing is in
  flight (the synth has no stream open, no request window is waiting, the
  helper is not being installed) and the person has been away from the
  computer for three minutes, or the app was started hidden at login and its
  window was never opened. It also switches when the person quits the app
  (without coming back, since they quit). After a switch at a quiet moment it
  comes back the way it was: tray only if the window was hidden, still paused if it was
  paused, and it links again by itself.
- With the setting off: General says that a version is available; the button
  installs it and restarts.
- Offline, feed errors and broken downloads are silent and tried again later.
- A failed install leaves the old version running (on macOS the app keeps a
  safety copy of itself during the switch and puts it back if the new bundle
  is missing or fails `codesign --verify`).
- Each update writes one line to Activity ("updated itself to version X"),
  and the synth learns the new version in the next hello.

### First install, per system

- **macOS**: download the `.dmg`, open it, DRAG "SNTH Connect" onto the
  Applications shortcut in the same window, then open it from Applications.
  This matters: an app opened straight from the disk image or from Downloads
  runs from a read-only copy that macOS makes, cannot update itself, and may
  lose its permissions at every start. The app says so on its first screen
  when it finds itself in that state.
- **Windows**: download the `-setup.exe` and run it. The installer is not
  signed with a Windows certificate, so SmartScreen shows "Windows protected
  your PC": click "More info", then "Run anyway". It installs for the
  current user, no administrator rights.
- **Linux**: the `.AppImage` is the one that updates itself: make it
  executable (`chmod +x`) and run it from a folder you own. The `.deb`
  installs like any package, but then the package manager owns the files:
  that copy only tells you a new version exists (General, with a button to
  the download page) and you install the new `.deb` yourself.

### Which version does a tester run

- In the app: General, Updates, "Version X". Diagnostics (same page) has
  "Copy diagnostics" with the version, the update state and the link state.
- From the synth's side: the app reports `app_version` in its hello; the
  synth's `device_status` tool and its `connect_devices` table show it.
- Activity shows the moment of each update.

### The updater key

Updates are accepted only when signed with the key whose public half is in
`src-tauri/tauri.conf.json` (`plugins.updater.pubkey`). The private half is
`~/.tauri/snth-connect.key` on the release Mac and the GitHub secret
`TAURI_SIGNING_PRIVATE_KEY` (password: `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`,
and the login keychain on the Mac). Keep a copy of both somewhere safe.

If the key or its password is lost: copies that are already installed can
never be updated again, because they trust only that key. The way out is a
new key (`npm run tauri signer generate -- -w ~/.tauri/snth-connect.key`),
its public half in `tauri.conf.json`, the new private half in the secrets
and the keychain, a new release, and every tester installing that release
by hand once. From then on updates work again. If the key leaks, do the
same: whoever has it can sign an update the apps would accept (they would
still have to get it into the release feed).

### Hub address

The pairing server is fixed at build time: `https://hub.snth.ai` unless the
environment variable `SNTH_HUB_URL` is set when the Rust crate is compiled
(`SNTH_HUB_URL=https://... npm run tauri build`, also honoured by the
release script and the workflow if set there). The onboarding screen has a
hidden "Advanced" override that accepts only `https://` addresses.

## How the shell is put together

- One `Core`, built in `setup`. The link runs under a small supervisor that
  restarts it after a new pairing (`Core::run_link` returns after an unlink).
- Approvals: the core calls `Approver::ask`; the shell queues the request,
  opens the approval window, fires a notification, and resolves the future on
  the owner's click. No answer in 55 s, a closed window and a pause are all
  refusals.
- Pause is remembered (`shell.json`): a relaunch stays paused until the owner
  resumes.
- Updates of the app itself are in `src/updates.rs`: the schedule, the quiet
  moment, the macOS safety copy, and the relaunch (a small shell waits for
  the old process to end and opens the app through the system, so macOS
  attributes the new process to the app itself and the single-instance check
  finds nothing in the way). What blocks self-update is detected at start:
  a translocated or unwritable location on macOS, a system package on Linux.
- The helper for computer control (Cua Driver) is the app's own pinned copy
  in `<app data>/cua/releases/<version>/`. `spawn_helper` in `main.rs` checks
  it at every launch, in the background, and installs it when it is missing
  or when this app version pins another one; a failure is retried after 30 s,
  2 min, 10 min, 30 min, then hourly, and at the next launch. The download is
  the release's bare-binary archive from GitHub, accepted only if its SHA-256
  equals the value compiled into `connect-core/src/cua_install.rs`. The
  official installer is not used: it cannot install privately (see the top of
  that file). A driver the owner installed themselves is never used, changed
  or stopped. Raising the pin = change `cua::PINNED_VERSION` and the hash
  table in `cua_install.rs` together.
- macOS permissions for computer control are this app's own (the helper runs
  embedded, as the app's child): the app checks and requests Accessibility and
  Screen Recording itself (`connect-core/src/tcc.rs`), and they show up under
  "SNTH Connect" in System Settings.
- Pairing links: `snth://pair?code=XXXXXX` only brings the window up and hands
  the code to the UI, which asks the hub who the code belongs to and shows the
  confirmation screen. The token is stored and the link opened only on the
  Connect click. If the computer is already linked, a notice says so and
  nothing changes.
- Webview permissions: `build.rs` lists every app command, which makes Tauri
  enforce `capabilities/*.json` per window. The main window gets the app's
  commands and the event listener; the approval window gets five commands.
  There is no shell plugin and no fs plugin; the folder picker is opened from
  Rust, so the webview never supplies a path; "open in browser" takes one of
  four fixed targets, never a URL. Windows refuse to navigate anywhere but
  the app's own pages, and the CSP is `default-src 'self'` plus the IPC
  endpoint.
- macOS: `src/menu.rs` replaces the default application menu so Cmd+Q goes
  through the same clean shutdown as the tray's Quit; `Info.plist` holds the
  sentences macOS shows when it asks about the Desktop, Downloads, Documents,
  the local network and control of other apps.
- The owner's apps (the Apps page): the core does the work
  (`connect-core/src/apps/`), the shell adds three things. It shows the
  synth's notifications as this app (`ui::AppNotifier`, the same plugin as
  its own notifications). It reports where each app's macOS permission
  stands (`apps_check`, read-only) and makes macOS ask on the owner's click
  (`apps_request_permission`). And the request window shows the action
  itself (`ApprovalDto.details`: the recipient and the whole text of a
  message, the title and time of an event) and offers only the answers the
  core allows for it (`remember`).
- macOS permissions of the Apps page. Contacts, Calendar, Reminders, Notes
  and sending in Messages go through Automation: macOS asks once per app
  ("SNTH Connect wants to control Calendar") the first time the synth uses
  it, or when the owner clicks "Ask now". That needs the entitlement
  `com.apple.security.automation.apple-events` (`Entitlements.plist`, wired
  as `bundle.macOS.entitlements`) and `NSAppleEventsUsageDescription`
  (`Info.plist`); `scripts/release-macos.sh` refuses to go on when the
  signed app lacks either. Reading Messages reads the history database and
  needs Full Disk Access, which no app can ask for: the page explains the
  step and opens the right pane. Shortcuts, the clipboard, "open" and
  notifications need nothing.
- Files on disk: `settings.json`, `shell.json` (and `secrets.json` in a debug
  build) in the app config dir; `journal.jsonl` in the app data dir.

## The SNTH Vault extension

One app, one pairing, one page. The browser extension that shares site
passwords with the synth (repo `snth-vault-extension`) talks to this app
when the app is installed, and works on its own when it is not.

THE RULE the Logins page states: a password the owner shares is used only
in the synth's cloud browser (it goes from the browser straight to the hub's
vault; this app never holds one). A sign-in session (cookies) is used only
for requests sent from this computer. Neither is ever used the other way
round.

- **The channel.** Native messaging. The browser starts
  `snth-connect-host`, a second small program in the app bundle (next to the
  app's own program: `Contents/MacOS/` on macOS, the install folder on
  Windows, `/usr/bin` in the Linux packages). It is a relay: it passes the
  extension's messages to the RUNNING app over a per-user endpoint (a Unix
  socket in a 0700 folder under the app's data folder; on Windows a named
  pipe only the same user can open) and tells the extension when the app is
  not running. It never starts the app. All of it is `crates/connect-host`
  (its README has the details and who can reach what).
- **Registering it.** `logins::sync` writes the host manifests for the
  installed browsers (Chrome, Edge, Brave, Chromium): per-user files on
  macOS and Linux, per-user registry keys on Windows. No administrator
  rights. It runs at every start (so the manifests follow an updated or
  moved app), when the computer is linked, and it removes them when the
  computer is unlinked. An app that runs from a temporary place (an
  AppImage, or a copy macOS made because the app was opened outside
  Applications) registers a copy of the helper kept in its data folder. The
  Windows uninstaller removes the registry keys (`windows/hooks.nsh`); on
  macOS and Linux a removed app leaves a manifest that points at nothing,
  which a browser treats as "no such app".
- **One pairing.** In the extension's popup the owner clicks "Link this
  browser"; the app shows its request window ("Chrome asks", naming the
  browser and the synth); on yes the core asks the synth for a pairing code
  over the existing link and hands it to the extension, which redeems it at
  the hub like a typed code. "Link this browser" on the Logins page does the
  same without the window: the click is the confirmation.
- **Sign-ins through the extension.** When the synth asks for a site's
  cookies and the owner allowed the site, the core may ask a linked browser
  (first on Windows, where Chrome and Edge keep cookies locked to the
  browser; as a fallback elsewhere). The browser shows the extension's
  "Step 2 of 2" window and then its own prompt for that one site. Cookies
  travel extension, helper, app, synth, and are written nowhere on the way.
- **The Logins page** (`screens/Logins.tsx`, `logins.rs`): by site, the
  password in the synth's vault (when it was saved and last used; "Remove"
  deletes it from the vault through the synth) and the sign-in allowed from
  this computer ("Revoke", which also takes the site back in the browsers);
  the browsers with the state of the extension in each (never heard from,
  connected but not linked, linked) and "Get the extension", "Link this
  browser", "Unlink"; then the existing switches for sign-ins.
- The vault list is asked of the synth when the page opens
  (`logins_refresh`) and kept in memory only. A synth older than the feature
  is shown as "needs an update", an unreachable one as "cannot be reached
  right now"; sign-ins from this computer are listed either way.

### Safari

The same three things, another way in. Safari has no native messaging host:
its extension has to live inside an app. So the macOS app CARRIES the
extension (`Contents/PlugIns/SNTH Vault.appex`, built and signed by the
release script; Safari lists it as "SNTH Vault (Connect)") and the owner
installs one thing. `app/safari/README.md` has the mechanics; in short:

- **The channel.** The extension's JavaScript is the same code as in
  Chrome. Where Chrome opens a native port, Safari calls the app
  extension's native handler (Swift, sandboxed), which relays to the RUNNING
  app over a Unix socket in the folder of an app group the two are signed
  into (`~/Library/Group Containers/CZAP38JFAE.ai.snth.connect/run`). That
  is a second door of the browser link (`connect_host::Door::Safari`): the
  same messages and checks, the same user only, and each door takes its own
  callers only. A browser that comes in there is Safari whatever it says.
- **Switching it on is the owner's.** macOS shows the extension in Safari's
  settings once the app has been opened from Applications; every extension
  starts switched off, and no program can switch it on. The Logins page has
  Safari's row from the start and follows the owner through it: "switched
  off" with a button that opens Safari's settings at the extension
  (`SFSafariApplication.showPreferencesForExtension`), "switched on, not
  connected yet" (`SFSafariExtensionManager` is asked every few seconds
  while that row is waiting, and when the window comes back), then
  "connected, not linked" with "Link this browser", then "linked". An app
  opened from the disk image says that Safari cannot use it from there.
- **Pairing** is the same as in Chrome: "Link this browser" in the popup or
  on the Logins page, the owner's yes in the app, a code from the synth,
  redeemed at the hub by the extension. No permission is asked for in
  Safari at this point: talking to the app is part of what this copy is.
- **Sign-ins.** Safari's files are never read, so its extension is the only
  source of a Safari sign-in. The app asks it after the owner allowed the
  site; the extension opens its "Step 2 of 2" page and Safari asks, in its
  own prompt, whether the extension may access that one site. With more
  than one linked browser, one that has no cookie of the site is not the
  end: the next is asked.
- **What is different from Chrome, and shown as such.** Safari stops an
  extension it finds idle. The extension keeps itself awake while Safari
  runs (an open port it writes to, and a timer as a fallback); a linked
  Safari that is not connected reads "not connected right now" with what to
  do, not "closed". In Safari the owner can also give an extension every
  website at once ("Always Allow on Every Website"): Safari then asks
  nothing per site, the extension reports it, and the row says that the
  answers given in the app are the only gate. A site allowed "for one day"
  is asked about again by Safari later.
- **The stand-alone "SNTH Vault" app for Safari** (extension 0.1.0, paired
  with a code, passwords only) stays for people without this app. The two
  are separate extensions with separate pairings; with both installed
  Safari lists both, and the Logins row says which is which and that the
  other can be switched off. The stand-alone one never talks to the app.
- A debug build carries no extension: no Safari row, and the Safari door
  stays shut (an unsigned program is not in the app group, and macOS would
  ask the owner whether it may open that folder).

### Firefox

Not done. The extension's code would run there (it already uses `browser.*`
where it exists), but it needs three things this work did not include: a
`browser_specific_settings.gecko.id` and a background script next to the
service worker in the manifest; a host manifest of Firefox's own kind
(`allowed_extensions` instead of `allowed_origins`, in Mozilla's folders or
registry key; the helper is then started with the manifest path and the
extension id instead of an origin); and a signature from addons.mozilla.org,
without which Firefox loads an extension only until it restarts. Firefox
cookies are read from its files on every system already, so the gain would
be pairing without a code.

## Windows: what else could be driven, and why it is not built

Built on Windows: notifications, the clipboard, and open (a link, a
document in a shared folder, a Start menu app). Looked at and left out:

| Target | Without extra installs | Without admin | How solid | Verdict |
|---|---|---|---|---|
| Classic Outlook (mail, calendar, contacts, tasks) through COM (`Outlook.Application`) | only where classic Outlook is installed; new PCs get the new Outlook | yes | works, with traps: it starts Outlook, can raise Outlook's own "a program is trying to access" window, and Microsoft does not support it without a signed-in desktop session | the one real candidate; not built (see below) |
| New Outlook for Windows | preinstalled on Windows 11 | yes | no COM, no VBA, no MAPI, by design; only web add-ins and Microsoft Graph | not scriptable on the computer |
| Windows Mail, Calendar, People | support ended on 31 December 2024; they no longer sync | - | their stores are empty or stale | nothing to drive |
| Windows contact and appointment stores (WinRT) | yes | yes | need a packaged app identity; with the apps above retired nothing fills them | nothing to read |
| Microsoft To Do | preinstalled on many PCs | yes | no local interface at all; Microsoft Graph only (a cloud sign-in) | belongs in the cloud |
| Phone Link (SMS through the phone) | yes | yes | no API; only clicking through its window | computer control can do it, an API cannot |
| PowerShell | yes | yes | it is a shell, not an app: "run a command" is the developer-mode package | later package |

Recommendation: do not add Outlook now. Classic Outlook is being replaced,
the new one cannot be scripted, and mail, calendar, contacts and tasks of a
Microsoft account are reachable from the synth's side through Microsoft
Graph with a normal sign-in, which works the same on every computer and
needs nothing installed. If testers who live in classic Outlook ask for it,
the shape is: offer an `outlook` app only when `Outlook.Application` is
registered and the classic `OUTLOOK.EXE` exists, drive it from a hidden
PowerShell child through COM, classify sending mail like sending a message
(asked every time, whole text shown). A day of work plus a real machine
with classic Outlook to verify on; the CI machines have none.

## Not verified

- The SNTH Vault extension with the INSTALLED app, on any system. What was
  run: the core and the helper as real processes (`connectd selftest host`,
  also on Windows CI with the shipped helper and real per-user registry
  keys), and the whole chain in Chrome for Testing with a throwaway profile
  (`e2e/run.js` in the extension repo). Never run: the app writing manifests
  into the real folders of Chrome, Edge, Brave or Chromium; a real browser
  finding the helper inside the installed app; the helper inside a signed
  and notarized macOS bundle; the request window for a browser link; the
  Logins page against a real synth and a real vault; the browsers' own two
  prompts being accepted by a person (no script can click them).
- Chrome and Edge on Windows through the extension: built for exactly that,
  proven only as far as the Windows self-test goes (the helper, the pipe,
  the registry). No browser was driven on Windows.
- Safari, from the moment Safari itself is involved. What was run: the
  relay's own tests (`swift test`); the whole path with the relay in a test
  host against a real core on a real socket (`connectd selftest safari`);
  the same pair signed the way the release signs them, the relay sandboxed,
  meeting in an app group folder (`scripts/safari-sandbox-probe.sh`, macOS
  26.6); the extension's JavaScript against a stand-in for the handler; the
  two calls into SafariServices against classes of the test's own; the app
  extension built with Xcode, signed and checked by the release script's dry
  run. NEVER run, because it takes a person in Safari: the extension showing
  up in Safari's settings and being switched on; Safari starting the app
  extension and the handler reaching the app from inside Safari's own
  sandbox for extensions; `SFSafariExtensionManager` answering the signed
  app; the popup; pairing; Safari's prompt for a site and what its `cookies`
  API then returns (in particular HttpOnly cookies and cookies of a
  subdomain); whether Safari really keeps the background script awake while
  the port is written to (read in WebKit's source, not observed); the page
  that asks for a site opening as a small window or as a tab; notarization
  of an app with an app extension in it. The order to try it in:

  1. Install the release into Applications, open it, link it to a synth.
     Logins shows Safari "switched off". Likely failure: no Safari row (the
     app extension is not in the bundle, or the app runs from the disk
     image: the row then says so).
  2. "Open Safari settings": Safari opens Settings > Extensions with "SNTH
     Vault (Connect)" in the list. Likely failures: Safari opens without the
     list (then find it by hand); the extension is not listed (run
     `pluginkit -m -i ai.snth.connect.safari`: no line means macOS did not
     register it; open the app from Applications once more).
  3. Tick it. Within a few seconds the row reads "switched on", then
     "connected, not linked". Likely failure: it stays "switched on, not
     connected": click the SNTH Vault button in Safari's toolbar; if the
     popup says the app is not running, the handler cannot reach the app
     (General > diagnostics has a "safari:" line with the door's state).
  4. "Link this browser" on the Logins page (or in the popup, then "Allow"
     in the app's window): the row reads "linked", the popup "Linked through
     SNTH Connect". Likely failure: the hub refuses the code (Safari did not
     let the extension call the hub: the popup then shows a network error).
  5. Share one password from a sign-in page with the popup: unchanged code,
     never run in Safari either.
  6. Ask the synth for something that needs a sign-in on one site (signed in
     in Safari, not in Chrome). Allow in the app ("Step 1 of 2" is shown when
     the extension goes first), then the extension's page ("Step 2 of 2"),
     then Safari's own prompt. The synth gets the cookies; the journal says
     "from safari-extension". Likely failures: no page opens (the extension
     was asleep: the row says "not connected right now"); Safari's prompt
     does not appear from the page's button; the read comes back empty
     although signed in (what Safari's `cookies` API returns for that site).
  7. Revoke the site on the Logins page: it leaves the row's list, and
     Safari's settings no longer list the site for the extension.
  8. Leave Safari idle for ten minutes, look at the row: still "linked" and
     connected. If it reads "not connected right now", the keep-awake does
     not hold in this Safari and the fallback timer is all there is.
  9. Quit and reopen the app with Safari open: the row reconnects by itself
     within about ten seconds.

- The owner's apps on a real Mac. Every script compiles against the real
  apps' dictionaries (`scripts/check-applescripts.sh`) and everything
  around them is tested with stand-ins, but no script has ever been RUN
  against Calendar, Contacts, Reminders, Notes or Messages, the Messages
  history has only been read from a database built by a test, and the
  Automation question and the Full Disk Access step have never been seen
  on screen in the signed app. Calendar's scripting finds a repeating
  event only on the date of its first occurrence.
- On Windows the Apps self-test (`connectd selftest apps --touch`, run on
  every push) shows a toast through PowerShell, round-trips the clipboard,
  opens a text file and starts a Start menu app. The INSTALLED app shows
  notifications through its own plugin instead, which the self-test does
  not cover.
- Linux apps (`notify-send`, clipboard tools, `xdg-open`): compiled for
  Linux in isolation, never built into the app there and never run.

- An update round trip. It needs two published versions; see "Testing the
  first update" below. Until then the whole apply path (download, safety
  copy, install, relaunch through `open`, coming back hidden or shown) is
  compiled and unit-tested in pieces only.
- The release workflow as a whole, the Linux build, and the Windows update
  (the installer restarts the app; the app only stores how to come back).
- Windows, beyond install, pairing, the link and the CI self-test: the tray,
  autostart, the stop shortcut, notifications and the approval window on
  Windows; computer control from the installed app after the named-pipe fix
  (the self-test starts the daemon from a console program, the app from a
  windowed one); the screen-lock state (always reported as unlocked);
  browser sign-ins (Chrome on Windows is not readable, Firefox is).
- `scripts/release-macos.sh` was run with `--dry-run` only: notarization,
  stapling, the upload and the `latest.json` edit have never run.
- Beyond what the first real run showed (pairing, the link, the helper
  install, computer control), nothing tests the shell's windows, tray,
  autostart, stop shortcut and notifications.
- Metered-connection detection is not implemented, so "Do not use on metered
  connections" is shown as unavailable.

### Testing the first update

1. Release `0.1.x` as above and install it from the DMG into Applications.
   Link it, grant the permissions.
2. Release the next version (any visible change) and publish it.
3. In the installed app: General, "Check now". It should show the download,
   then "Version X is downloaded...". Click "Restart now": the window goes
   away and comes back as the new version, Activity has the update line,
   General says "Updated from version ...", the link is back, and the two
   macOS permissions are still granted (computer control still works).
4. Repeat with a third version without touching the app: leave it in the
   tray, lock the screen for a few minutes, and check that it switched by
   itself and came back tray-only.
5. The traps: open a copy straight from the mounted DMG and check that the
   first screen says to move it to Applications.
