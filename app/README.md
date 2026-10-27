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
  index.html, approval.html, island.html
                the three pages (main window, approval window, the island)
  src/
    backend/    types.ts (the contract), tauri.ts (real), mock.ts (in-memory)
    i18n/       en.ts, ru.ts: every string the UI shows
    lib/        store.tsx (state), format.ts (journal lines, sizes, shortcuts)
    ui/         controls.tsx, icons.tsx
    screens/    Onboarding, Status, Internet, Computer, Files, Apps,
                Island, Observe ("What Mia sees"), Logins, Activity,
                General, HelperStatus
    island/     IslandApp.tsx (the island window), IslandStage.tsx (its
                browser review stage), faceApi.ts (the one door to the
                face module), face/ (the face: renderer, schema, presets,
                editor; its own README lines)
    approval/   ApprovalView.tsx (the request window)
    MockStage.tsx   browser-only frame + review controls
    styles/app.css
  src-tauri/
    src/main.rs        plugins, setup, link supervisor, event forwarder
    src/observe/       what the synths see (User Context reports): the
                       observe thread (mod.rs) and this system's probes:
                       mac.rs (CoreAudio, NSWorkspace, Accessibility,
                       CoreWLAN, CoreLocation), win.rs (the microphone's
                       consent store, the foreground window, the WLAN API)
    src/commands.rs    every command the webview may call
    src/approvals.rs   the Approver the core asks through
    src/actions.rs     pause / resume / quit / deep link / stop shortcut
    src/windows.rs     main + approval windows, navigation guard
    src/island/        the island: window + panel (panel.rs), notch math
                       (geometry.rs), hold keys (hold.rs), microphone
                       (audio.rs), screenshot (shot.rs), the synth's
                       spoken answer (speak.rs), the rest (mod.rs)
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

The island face (every look in every state, eyes on the mouse, the face
editor) has its own page: `http://127.0.0.1:1420/src/island/face/demo.html`
(`?view=grid|presets|states|editor`, more in `src/island/face/FaceDemo.tsx`);
its schema tests run with `node --test src/island/face/*.test.mjs`.

The face lab compares motion styles for pill eyes (Buddy) side by side:
`http://127.0.0.1:1420/src/island/face/lab.html` (0 = shipped, A = Cozmo
perspective + saccades, B = + gloss, C = + head layer and attention, C2 =
C made fluid, with live sliders; default view 0, C, C2, `?only=all` for
every column; the style is the `motionStyle` prop of `Face`, C2's live
values the `rigTuning` prop, logic in `src/island/face/rig.ts`, default
`classic`). Pill eyes (Buddy) use C2 (`smooth`) everywhere by default since
0.1.17 (`DEFAULT_PILL_MOTION` in `rig.ts`; set it to `"classic"` to roll
back); every other eye shape stays classic. The island tells the face where
the pointer is (on or just below its shape, via `attention.near`), when the
owner is busy with it (no boredom) and when the gaze is the synth's own
pointer (no startle or squint), and pauses it while tucked away; the face
also pauses while the page is hidden. Settled C2 draws about 20 frames a
second (breathing). The lab is a dev page only, not in `vite build`.
`node scripts/build-face-lab.mjs`
writes a self-contained single-file copy to `/tmp/face-lab/index.html`.

