# SNTH Connect: desktop app

The tray app that links a person's computer to their synth. A Tauri 2 shell
(`src-tauri/`, Rust) holds one `connect_core::Core` (`../crates/connect-core`)
and shows it through a React + TypeScript UI (`src/`).

Status: the shell compiles, its unit tests pass and a debug `.app` bundle
builds. It has NOT been launched: nothing here was verified in a running
window, against a real synth, signed, or on Windows / Linux. See "Not
verified" at the end.

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
| `update` | `available`, `unconfigured`, `error` |
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

## Before a real release

### 1. Updater signing key (required for auto-update)

The app verifies every update against a public key in
`src-tauri/tauri.conf.json` (`plugins.updater.pubkey`). The repo ships the
placeholder `REPLACE_WITH_TAURI_UPDATER_PUBLIC_KEY`. While it is there the
updater plugin is not loaded and General shows "Updates are not set up in
this build".

Run once, on a machine you trust, and choose a password when asked:

```
cd app
npm run tauri signer generate -- -w ~/.tauri/snth-connect.key
```

This writes the private key to `~/.tauri/snth-connect.key` and the public key
to `~/.tauri/snth-connect.key.pub` (it is also printed).

- Public key: paste the contents of the `.pub` file as the value of
  `plugins.updater.pubkey` in `src-tauri/tauri.conf.json`, and commit that.
- Private key: the contents of `~/.tauri/snth-connect.key` go into the GitHub
  secret `TAURI_SIGNING_PRIVATE_KEY`; the password into
  `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. Never commit the private key.

Keep a safe copy of the private key and its password. If they are lost,
installed copies can never be updated again: they only accept updates signed
with that key.

The update feed is
`https://github.com/snth-ai/snth-companion/releases/latest/download/latest.json`.
`tauri-action` writes `latest.json` into the release when the signing key is
present. A release must be published (not a draft) to become "latest".

### 2. macOS signing and notarization (optional in CI)

`.github/workflows/connect-release.yml` signs and notarizes only when these
repository secrets exist; otherwise it builds unsigned and says so in a
warning.

| Secret | What |
|---|---|
| `APPLE_CERTIFICATE` | the Developer ID Application certificate, exported as `.p12`, base64 encoded |
| `APPLE_CERTIFICATE_PASSWORD` | the password of that `.p12` |
| `APPLE_SIGNING_IDENTITY` | for example `Developer ID Application: Name (TEAMID)` |
| `APPLE_API_KEY` | App Store Connect API key id (for notarization) |
| `APPLE_API_ISSUER` | the issuer id of that key |
| `APPLE_API_KEY_P8` | the contents of the `AuthKey_<id>.p8` file |

An unsigned macOS build is stopped by Gatekeeper for ordinary users. Signing
locally after an unsigned CI build is the other route.

### 3. Hub address

The pairing server is fixed at build time: `https://hub.snth.ai` unless the
environment variable `SNTH_HUB_URL` is set when the Rust crate is compiled
(`SNTH_HUB_URL=https://... npm run tauri build`). The onboarding screen has a
hidden "Advanced" override that accepts only `https://` addresses.

### 4. Version and release

The version lives in `app/package.json` (`tauri.conf.json` reads it). To
release: bump it, commit, push the tag `connect-v<version>`. The workflow
attaches the installers to a draft release; check them, then publish.

The workflow has never been run. Treat its first run as a test.

## How the shell is put together

- One `Core`, built in `setup`. The link runs under a small supervisor that
  restarts it after a new pairing (`Core::run_link` returns after an unlink).
- Approvals: the core calls `Approver::ask`; the shell queues the request,
  opens the approval window, fires a notification, and resolves the future on
  the owner's click. No answer in 55 s, a closed window and a pause are all
  refusals.
- Pause is remembered (`shell.json`): a relaunch stays paused until the owner
  resumes.
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

- The app was never launched: tray, window hiding, single instance, autostart,
  deep link, global shortcut, notifications, the approval window and the
  updater are written against the Tauri 2.11 APIs and compile, nothing more.
- No real pairing, no real link, no real approval round trip.
- No Keychain access (token store or the browser key), no real cookie read.
- The helper install was run for real once, into a temp dir, through the
  core's installer (download, hash check, unpack, `--version`). It was never
  run from the app, and the installed helper was never started as a daemon.
- The macOS permission flow (the app asking for Accessibility and Screen
  Recording itself, the embedded helper working under those grants) follows
  the driver's embedding guide but was never exercised: it needs a launched,
  signed app.
- Windows and Linux builds, the helper install there (the `.zip` is unpacked
  with the system `tar`), and the CI workflow.
- Metered-connection detection is not implemented, so "Do not use on metered
  connections" is shown as unavailable.
