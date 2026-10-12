# SNTH Connect: desktop app

The tray app that links a person's computer to their synth. A Tauri 2 shell
(`src-tauri/`, Rust) holds one `connect_core::Core` (`../crates/connect-core`)
and shows it through a React + TypeScript UI (`src/`).

Status: runs for real on macOS (one Mac linked to a live synth, the helper
installs itself, computer control works under the app's own permissions).
Windows and Linux builds exist in CI but have not been used. What has never
been exercised is listed under "Not verified" at the end.

## Layout

```
app/
  index.html, approval.html   the two pages (main window, approval window)
  src/
    backend/    types.ts (the contract), tauri.ts (real), mock.ts (in-memory)
    i18n/       en.ts, ru.ts: every string the UI shows
    lib/        store.tsx (state), format.ts (journal lines, sizes, shortcuts)
    ui/         controls.tsx, icons.tsx
    screens/    Onboarding, Status, Internet, Computer, Files, Cookies,
                Activity, General, HelperStatus
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
    src/state.rs       shared context and the UiState the UI renders
    src/prefs.rs       shell.json (onboarding, language, autostart, shortcut)
    build.rs           the command list (turns on per-command permissions)
    capabilities/      what each window may call
    tauri.conf.json, tauri.release.conf.json
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
| `screen` | `onboarding`, `status`, `internet`, `cookies`, `computer`, `files`, `activity`, `general` |
| `step` (with `screen=onboarding`) | `1` welcome, `2` code, `3` confirmation, `4` preset, `5` folders, `6` computer control, `7` done |
| `state` | `paused`, `working`, `connecting`, `offline`, `repair`, `empty`, `off`, `metered` |
| `approval` | `net`, `cookies`, `computer`, `foreground`, `files`, `other`, `queue` (shows the approval window alone) |
| `preset` | `careful` (default), `balanced`, `full` |
| `cua` | the helper: `ready` (default), `checking`, `downloading` (frozen at 42%), `installing` (plays the whole automatic install, about 7 s), `verifying`, `failed` (with `why=offline`, `download`, `checksum`, `disk`, `verify`, `unsupported`, `timeout`); the macOS permissions: `noperm`, `half` |
| `update` | the app's own updates: `checking`, `downloading` (frozen at 40%), `ready` (downloaded, waiting for a quiet moment), `available` (the setting is off), `applying`, `applied` (first run after an update), `failed` (with `step=check`, `download`, `install`), `blocked` (with `block=translocated`, `read_only`, `package`), `flow` (plays check, download, ready), `off`, `unconfigured` |
| `pair` | a 6-digit code: behaves as if `snth://pair?code=...` was opened |
| `lang` | `en`, `ru` |
| `theme` | `light`, `dark` |
| `os` | `macos` (default), `windows`, `linux` |

Examples: `?screen=onboarding&step=3`, `?screen=status&state=paused`,
`?approval=cookies`, `?screen=computer&cua=noperm&lang=ru&theme=dark`.
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
`cargo clippy --workspace`. The workspace builds with Rust 1.85
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
to the repo. The header of the script lists the environment overrides.

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
  sentences macOS shows when it asks about the Desktop, Downloads, Documents
  and the local network.
- Files on disk: `settings.json`, `shell.json` (and `secrets.json` in a debug
  build) in the app config dir; `journal.jsonl` in the app data dir.

## Not verified

- An update round trip. It needs two published versions; see "Testing the
  first update" below. Until then the whole apply path (download, safety
  copy, install, relaunch through `open`, coming back hidden or shown) is
  compiled and unit-tested in pieces only.
- The release workflow as a whole, the Linux build, and the Windows update
  (the installer restarts the app; the app only stores how to come back).
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