Under the columns the lab has "Наряди Дружка" (`DressLab.tsx`): Buddy in
C2 at 84 and 34 px with pickers for accessory, eye colour, shine, glow,
mouth, brows, lashes and blush, all eleven accessories live at once, and a
button that copies the look as a FaceConfig. `?dress=shots` (with
`cols=states` or `rows=colours|mouths|brows|lashes`) and `?dress=sweep`
show only its review matrices. On pill faces brows, cheeks and every
accessory go through the same turn as the beans (`pillWarp` in `rig.ts`,
drawn in `pillWear.tsx`), any eye colour gets a gloss shaded for it
(`glossStops` in `Face.tsx`), and the editor offers eye colour first, with
quick picks. Other eye shapes draw exactly as before.

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
| `approval` | `net`, `cookies`, `cookiesext` (a sign-in a linked browser will ask about too: "Step 1 of 2"), `vault` (a browser asks to be linked: not the synth asking), `computer`, `foreground`, `files`, `other`, `queue` (shows the approval window alone); for the owner's apps: `message` (a message as the owner, with "always for this person"), `messageask` (the same when nothing may be remembered, with a hostile reason), `event`, `reminder`, `note`, `shortcut`, `shortcutinput`, `clipboard`, `openurl`, `openfile`, `openapp`; for the command line: `shell` (the whole command, its folder and the shell, with "always this exact command") |
| `developer` | `1`: developer mode on, so General shows the Command line section (the switch asks once more with the warning; off by default, off in every preset) |
| `apps` | the macOS permissions of the apps: `granted` (default), `noperm` (macOS has not asked yet), `denied`, `nodisk` (Messages may be controlled, its history cannot be read) |
| `messages` | `on`: the Messages switch, which is off by default |
| `preset` | `careful` (default), `balanced`, `full` |
| `cursor` | Computer control, "How it looks": `on` (default), `off` (the rest of the pointer's rows disappear) |
| `color` | the pointer's colour: `auto` (default: the helper's own pointer, a colour per task), `sky`, `violet`, `rose`, `green`, `amber`, `red`, `graphite` |
| `speed` | `fast`, `normal` (default), `relaxed` |
| `idlehide` | hide the pointer when nothing happens: `5`, `15` (default), `60`, `0` (never) |
| `motion` | the pointer's animation: `system` (default), `always` (always still), `never` (always animated) |
| `shot` | under Advanced, the largest screenshot the synth gets: `0` (default: the helper decides), `1024`, `1568`, `2048`. Any other value than `0` opens Advanced |
| `pip` | under Advanced, the experimental preview window: `on`, `off` (default). Exists with `os=macos` only; `on` opens Advanced |
| `which` | Computer control, "Which apps": `all`, `except` (default here: two picked apps and one word typed by hand in an older version), `only` |
| `picker` | what "Add apps..." finds: `ok` (default: the apps of the staged system, after a second and a half), `slow` (still reading), `failed`, `missing` (no helper yet), `bare` (no password manager on this computer, so a new list of exceptions starts empty) |
| `task` | with `state=working`: the name the synth gave its computer task, shown on Status under "Right now" (default `Mia: flights`) |
| `health` | General, Diagnostics, "Check the helper": `ok` (default), `noperm`, `half` (macOS permissions), `nopointer`, `off`, `paused`, `missing` (no helper), `nodaemon`, `bounded`, `notbounded`, `slow` (never answers), `unknown` (a check this app has no words for) |
| `wayland` | `1` with `os=linux`: a desktop that draws the pointer itself, where only the on/off switch is offered |
| `cua` | the helper: `ready` (default), `checking`, `downloading` (frozen at 42%), `installing` (plays the whole automatic install, about 7 s), `verifying`, `failed` (with `why=offline`, `download`, `checksum`, `disk`, `verify`, `unsupported`, `timeout`); the macOS permissions: `noperm`, `half` |
| `update` | the app's own updates: `checking`, `downloading` (frozen at 40%), `ready` (downloaded, waiting for a quiet moment), `available` (the setting is off), `applying`, `applied` (first run after an update), `failed` (with `step=check`, `download`, `install`), `blocked` (with `block=translocated`, `read_only`, `package`), `flow` (plays check, download, ready), `off`, `unconfigured` |
| `pair` | a 6-digit code: behaves as if `snth://pair?code=...` was opened |
| `account` | the platform account: `signed_out` (first run, nothing linked: "Sign in with SNTH" waits about 2 s in the "browser", then shows the consent screen for Mia and Maya), `consent` (signed in, the consent screen), `two` (signed in, Mia and Maya linked), `none` (signed in, no synths: the code field), `repair` (two linked, Maya refused her token). Without it: signed out, Mia linked with a code, as before |
| `login` | with `account=signed_out`: the sign-in fails, `denied` (Cancel in the browser) or `expired` |
| `connectfail` | with `account=consent`: linking Maya fails, `connect_unavailable`, `synth_outdated`, `synth_unreachable` |
| `island` | the Island screen (with `screen=island`): `off` (default), `on`, `outdated` (the synth is older than the island), `taken` (another app holds the talk key), `micdenied`, `two` (with `account=two`: Maya listens to the keys), `nolink` (on, Mia offline: the keys are released). On `island.html` (the island window on a pretend desktop): `idle` (default), `hover`, `listening`, `screen`, `sending`, `accepted`, `thinking`, `working`, `driving`, `done`, `error`, `approval`, `text`, `offline`, `outdated`, `micdenied`, `asking`, `paused`, `two`, `shot`, and for speech on this computer `live` (the words of a hold), `livesent` (the text going), `offer` (the synth's offer), `offerdl`, `offerready`, `offerfail` (the model's download after "Turn on"), and for wake by name `wakeidle` (the dot), `wakepaused` (paused for a call), `woke` (a wake recording with its words), `wokename` (the name alone), and for the synth's voice reply `speaking` (Mia speaks her answer here: the face talks with the voice's level, a click stops it), `speakingwake` (the same with wake by name paused meanwhile); with `limits=claude` (Claude Max 34% / 61%), `limits=codex` (ChatGPT Plus 85%: the yellow dot), `limits=blocked` (refused: the red dot), `limits=stale` (40 minutes old, a third Opus row) or none (another runtime: nothing) added to any of them for the synth's subscription limits (the block on `hover`, the dot closed, e.g. `island=wakeidle&limits=codex`), and `hover=1` to open any scenario as if hovered (`island=done&hover=1&limits=stale`: the tallest); with `notch=0` (a Mac without a notch) and `os=windows` (the Windows desktop: the island floats above a taskbar at the bottom; add `taskbar=top` for a taskbar on the top edge); `pos=<vertical>-<horizontal>` (`top-left`, `top-center`, `top-right`, `middle-left`, `middle-right`, `bottom-left`, `bottom-center`, `bottom-right`) puts the island at the owner's spot on either system (a Dock is drawn on the Mac; `os=windows&pos=top-center` is the drawn notch flush with the top edge), and on the Island screen (`screen=island&island=on&pos=...`) the position picker shows it. The strip under the desktop plays a hold, a hold with its words, the offer, a wake by name (with a request, or the name alone), a call pausing it, a line of text, an answer, an answer aloud (a hold or a wake during it stops the voice), a request. Reply with voice on the Island screen: `voice=always|auto|off` (the owner's choice, default always), `island=voiceold` (Mia cannot answer with her voice yet: the note). Speech recognition on the Island screen: `stt` (on, the model here), `sttdl` (downloading), `sttspace` (the disk is too full), `sttold` (the synth does not take text yet), `sttintel` (not available), and with `os=windows` the same scenarios with Windows' single engine (Parakeet, no choice); wake by name: `wake` (on, listening), `wakedl` (its model downloading), `wakecall` (paused: the microphone is in use), `wakebt` (paused: the only microphone is in AirPods), `wakemic` (waiting for the microphone; with `os=windows` the Windows texts and sizes); `sttnospace=1` makes the mock download fail for lack of space |
| `faceset` | `invalid`: the synth refuses every face the editor saves |
| `lang` | `en`, `ru` |
| `theme` | `light`, `dark` |
| `os` | `macos` (default), `windows`, `linux`. Apps follows it: the Mac apps are listed only with `macos` |

Examples: `?screen=logins`, `?screen=logins&logins=offline&ext=closed&os=windows`,
`?screen=logins&safari=off`, `?screen=logins&ext=linked&safari=closed&lang=ru`,
`?screen=logins&safari=allsites`, `?screen=logins&safari=both&lang=ru`,
`?screen=logins&logins=empty&state=empty&ext=none&lang=ru`, `?approval=vault`,
`?screen=onboarding&step=3`, `?screen=status&state=paused`,
`?approval=cookies`, `?screen=computer&cua=noperm&lang=ru&theme=dark`,
`?screen=computer&preset=balanced&color=rose&speed=relaxed&idlehide=0&motion=always`,
`?screen=computer&cursor=off&lang=ru`, `?screen=computer&shot=1024&pip=on`,
`?screen=computer&os=windows&shot=2048`, `?screen=computer&os=linux&wayland=1`,
`?screen=computer&which=only`, `?screen=computer&which=except&picker=failed&lang=ru`,
`?screen=computer&which=all&os=windows`, `?screen=status&state=working&task=Mia:%20flights`,
`?screen=general&health=noperm`, `?screen=general&health=bounded&which=only&lang=ru`,
`?screen=general&health=nodaemon&os=windows`,
`?screen=apps&apps=noperm&messages=on`, `?screen=apps&os=windows`,
`?approval=message&lang=ru`, `?screen=general&developer=1`, `?approval=shell&lang=ru`.
`/approval.html` is the approval window's own page, `/island.html` the
island's (`?island=listening`, `?island=done&lang=ru`, `?island=idle&notch=0`).

In the mock pairing step: any 6 digits work; `000000` is "not valid",
`111111` is "expired", `222222` is "server unreachable", `444444` is a new
key for a synth that is linked already, `555555` is "too many tries",
`333333` (signed in) is a synth of another account. In General, the
shortcut `Ctrl/Cmd+Shift+Q` is "taken by another app". Examples:
`?account=signed_out`, `?account=two&screen=general&lang=ru`,
`?account=repair`, `?account=consent&connectfail=synth_outdated`.

## Keeping a Mac available

On Computer control, enable **Keep this Mac available** to prevent idle system
sleep while paired, computer control is enabled, and Connect is not paused.
Permitted computer tasks wake the display; it can sleep between tasks.
Pausing, disabling computer control, unpairing, or quitting releases the hold.
The setting is off by default and is preserved when switching presets.

The session must be unlocked for computer tasks. This setting does not unlock
macOS, wake a fully sleeping Mac, or override a closed lid. Automatic unlock is
deferred; the experimental native validation components are not bundled or
installed by the app.

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
at the end, and the one that stays in the build tree loses its extension,
so Safari on the release Mac does not show an extension from a build folder.
The disk image and the updater bundle are the release.

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

### Platform address

The owner's account lives on the platform: `https://platform.snth.ai`
unless `SNTH_PLATFORM_URL` is set when the Rust crate is compiled
(`state::PLATFORM_URL`). Sign-in opens `<platform>/connect/login`, the app
talks to `<platform>/api/app/v1/*` only. There is no override in the UI.

## How the shell is put together

- One `Core`, built in `setup`. The computer can be linked to several synths
  at once (every synth of the owner's account, plus any linked with a code):
  one link per synth, each with its own reconnect loop. A small supervisor
  in `main.rs` starts `Core::run_slot` for every link at start and again
  after each new pairing (`relink`; a second call for a running link
  returns at once). A loop ends when its link is removed. The policy, the
  journal, the approvals, computer control and the browser link are the
  computer's, shared by every link.
- Pairing, two ways (`connect-core/src/account.rs`, the core README has the
  whole account in "Pairing and the platform account"):
  - **Sign in with SNTH** (the first screen's main button). The webview
    calls `open_external("platform_login")`; Rust makes the state and the
    PKCE verifier, builds the address and opens the browser (the same call
    again, within 10 minutes, opens the same sign-in: "Open again"). The
    platform sends the browser to `snth://login?code=...&state=...`;
    `actions::handle_deep_link` hands it to `Core::login_complete`, which
    ignores (and journals) a state it is not waiting for, exchanges the
    code for the app's token (keychain, `account_token`) and reads the
    account's synths. The owner confirms on the consent screen
    (`account_connect`): every synth of the account is linked. With no
    synths yet, the screen takes a code from a bot.
  - **A code from the bot** (`/connect_computer`, the second button):
    `pair_begin` then `pair_confirm`, as before. While signed in, the code
    goes to the platform (`/api/app/v1/link-code`), which also ties the
    synth to the account when it is free.
  - Only the computer's FIRST link resets the settings, pauses the core and
    runs the onboarding steps (preset, folders, computer control). A second
    link changes no setting. A code for a synth that is linked already
    replaces that link's token (the way back from "needs relinking").
  - The account is read again (`account::spawn_sync`) at start, every 10
    minutes and when the main window gets focus (at most once a minute):
    a new synth of the account is linked with a notification ("This
    computer is connected to Maya"), unless the owner disconnected it here;
    a synth that left the account is unlinked; a refused app token (the
    owner removed this computer in the platform) signs the app out: the
    links made through the account go, links made with a code stay.
  - General has the account (email, Sign out with a confirmation) and the
    list of synths: the state of each link, "Disconnect" (that synth only:
    its token, its remembered answers and its browsers go; settings and the
    other links stay), and "Connect again" for a link its synth refused
    (an account link gets a fresh token; a code link asks for a new code).
    When the last link goes, the settings are reset and the next link runs
    the onboarding again.
- A synth that refuses this computer's token (HTTP 401) is not dialed
  again: its link reads `needs_repair` until it has a new token. An account
  link first asks the platform once for a fresh one by itself.
- Approvals: the core calls `Approver::ask`; the shell queues the request,
  opens the approval window, fires a notification, and resolves the future on
  the owner's click. No answer in 55 s, a closed window and a pause are all
  refusals.
- Pause is remembered (`shell.json`): a relaunch stays paused until the owner
  resumes.
- Developer mode (`shell.json` `developer`, General): shows the developer
  section, today the command line (`settings.shell`, the core's
  `shell.rs`). `update_settings` refuses to switch the command line on
  while developer mode is off, and `set_developer(false)` switches it off
  (which also stops a command that runs). The command line's own switch
  asks once more, with the full warning, before it turns on. Its approval
  window shows the command in a fixed-width face with its folder and shell;
  the only thing that can be remembered is "always this exact command"
  (listed under the section, with Revoke). Activity lists each command with
  its exit code, folder, time and output size, never the output.
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
  or stopped. When an app update brings a new pin, the copy of the version
  before keeps working until the new one is downloaded and verified (also
  when the download fails), a task that is running finishes on the old copy,
  the next one starts on the new copy, and the old copy is then removed. The
  pin is 0.34.1 (0.33.1 before). What 0.34 changed for the owner: the
  pointer's default movement is one arc with a soft glow instead of the old
  glide, so "Normal" speed is now a move of a fifth of a second to a second
  and a half; "Fast" and "Relaxed" are fixed lengths as before. Its six
  motion styles are not offered: the helper can only save a default for them
  in a settings file it shares with a driver the owner may have installed
  themselves, which this app never writes. Raising the pin = change `cua::PINNED_VERSION` and the hash
  table in `cua_install.rs` together.
- macOS permissions for computer control are this app's own (the helper runs
  embedded, as the app's child): the app checks and requests Accessibility and
  Screen Recording itself (`connect-core/src/tcc.rs`), and they show up under
  "SNTH Connect" in System Settings.
- What the synth is never allowed to trigger through computer control,
  whatever the settings say (`connect-core/src/cua_guard.rs`; the core README
  has the reasons): `start_recording` (it would keep pictures of the screen
  on disk and there is no switch for it yet), `install_ffmpeg` and
  `install_extension` (they install software), `check_for_update` (it writes
  into the home of a driver the owner may run themselves and answers with an
  installer command), `set_config` (the helper's own settings),
  `replay_trajectory` (stored actions the app cannot check). A screenshot or
  a download the helper writes to disk goes only into a folder shared for
  writing in Files. Each refusal tells the synth what to say to the owner,
  and shows in Activity as "never allowed". The self-test fails when a new
  helper version brings a tool of these kinds that nobody has classified.
- "How it looks" on the Computer control page (`settings.computer.look`,
  `connect-core/src/cua_look.rs`) is the helper's own look, applied as the
  start flags of the app's own helper daemon; the core README has the whole
  account ("How it looks"). What the shell adds: `update_settings` brings the
  values to the offered ones (`LookSettings::normalize`), a look does not
  make a preset "custom" (`matching_preset`), applying a preset keeps the
  look, and `UiState.look_support` says which controls this computer's helper
  follows (a fixed colour and the motion rows are not shown on a Wayland
  session, the preview window only on macOS). A look change does not move
  `settings_rev` and nothing of it is sent to the synth. It starts with the
  synth's next task: a session that is open keeps the helper it started with.
  The name under the pointer is the helper's, not a setting: it is the label
  the synth gives its task.
- "Which apps" on the Computer control page (`settings.computer.apps`,
  `connect-core/src/cua_apps.rs`): every app, every app except these, only
  these. The core README has the whole account ("Which apps"): "only these"
  is the helper's own bounded mode, "every app except these" is held by the
  app's pipe, and both lists are kept. What the shell adds: `computer_apps`
  reads the picker's list from the helper (a few seconds; the sheet says so)
  and what a new list of exceptions starts with (the password managers and
  security settings this computer has; the owner removes what they want),
  `update_settings` brings the lists to shape (`AppScope::normalize`), the
  choice does not make a preset "custom" and a preset keeps it. The page
  says what a limit costs: no picture of the whole screen, no click by
  screen position, and for "only these" no clipboard and none of the
  helper's page tools. A change starts with the synth's next task. The old
  "Blocked apps" list is carried over as exceptions, each word tagged "typed
  by hand".
- The task's name. The synth names its computer task (`Mia: flights`), the
  helper shows that under the synth's pointer, and Status shows it under
  "Right now" while a computer action is the latest thing (`core.computer_task`,
  cut by the core exactly as the helper cuts it; React renders it as text).
  Activity does not show it and the journal does not hold it.
- "Check the helper" under General, Diagnostics (`cua_health_check`,
  `connect-core/src/cua_health.rs`): the helper's own health report from the
  app's own daemon, one line per item in the owner's words (`general.health`
  in both languages: installed and the tested version, running, the two
  macOS permissions, reading windows and sending input, taking pictures, the
  synth's pointer, which apps), and "Copy details" with the helper's English
  after the core took out every path outside the app's own folder. It asks
  nothing of a driver the owner runs themselves and raises no system prompt.
- Pairing links: `snth://pair?code=XXXXXX` only brings the window up and hands
  the code to the UI, which asks who the code belongs to and shows the
  confirmation screen. The token is stored and the link opened only on the
  Connect click. On a computer that is linked already the same sheet adds
  the synth (or replaces the token of a synth that is linked already).
  `snth://login?...` is the platform's answer to the app's own sign-in and
  is acted on only with the state the app is waiting for.
- The island (test feature, off by default; `src/island/`, connect-core
  `island.rs`, the protocol in the synth's `docs/connect.md`, "Island").
  - Window: a third window `island` (island.html), transparent, no frame,
    created only while the island is on and a synth is linked. macOS: the
    window is turned into a non-activating NSPanel (`panel.rs`: its class
    switched to a run-time subclass of NSPanel, `NonactivatingPanel` style,
    level `NSMainMenuWindowLevel + 3`, `canJoinAllSpaces | stationary |
    ignoresCycle | fullScreenAuxiliary`, never hidden on deactivate, no
    shadow, `NSWindowSharingNone`), done with objc2 directly rather than
    the `tauri-nspanel` git dependency; every send is inside
    `exception::catch`. It never activates the app or takes the focus, and
    is not in the Dock, Cmd-Tab or Mission Control. Windows: an
    always-on-top tool window with `WS_EX_NOACTIVATE`, content-protected
    (`WDA_EXCLUDEFROMCAPTURE`). Linux: not shown.
  - Placement (`geometry.rs`, unit-tested): the screen with the menu bar.
    With a notch (`safeAreaInsets.top` > 0 and both `auxiliaryTop*Area`),
    the window is centred on the notch and the closed island is the notch
    plus two 84 pt ears (face left, status right), as tall as the notch;
    without one the same shape is drawn as if there were a 184 pt notch, as
    tall as the menu bar. Placed again on
    `NSApplicationDidChangeScreenParametersNotification`. Windows (no menu
    bar; the top is tabs and title bars): the island floats at the bottom
    centre of the primary monitor's work area (`Monitor::work_area`, the
    screen less the taskbar), a free-standing 84 x 28 pt pill with every
    corner rounded, 8 pt above the taskbar (12 pt above the screen's edge
    when no taskbar is below: auto-hidden, or on a side), `edge: bottom`;
    the open island grows upwards from the face (listening and the line of
    text only widen the pill). A taskbar on the top edge puts it 8 pt under
    the taskbar (`edge: top`, still a floating pill). Top centre chosen
    in Settings is the drawn notch instead (see Position). Pixels to points at
    the monitor's own scale; the poller asks for the placement again every
    3 s and moves the window only when it changed.
  - Position (Settings, "Where it sits"; `IslandSettings.position`,
    `{vertical: top|middle|bottom, horizontal: left|center|right}`, each
    `#[serde(default)]`, absent = this system's default; saved through
    `island_set` with `vertical` / `horizontal`, "default" for an axis):
    eight spots, the middle of the screen is not one (middle + center is
    refused by the command and read as the default). The default spot is
    exactly the placement above on each system (macOS top centre at the
    notch, Windows bottom centre with the top-taskbar rule). Windows top
    centre chosen is the macOS no-notch shape, as Windows had it before
    0.1.25: the drawn 184 pt notch with its two 84 pt ears, 24 pt
    (`MIN_BAR_H`) tall, flush with the monitor's top edge and centred on
    the monitor (flush under a taskbar on the top edge: the work area's
    top), `floating: false`, opening downwards. Every other
    spot, macOS included, is the floating 84 x 28 pill inside the usable
    area (Windows work area; macOS `NSScreen.visibleFrame`, the screen less
    the menu bar and a visible Dock, and never above the menu bar or the
    notch): top spots 8 pt under the bar / taskbar (12 pt off a bare
    edge), bottom spots 8 pt above the Dock / taskbar (12 pt off a bare
    edge), middle spots centred vertically; left / right spots 12 pt from
    that side. The window (480 x 360) is flush with the pill's side, so
    the open island is anchored to the pill's side edge and grows inwards
    (the row mirrored at the right: the face at the right end), and in the
    middle of a side it is centred on the pill with the face kept at the
    pill's height. A floating spot on macOS polls the placement every 3 s
    as Windows does (the Dock may move without a screen-parameters
    notification); the notch never polls. Full screen: the notch and the
    top spots come back with the menu bar, bottom and middle spots stay
    hidden. Changing the setting places the island again at once.
  - Clicks: the webview reports the shape and the face centre
    (`island_layout`); a poller in Rust reads the pointer (macOS
    `CGEventGetLocation`, Windows `GetCursorPos`) only while the island is
    shown, 30 Hz while it moves and 5 Hz at rest, switches
    `set_ignore_cursor_events` as it crosses the shape (hover opens the
    island), and sends `island:gaze` (a vector from the face, -1..1) only
    when the target moved. While the main synth drives the computer the
    eyes follow the synth's own pointer (`move_cursor` or a desktop click
    through the pipe, kept 10 s by the core).
  - Keys (`hold.rs`): defaults macOS Option+Space (talk), Option+Shift+Space
    (talk + screenshot), Control+Option+Space (text); Windows
    Ctrl+Alt+Space, Ctrl+Alt+Shift+Space, Ctrl+Alt+Enter (Alt+Space is the
    window menu, Ctrl+Space the input method, Win+ belongs to the shell).
    Registered only while the island is on, the main synth is linked and
    the app is not paused; otherwise other apps get them. A hold under
    300 ms sends nothing, two minutes ends a hold. The stop shortcut is
    registered next to them (it no longer unregisters everything).
  - Voice (`audio.rs`): cpal, mono, resampled to 16 kHz on the fly, WAV
    16-bit (the synth's Gemini speech-to-text takes it as it is; two minutes
    are 3.8 MB), a level for the face every 50 ms. macOS: recorded only
    after `AVCaptureDevice` says the microphone is allowed (asked once,
    `NSMicrophoneUsageDescription`, entitlement
    `com.apple.security.device.audio-input`, checked by the release
    script). Stream errors (cpal's error callback) are collected
    (`audio::StreamErrors`, the first one logged as it comes) and judged
    at the stop (`audio::verdict`): with a quarter of a second or more
    captured the recording goes on and the log says how many errors it
    had; only less than that after an error is `mic_failed`. cpal 0.18
    reports glitches the stream goes on after (WASAPI data discontinuity
    and CoreAudio overload as `Xrun`, a default-device change): up to
    0.1.28 any one of them, even hours earlier in the wake listener's
    stream, threw a finished hold away as "The microphone did not start".
    A device that does not stop in 5 s gives what it recorded. Every
    `mic_*` notice logs its path ("key hold, opening the microphone",
    "wake hold / key hold, stopping the recording at the release") in
    `stderr.log`; the wake listener's failed opens are never a notice,
    retried quietly with a growing wait (0.5 s up to 30 s,
    `wake::reopen_backoff`) and logged with why it was opening ("first
    open", "reopen after a wake", "after a stall", ...).
  - Screenshot (`shot.rs`): at the press, the app's own helper runtime
    (`get_desktop_state`, the same direct runtime that reads the list of
    apps, never the synth's pipe); macOS falls back to `screencapture -D`
    on the display under the pointer. Scaled to 2560 px, JPEG 80. The
    island is excluded from capture and made invisible for the moment.
  - Sending: `owner.message` on the main synth's control stream (a request
    line; waits only for `accepted`, 90 s; one retry with the same
    client_id after a dropped link). One message at a time. `island.event`
    (rpc) moves the face: accepted, thinking, working, done with the
    summary for 6 s (channel named), error.
  - The synth's voice reply (`src-tauri/src/island/speak.rs`, TZ
    `tz-voice-reply.md` in the synth's repo). The Island screen's "Reply
    with voice" (`IslandSettings.voice_reply`: `always` the default and
    unset, `auto` the synth decides, `off` text only; saved through
    `island_set`'s `voiceReply`). A voice message (`voice`,
    `screen_voice`, with the audio or the device transcript) carries
    `owner.message` `voice_reply` only to a synth whose hello_ack has
    `island_voice_reply` (connect-core drops it otherwise); a line of text
    never does. The synth answers with `island.event kind=speech`
    (`{message_id, audio: {mime, data (base64)}, text, duration_ms?}`,
    before or after `done`): mp3, WAV or Ogg Opus (aliases such as
    `audio/mp3`, `audio/x-wav`, `audio/opus` taken). Ogg Opus is common
    (Gemini voices, multi-chunk ElevenLabs), WAV only the synth's
    fallback. The synth sizes the audio to the line the app advertises
    (`capabilities.files.max_request_bytes`, 6 MB; the WS takes 8 MB), so
    connect-core takes up to that as base64 (`MAX_SPEECH_BYTES`, 4.5 MB
    decoded); about 90 s of speech is 1.5 MB. The synth waits for the
    rpc's answer (up to 30 s): connect-core hands the speech to the
    shell's sink (`Core::set_speech_sink`, `island::take_speech`, off the
    async runtime), which decides at once and answers `{"ok":true}` once
    playback has started (not when it ends), or an error `unavailable:
    <why>` when it will not play it (the owner chose text only since, not
    the answer to the last message sent from here, a hold running or being
    sent, another app records (the wake's `mic_busy`, a call), the audio
    does not decode). Under "synth decides" the synth then sends a voice
    note to the chat instead; otherwise only the summary shows, as always.
    Without a sink it is `CoreEvent::IslandSpeech` (the audio is never
    printed). Decoded in pure Rust (symphonia 0.5 for mp3 and WAV,
    `ogg` + `opus-decoder` for Opus, cut at 240 s). The pure-Rust Opus
    decoder runs only about 15x real time (90 s in 4 to 8 s on Apple
    silicon, release), so Opus streams: `speak::prepare` decodes the first
    half second before the rpc is answered (about 50 ms), the rest is
    decoded on `island-speech-decode` while it plays; mp3 decodes whole
    (90 s in about 60 ms). Resampled as it comes (linear) to the default
    output's rate and played through cpal (CoreAudio, WASAPI) on an
    `island-speaker` thread; a buffer not decoded yet plays silence in
    place, never a skip. One at a time: a new answer stops the one
    that plays. The face is in the `speaking` state (mouth or a pill's
    beans with the voice's level, sent as `island:level` every 50 ms, the
    meter bars beside the eye); the island does not open for it, the
    status says "Speaking" when it is open. Barge-in: any island key's
    press, a wake, a click on the island (`island_speech_stop`, not on a
    button) or the app's pause stops it at once; the key or the wake then
    goes on as usual. Wake by name is paused while it plays and 300 ms
    after (`speak::TAIL`, `Pause::Speaking`), and a wake decoded from audio
    caught just before the answer began is dropped, so the synth never
    wakes itself with its name; the price is that a wake cannot barge in
    over the voice (keys and the click can). Played even when the island
    is hidden by a full-screen app or was switched off after the message
    went (connect-core lets `kind=speech` through then; every other kind
    is refused): the sound does not depend on the window. One log line per
    answer: `island: speech: <bytes> bytes of <mime>, message <id>:
    played <s> s` / `stopped after <s> of <s> s: <why>` / `failed after
    ...: <why>` / `skipped: <why>`.
  - Faces: cached per synth in settings.json (`faces`), read with
    `face.get` when a synth that announces `island` links, saved with
    `face.set` from the editor on the Island screen, replaced live by
    `island.event kind=face`. A synth without a face shows the default
    preset of its id.
  - Subscription limits (TZ `tz-island-limits.md` in the synth's repo;
    connect-core `island/limits.rs`, the island's `limitsView.ts` and
    `LimitsBlock.tsx`). A synth whose main runtime is on a subscription
    (Claude SDK: Claude Max / Pro; Codex app-server: ChatGPT Plus / Pro)
    announces `island_limits` and pushes `island.event kind=limits` with
    no message_id: `{"kind":"limits","limits":{"provider":"claude"|"codex",
    "plan":"Claude Max","windows":[{"id":"five_hour","label_ru":"5ч",
    "label_en":"5h","used_pct":34,"resets_at":"<ISO>"}, ...],
    "blocked":false,"fetched_at":"<ISO>"}}`, or `"limits": null` (nothing
    to show: another runtime). The parse is defensive (unknown fields
    ignored, a window without a numeric `used_pct` dropped, percents
    clamped to 0..100, text clipped, at most 6 windows; a `limits` that is
    neither an object nor null is refused and the last snapshot stays).
    Kept per synth in memory only (`Shared::island_limits`), dropped when
    the link goes down (the synth sends its last snapshot again after the
    next hello_ack), shown only for the main synth while its link is up
    and it announced the feature (`Core::island_limits`, the view's
    `limits`); `CoreEvent::Limits` re-sends the island's view. Hovering:
    a block above the buttons (not over a request, a hold, the line of
    text or the speech card): the plan, a row per window (the synth's
    label in the island's language, a bar, the percent rounded, "resets
    18:20" / "до пн 03:00" in this computer's time: today the time only,
    within six days the weekday, later the date); bars white below 80%,
    yellow from 80%, red from 95% or on the fullest window when `blocked`
    ("limit reached" beside the plan). A window past its `resets_at`
    shows 0% "reset" (and ends a `blocked`); a snapshot older than 30 min
    says "updated N min ago" (from `fetched_at`, else the receipt). In a
    narrow card (the middle of a side, a spot near a screen edge) the
    reset word is dropped. Closed: a 7 px solid dot (yellow / red, a faint
    halo) left of the right ear's glyph and the wake dot, on the floating
    pill in the glyph slot (the slot widens by 11 px when both show; at
    the right of the screen the dot stays on the inner side). The island's
    window is 360 tall (was 300) so the tallest hover (a summary and three
    rows) fits. Stage: `&limits=claude|codex|blocked|stale|none` on
    `island.html`, `&hover=1` to open any scenario.
  - Island screen: on/off, who listens, the three keys (recorder,
    conflicts with each other and the stop shortcut, a system check,
    default), the microphone, speech recognition, the face editor and "Ask
    <name> to design their own face" (a line of text through
    `owner.message`).
  - On-device speech-to-text (`src-tauri/src/island/stt/`, TZ
    `tz-local-stt.md` in the synth's repo; off by default, the switch
    "Speech recognition: through the synth / on this computer" on the
    Island screen, `IslandSettings.stt` in settings.json). On Apple
    silicon and Windows x64 (build.rs sets the cfg names `device_stt`
    for both and `whisper_stt` for Apple silicon, in step with the target
    tables of Cargo.toml); Linux and the Intel half of a universal build
    do not build it and keep the audio path (the switch says it is not
    available there). Apple silicon has two engines: whisper.cpp through
    `whisper-rs` 0.15.1 with Metal (a target-specific dependency) and
    Parakeet (below); Windows has Parakeet alone (no whisper.cpp, so no
    cmake build on Windows). The whisper model is `ggml-large-v3-turbo-q5_0`
    plus the Silero VAD for whisper.cpp, pinned to a Hugging Face revision
    and a sha256 (`stt/model.rs`), 548 MB in `<data dir>/stt/`, fetched
    only on the owner's yes, resumed after a break, checked, deletable, with
    a disk-space check. One thread (`island-stt`) loads the model when a
    hold starts (or the switch goes on) and unloads it after 5 min idle.
    While the key is held (`stream.rs`) the audio is cut at pauses (Silero;
    the loudness detector when it is missing), each piece decoded once with
    the encoder window cut to the piece, and a preview of the rest goes to
    the island (`island:view` `transcript`). The release usually takes the
    last preview as it is. The text goes as `owner.message`
    `voice_transcript` with `stt: "device"` and no audio, only to a synth
    whose hello_ack has `island_device_stt`; a synth without it, a missing
    model, an engine error, text that is only a known whisper invention
    ("Продолжение следует...") or nothing read from audio with speech in it
    send the audio as before. The release waits at most 2.5 s
    (`stt::RELEASE_WAIT`); a model still loading, a failed load, a gone
    speech thread (started again by the next hold) or a voice detector
    that could not run also send the audio (`stt/release.rs`, a pure
    `decide`). Nothing is sent only when Silero went over the whole
    recording and neither detector heard speech (the island says so); a
    silent screenshot + voice hold sends the screenshot alone. Every
    release logs one line: `island: speech-to-text: sent transcript N ms
    after release`, `sent audio because <reason>`, `dropped: silence
    (vad)` or `sent the screenshot alone`. The synth's offer (an island
    card, `SttCard`, `stt_offer` pending/shown/accepted/declined, pending
    after the update for owners with the island on) shows only on an idle
    island; accepting it starts the download and shows its progress
    there.
  - Second on-device engine: NVIDIA Parakeet TDT 0.6B v3 (CC-BY-4.0,
    credited in the engine choice), `stt/parakeet.rs`. The owner picks it
    under "on this computer": "Whisper: better with English words" (the
    default) or "Parakeet: faster" (`IslandSettings.stt_engine`, serde
    default whisper). Model: istupakov's int8 ONNX export
    (`istupakov/parakeet-tdt-0.6b-v3-onnx` at 8f23f0c, encoder, decoder +
    joint, vocab.txt, each with a sha256) plus its own copy of the Silero
    VAD, 671 MB in `<data dir>/stt/parakeet/`, the same download rules
    (choosing it fetches it, never silently; until it is here the voice
    goes as audio). It runs on the CPU through the `ort` already there for
    the tone (the CoreML provider splits the encoder into hundreds of
    partitions and takes 4 GB). The NeMo log-mel features (with our own
    FFT and slaney filters; the repo's nemo128.onnx is not used) and TDT
    greedy decoding are a port of onnx-asr. One engine is loaded at a time
    (`Stt::switch` unloads the other); the stream cuts Parakeet's holds
    only at pauses of 0.6 s or more and at most every 25 s (`Decoder::cuts`).
    Each decode logs `island: stt: parakeet decoded X s in Y s`. On an M1
    Air: cold load 1.2 to 1.6 s, a 10.4 s clip in 0.33 to 0.45 s (whisper:
    1.0 s with the language known, 1.2 to 1.8 s without), footprint 1.1 to
    1.2 GB loaded (whisper 0.65 GB). The proof of the port
    (`stt_parakeet_matches_the_reference`, files in /tmp/parakeet-bench):
    on the float model the text matches onnx-asr on all 108 clips; the
    int8 model's text moves with float noise of 1e-5 (in Python too), its
    word error rate matches.
  - On Windows x64 Parakeet is the only engine (`stt::ENGINES`). The
    settings keep the owner's `stt_engine` as it is (whisper by default);
    `stt::effective` maps a choice the build lacks to Parakeet everywhere
    (the speech thread, the files, the download, the Settings DTO's
    `stt.engine` / `stt.choices`), so the same settings on a Mac would
    run whisper again. Settings shows no engine choice there, only
    "Engine: Parakeet (the one on Windows)" and its model (about 640 MiB).
    Its voice detector is the official Silero VAD v5.1.2 ONNX model
    (`stt/silero.rs`, `model::SILERO_ONNX`, snakers4/silero-vad at commit
    6478567, sha256-pinned, 2.3 MB, instead of whisper.cpp's ggml file) on
    the same `ort`: 512-sample windows with the LSTM state carried, the
    64-sample context given as the window's reflection (what whisper.cpp's
    graph computes), and whisper.cpp 1.7.6's
    `whisper_vad_segments_from_probs` ported line by line with the hold's
    parameters (`silero::HOLD`, which engine.rs gives whisper.cpp too).
    `stt_silero_onnx_matches_whisper_cpp` (ignored, files in /tmp) checks
    it on 245 clips: the segment logic is identical on whisper.cpp's own
    probabilities, the ONNX probabilities are within 0.013 of whisper.cpp's
    and the spans of every clip are the same. The encoder and the emotion
    model take half the logical CPUs, 2 to 4 (`parakeet::threads`; 4 on a
    Mac). `stt_e2e_parakeet_onnx` (ignored) runs the whole hold flow with
    the ONNX detector; the Windows CI runs it on clips spoken by the
    Windows speech synthesizer after downloading the pinned files. A
    microphone that Windows refuses (Privacy & security > Microphone off:
    WASAPI's E_ACCESSDENIED) is reported as `mic_denied` (the island says
    the microphone is not allowed and offers the privacy settings), not as
    a generic failure.
  - Wake by name (`src-tauri/src/island/wake/`, TZ `tz-wake-word.md` in
    the synth's repo; off by default, «Откликаться на имя» on the Island
    screen, `IslandSettings.wake` + `wake_phrase`). Apple silicon and
    Windows x64 (build.rs cfg `wake_whisper`; on Windows only with AVX2,
    FMA, F16C and BMI2, `wake::cpu_ok`; hidden elsewhere). Its own
    download in `<data dir>/wake/`: whisper tiny multilingual q5_1
    (`wake::TINY`, 32 MB, pinned like the hold's model) and a copy of the
    Silero file (`wake::VAD`: whisper.cpp's ggml file on a Mac,
    `silero_vad.onnx` on Windows), so deleting one model never breaks the
    other. One thread (`island-wake`) keeps the microphone open
    through `audio.rs` (the hold's own capture, trimmed to the last 4 s
    with `Tap::drop_front`) and feeds `listen.rs`: the loudness against
    the room's floor finds stretches; each is checked once (after 2 s, or
    when it ended): Silero says whether it is speech, whisper tiny on the
    CPU decodes its first 2 s with the name as the prompt (Russian with
    "Мия.", English with "Mia." only after a near miss), and `names.rs`
    matches the name by sound (Cyrillic and Latin to one key, exact for
    names of up to four letters) as the first or second word only. The
    name heard, the running recording is cut to the speech (0.5 s
    pre-roll) and becomes a hold of the voice key (`wake::start`, same
    `Hold`, same speech-to-text session); a second of silence (Silero)
    releases it through the hold's own path (`island::finish`, every
    fallback), the name alone waits 6 s for the request, a minute is the
    cap, Escape (a global shortcut only while a wake records) or a click on
    the island cancels, then 2 s are skipped. A transcript that is only
    the name sends nothing (`wake::request`); a sent transcript has the
    name stripped. It pauses by itself (`pause.rs`) while the app is
    paused, a hold key is down, the screen is locked
    (`CGSessionCopyCurrentDictionary`), or another process records from an
    input device (`system.rs`: CoreAudio process objects, macOS 14+; a
    process that says "running input" with no input device is ignored,
    as `avconferenced` does for days on macOS 26; before 14 a known call
    app in front). It never opens a Bluetooth microphone (`device.rs`: an
    always-open one switches AirPods to call audio): with a Bluetooth
    default input it listens on the built-in microphone (another wired one
    where there is none), and pauses (`bt_mic`) where there is no other;
    the choice is made again every second, so a changed default or a
    device that came or went reopens the microphone. Hold-to-talk keeps
    the default input. The view's `wake` (`off`, `waiting`, `listening`,
    `paused`, `woke`) puts a dim dot in the closed island's right ear (a
    ring while paused). Log lines: `island: wake: heard "<name>" (score),
    recording`, `sent after <why>`, `sent nothing: <why>`, `paused, the
    microphone is in use by <app>`; never the words. Measured on an M1
    Air: 0.3% of one core in a quiet room with the capture, about 2.2% of
    one core on non-stop speech without the name (a decode is about 40 ms
    on two threads), 110 MB peak with whisper tiny loaded.
  - Wake by name on Windows x64: the same listener, names and pause
    rules. The ears (`wake/ears.rs`) are whisper tiny on whisper.cpp
    (`whisper-rs` =0.15.1 without Metal, built by cmake + MSVC; bindgen
    uses the runner's LLVM) and the ONNX Silero (`stt/silero.rs`, the
    same spans as whisper.cpp's). ggml's CPU code is built for AVX2, never
    for the build machine: `.cargo/config.toml` sets
    `CMAKE_PROJECT_INCLUDE_BEFORE` to `.cargo/ggml-x64.cmake`, which on a
    Windows host turns `GGML_NATIVE` off and AVX/AVX2/BMI2 on (AVX-512
    off) and does nothing elsewhere; the CI's "whisper.cpp CPU floor"
    step checks ggml-cpu.lib for AVX2 and no AVX-512 code. Whisper tiny
    rather than Parakeet as the ears (`wake_real_refs` prints both on the
    same clips): Parakeet holds about 675 MB of working set on Windows
    (1.07 GB footprint on an M1) for as long as the mode is on, against
    about 92 MB for whisper tiny with the ONNX Silero (its commit is
    about 435 MB, mostly untouched), takes 330 to 370 ms a check on the
    CI runner against 150 to 200 ms (M1: 135 ms against 40 ms), and hears
    the name said alone as "Me." / "Nie." (so "Mia." alone and "Mia.",
    a pause, the request never wake). It is better than whisper tiny at
    English ("Maya" and "Me and my ..." do not wake it; tiny with the
    prompt "Мия." wakes on both, on a Mac as well for "Maya").
    On the runner (4 vCPU, MSVC-built ggml) the listener takes 8 to 13% of
    one core on non-stop speech without the name. `system.rs` on Windows: another
    app records = an entry of the microphone consent store
    (`HKCU\...\CapabilityAccessManager\ConsentStore\microphone`, Store
    apps and `NonPackaged` programs) with `LastUsedTimeStart` and no
    `LastUsedTimeStop`, not this program and, for a desktop program, only
    while a program of that file name runs (`system::consent`, tested on
    fixtures); locked = `WTSQuerySessionInformation(WTSSessionInfoEx)`
    says locked or the session is not the active one (the input desktop
    as the fallback when the flag is unknown); the inputs come from the
    MMDevice API with `PKEY_Device_EnumeratorName`, mapped to the Mac's
    transport codes by `device::windows_transport` (BTH* = Bluetooth,
    including the hands-free BTHHFENUM microphone; HDAUDIO/INTELAUDIO =
    built in; USB; ROOT/SW = virtual), so `device::choose` is shared. A
    microphone refused by Windows' privacy settings makes the mode wait
    (`need: mic`, its own text on Windows) and try again once a minute.
    `wake_real_refs` (ignored) runs the listener on clips with a
    `refs.tsv`; the Windows CI makes them with the speech synthesizer
    ("Mia, what time is it?", "Kira, ...", the name alone, the name then a
    pause, and clips that must not wake) and runs it on the downloaded
    files.
  - The tone of voice for a hold sent as text (`src-tauri/src/island/stt/tone/`;
    the switch "Detect tone of voice on this computer" under speech
    recognition, `IslandSettings.stt_tone`, on by default). Sent as
    `owner.message` `voice_tone` (one Russian line, at most 300 characters;
    the protocol allows 600) with the transcript, only to a synth whose
    hello_ack has `island_device_tone` (connect-core drops it otherwise).
    Two layers: `prosody.rs` (speech rate from the transcript's vowels over
    the speech time, pauses from the loudness detector's spans, level and
    its spread in dB, pitch by YIN at 8 kHz) compared with the owner's
    usual manner (`baseline.rs`, a running mean and spread per feature,
    `<data dir>/island-tone.json`, after 5 holds; before that only wide
    absolute bands), and `emotion.rs`, emotion2vec+ base (FunASR Model
    License 1.1: attribution and the model's name kept) as our ONNX export
    with int8 matrix products, 118 MB, run by ONNX Runtime 1.22 (`ort`
    2.0.0-rc.10, statically linked, Apple silicon and Windows x64; its
    build script fetches the prebuilt library from cdn.pyke.io once, cached
    in ~/Library/Caches/ort.pyke.io, %LOCALAPPDATA%\ort.pyke.io on
    Windows). The model file is pinned to a release
    asset of this repository (tag `model-emotion2vec-plus-base-1`) and a
    sha256, fetched after the whisper model while the switch is on, and
    deleted with it; a failed fetch is retried after 6 h (or when the owner
    asks), and until it is here the line has the prosody alone. It loads
    when a hold starts and unloads after 5 min idle. A thread per hold
    (`island-tone`) runs the model on 6 s windows while the key is held,
    and on the window in progress every 2 s; at the release it answers at
    once with what it has, then again with the rest run. The release waits
    for the complete answer at most 300 ms (`TONE_WAIT`) after the text is
    there, else takes the first one; the message is never held longer. On
    Russian (Dusha) the model is right about 46% of the time on four
    classes, but a sure "angry" or "happy" (probability 0.8 or more) is
    right about 75 to 88% of the time; "sad" is wrong 88% of the time, so
    only irritation and cheer are ever said. One log line per hold: `island:
    tone: N characters, emotion <top label> <p> (complete), waited N ms`.
- Webview permissions: `build.rs` lists every app command, which makes Tauri
  enforce `capabilities/*.json` per window. The main window gets the app's
  commands and the event listener; the approval window gets five commands.
  There is no shell plugin and no fs plugin; the folder picker is opened from
  Rust, so the webview never supplies a path; "open in browser" takes one of
  a few fixed targets, never a URL (`platform_login`: the address is built
  in Rust). Windows refuse to navigate anywhere but
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
  Both calls into SafariServices (`safari.rs`, `ffi`) ask
  `respondsToSelector:` first and run inside `objc2::exception::catch` and
  an autorelease pool. An Objective-C exception must never unwind into
  Rust: Rust cannot catch it, so the `catch_unwind` of the tokio task the
  question runs on aborts the whole app ("fatal runtime error: Rust cannot
  catch foreign exceptions"). That was the 0.1.7 crash on opening Logins;
  `safari::tests::calls::safari_throwing_does_not_take_the_app_down`
  replays it in a child process (a Safari that throws, an answer that is
  not a state, a state whose `isEnabled` throws, a class without the
  method). Any new Objective-C call in the app follows the same rule.
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

## Crash logs

`crashlog.rs`, installed first thing in `main`. The folder is the one
Tauri's `app_log_dir` names: `~/Library/Logs/ai.snth.connect/` (macOS),
`%LOCALAPPDATA%\ai.snth.connect\logs\` (Windows),
`~/.local/share/ai.snth.connect/logs/` (Linux, or under `$XDG_DATA_HOME`).

- `panic.log`: every Rust panic: time (UTC), app version, system, pid,
  message, file:line, the executable's load address and
  `Backtrace::force_capture()`. "Copy diagnostics" says how many panics are
  in it and when the last was; the file itself never leaves the computer.
- `stderr.log`: stderr, when it goes nowhere (started from Finder, the
  Start menu, a desktop file; from a terminal it stays on the terminal).
  connect-core's `logln!` lines, the default panic message with the
  thread's name, and the runtime's last line before an abort that no panic
  hook sees (a foreign exception, a panic while panicking, a panic in a
  function that cannot unwind). The macOS crash report has the stack of
  such an abort; this file has the reason.
- Each file is moved to `<name>.1` at start when it is over 2 MiB.

A release build is stripped (`strip = true`, `debug = false`), so a
backtrace is bare addresses (`0x1047e1234 - __mh_execute_header` on macOS).
They become names with the debug symbols of that exact build:

```bash
atos -arch arm64 -o snth-connect.dSYM/Contents/Resources/DWARF/snth-connect \
     -l <load address from panic.log or the crash report> <address> ...
```

No release keeps those symbols yet. PROPOSAL (not wired in): build the
release with `CARGO_PROFILE_RELEASE_DEBUG=line-tables-only` and
`CARGO_PROFILE_RELEASE_SPLIT_DEBUGINFO=packed` in the environment (no
change to `Cargo.toml`, so dev builds and CI stay as they are). The shipped
binary keeps the same size (strip still runs; checked on a small crate:
byte-identical size), and the symbols land next to it:

- macOS, `scripts/release-macos.sh`: export both variables for the
  `npm run tauri build` call, then
  `ditto -c -k --keepParent target/$TARGET/release/snth-connect.dSYM "$OUT/snth-connect-$VERSION-$TARGET.dSYM.zip"`
  (with `--universal`, one per architecture), check
  `dwarfdump --uuid` against the signed binary, and keep the zip with the
  release files (a private place is enough: the app is open source, so
  uploading it to the draft release is also fine).
- Windows, `connect-release.yml`: the same two variables give
  `target/<target>/release/snth_connect.pdb`; add it to an
  `actions/upload-artifact` step.
- Linux: `packed` gives a `.dwp` next to the binary; the same artifact step.

The cost is a longer and larger release build (the debug info is generated,
then split off), so it waits for the release manager.

## What the synth sees (observe)

The "What Mia sees" page: per linked synth, a switch for each source the
synth takes right now (connect.md "Observe: User Context reports";
`hello_ack` feature `observe`, the list `observe_resources`, the
descriptors from the `observe.resources` request at every link and every
10 minutes). Every source is off by default; switching one on shows the
descriptor's `privacy.consent` first. The logic (what counts as a call,
the categories, the exclusions, the place by network, what goes when) is
in `crates/connect-core/src/observe.rs` and `observe/sources.rs` with its
tests; the shell (`src-tauri/src/observe/`) only reads the system, every
2 s, and nothing while paused.

| Source | Resource | macOS | Windows | Permission |
|---|---|---|---|---|
| Calls | `device.call` | the processes that record (CoreAudio process objects, macOS 14+; before: the default input running while the app's own microphone is closed) + the running call apps | the microphone's consent store (`LastUsedTimeStop == 0`), this exe left out | none (the microphone is never opened) |
| The app in front | `device.activity` | `NSWorkspace.frontmostApplication` + `AXFocusedWindow`/`AXTitle` | `GetForegroundWindow`, `GetWindowTextW`, the process image | macOS: Accessibility for the titles (computer control's; never asked by this page), none on Windows |
| Place | `place.current` | CoreWLAN `ssid` | `WlanQueryInterface` current connection | macOS 14+: Location Services, asked when the place is switched on (`NSLocationUsageDescription`, `com.apple.security.personal-information.location`); Windows 11 24H2+: location for desktop apps |

The app's own microphone (a hold of the island, wake by name) never
counts as a call: the process list leaves this process out, and before
macOS 14 a running device while the app holds it (`audio::own_capture`,
plus a 3 s tail) is not a call. Password managers, the passwords page of
System Settings and private browser windows (by title) always go as
`private`; the owner adds own apps from the apps lately in front or
running now. The Wi-Fi network's name stays on the computer: only the
owner's name for the place goes, `null` on an unnamed network.

Mock mode: `?screen=observe&observe=on|call|private|notasked|denied|nowifi|unnamed|notitles|nofeature|nogrant|two`.
The probes on a real Mac print with
`cargo test -p snth-connect observe_probes_now -- --ignored --nocapture`.

## Not verified

- Observe, live: the switches against a real synth with UC on, the
  Location Services prompt in a signed build (the prompt, the answer, the
  network's name after it), a real call (Zoom, FaceTime, Meet in a
  browser, Telegram) seen as one, private-window markers in each browser
  (Chrome on macOS may not put "Incognito" in the window title), and
  everything on Windows (the consent store, the foreground window, the
  WLAN API and its location gate). Tested: the logic in the core (unit
  tests, also on the Windows CI machine), the macOS probes reading this
  Mac (`observe_probes_now`), the Windows probes compiled for
  x86_64-pc-windows-msvc.

- The island, live. Unit-tested: the notch arithmetic, the hold state
  machine, the WAV and resampler, the JPEG scaling, owner.message and its
  limits and errors, faces, island.event. Built (debug app) on macOS. Never
  done on a real machine: the panel over a full-screen app and on every
  Space, clicks through the transparent part, a real notch, a screen
  change, the microphone prompt and a real recording, the helper's
  `get_desktop_state` picture (and which display it takes), the island out
  of the picture, the keys on Windows, a whole round trip with a synth.
- The synth's voice reply, live. Tested: the setting and `voice_reply`
  (only with a voice message, only to a synth that announces it), the
  `speech` event's checks, decoding of an mp3, an Ogg Opus and a WAV
  fixture, the resampling, one answer at a time, the stop and the wake's
  pause with its tail, the shell answering within the rpc (unit tests);
  a real playback of the Opus fixture
  through CoreAudio on an Apple-silicon Mac (`speak_real_device`,
  ignored, run by hand; with `SPEAK_FILE=<3 s Ogg Opus>` the streamed
  path, also finished); the same test on the Windows CI machine, which
  may have no output device. Never done: WASAPI playback on a real
  Windows PC (shared mode at the device's mix format, a Bluetooth
  headset), a real answer from a synth end to end, barge-in by a real key
  over a real voice, a call detected at the moment an answer arrives.
- On-device speech-to-text, live. Tested: the cutting, stitching, silence,
  hallucination filter, language choice and offer rules (unit tests), and
  the real model through the speech thread on generated Russian and
  English clips fed in real time (`cargo test -p snth-connect --release --
  --ignored stt_real --nocapture --test-threads 1`, files in
  /tmp/whisper-test; `STT_ENGINE=parakeet` runs the same on Parakeet, its
  files in /tmp/parakeet-app/pk). Not done: a real microphone hold in the app, the
  download from Hugging Face end to end in the app, the offer on a real
  island, a Mac on macOS 12, a signed and notarized build with whisper.cpp
  in it.
- Sign-in through the platform, and several synths at once, end to end. The
  core is tested against a fake platform (PKCE, the state check, consent,
  the sync adding and dropping synths, a revoked app token, sign-out, a
  code through the account) and a local server that refuses tokens (no
  redial after a 401, one fresh token for an account link). Not done: a
  real `snth://login` coming back from a browser into the installed app on
  macOS, Windows and Linux; the real platform's `/connect/login` page and
  `/api/app/v1/*`; two real synths linked at the same time (two links, one
  driver of the computer, the refusal the second synth gets); the move of
  the one link of an app 0.1.12 to the new shape in the real keychain; the
  notification after a sync; the window-focus sync.
- "Which apps", with a synth at work. `connectd selftest cua` proved on the
  real pinned helper (macOS here, Windows in CI) that the bounded daemon
  lets the chosen app be observed and refuses another running app, the list
  of every window and the tools the manifest leaves out, and that the app's
  pipe refuses an excepted app by pid, by window alone and by a window under
  another app's pid. No tool that acts was run, so nobody has seen a click
  refused or allowed for real, and nobody has looked at the page in the app.
  The check by hand: Computer control, "Only these", add one app (say Notes),
  ask the synth to read a window of Notes (works) and then one of Safari (it
  says the owner does not allow that app); then "Every app except these"
  with Notes on the list, and ask for Notes again (refused) and Safari
  (works). Each choice starts with the synth's next task, five quiet minutes
  after the last one at the latest.
- The name under the pointer. The label rule was read in the helper's source
  and the labels were accepted by the real helper; nobody has seen `Mia:
  flights` drawn under a moving pointer or on Status. The check by hand: ask
  the synth to click something on this computer and look under its pointer
  and at Status, "Right now".
- "Check the helper", in the app. The self-test ran it against the real
  helper from a terminal (where macOS gives no permissions, so four lines
  fail as they should). In the installed app the lines for the permissions
  and "Started by SNTH Connect" should be green: General, Diagnostics, "Check
  the helper".

- "How it looks", on a screen. Everything about it was read back from the
  real pinned helper (`connectd selftest cua`, macOS here and Windows in CI):
  the theme, the motion values, the reduced motion choice and the hidden
  pointer as `get_agent_cursor_state` reports them, the seven themes as the
  helper's own `cursor-theme list` and `inspect` accept them, and two of them
  drawn to pictures by its `cursor-theme preview`. Nobody has LOOKED at a
  pointer moving in a chosen colour or at a chosen speed: no tool that acts
  was run. The preview window was only started once (the helper ran and
  answered with it); whether the window is useful was not judged. Linux was
  read in the helper's source only. The check by hand: Computer control, pick
  Rose and Relaxed, ask the synth to click something in a window; the pointer
  should be pink, take a second and a half per move, and show the task's name
  under it for a few seconds. Then switch "Show the pointer" off and ask
  again: no pointer.
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
