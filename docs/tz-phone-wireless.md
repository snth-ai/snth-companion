# TZ: phone_control over Wi-Fi (no cable), launch by bundle id, paste

Status 2026-11-07. Owner: Sasha. Builds on phone_control P0 (branch feat/phone-control here; synth PR #547 / v0.6.284).
Goal in Sasha's words: "не хочу провод и хочу быстро".

## 0. Facts measured on the Mac mini (macOS 27.2, Xcode 27.2) with the test iPhone XR (iOS 18.7.10, UDID 00008020-0003650E0192002E, Developer Mode on, paired), cable UNPLUGGED, transport localNetwork

| command | time |
|---|---|
| `xcrun devicectl device capture screenshot --device <udid> --destination f.png` | 0.45-0.68 s (first 0.8), PNG 828x1792 |
| `xcrun devicectl device process launch --device <udid> com.apple.calculator` | 0.41 s |
| `printf 'Привет, тест' \| xcrun devicectl device pasteboard copy -d <udid>` | 0.59 s, `pasteboard paste` reads it back |
| `xcrun devicectl device info apps --device <udid> --include-all-apps --json-output f.json` | 0.39 s, 75 apps, `result.apps[].name` + `.bundleIdentifier` (Settings=com.apple.Preferences, Clock=com.apple.mobiletimer, App Store=com.apple.AppStore, Telegram=ph.telegra.Telegraph) |
| `xcrun devicectl list devices` | lists the XR "connected", and ALSO Sasha's personal iPhone 16 Pro "available (paired)" |

Today (cable eyes + Spotlight): frame 1.3-2.8 s, open_app 7.1-7.5 s every time, non-ASCII text refused.

## 1. Iron rules

- NEVER pick a device by itself. The phone is the one UDID the owner chose on the Phone page. Sasha's personal iPhone 16 Pro is visible in the same list and must never be touched. No fallback to "the only/first iPhone", no fallback from network eyes to the cable capture of another device or to a Continuity Camera.
- Hands stay the SNTH Phone Bridge board (Bluetooth, already wireless). Touch injection over CoreDevice/idevice is NOT in this TZ.
- Use only Apple's public `xcrun devicectl` (ships with Xcode). No private frameworks, no idevice, no new native deps. Xcode missing -> `no_screen: Xcode is not installed on this Mac (the phone over Wi-Fi needs it)`.
- Old synths keep working against the new app, and the new synth keeps working against the old app (abilities are advertised, absent = old behaviour).
- No em-dash characters anywhere (code, comments, docs, UI strings). No Co-Authored-By lines in commits.

## 2. Connect app (Rust, crates/connect-core/src/phone + app/src/screens/Phone.tsx)

### 2.1 Settings
`PhoneSettings.device_udid: String` (serde default empty). Empty = today's cable capture (HelperEyes), unchanged. Non-empty = network eyes for exactly that UDID.

### 2.2 Phone page
A "Phone over Wi-Fi" picker: lists physical iOS devices from `devicectl list devices --json-output -` (name, model, UDID, state), the owner picks one or "Cable (this Mac's capture)". Shows the chosen one's state. Strings in en.ts + ru.ts (Russian: natural, no em-dash). A short note: the phone must be paired with this Mac once by cable with Developer Mode on, then it works over the same Wi-Fi; keep it awake (Auto-Lock Never) and on a charger (a Qi pad works for the XR).

### 2.3 NetEyes (new, implements the existing `Eyes` trait in eyes.rs)
- `jpeg(after_ms, max_edge, quality)`: wait after_ms, one `devicectl device capture screenshot` into a private temp file (0600, deleted after read), decode PNG, scale to max_edge, encode JPEG. Report frame_ms.
- gray frames for diff/stability: from the same screenshot (decode + downscale like the helper's gray frames). Serialize calls (one devicectl at a time per device) and reuse a screenshot younger than ~150 ms instead of taking a new one.
- `status()`: `ok` when the chosen UDID is listed as connected/available (cache the list ~5 s), `device` = its name, width/height from the last frame (or 828x1792), plus `"source":"network"` (cable eyes report `"source":"cable"`).
- Per-call timeout (screenshot 5 s); a failure -> `no_screen: <devicectl's reason, short>`.

### 2.4 Stability with slow eyes
`wait_stable` assumes frames every 10-40 ms (quiet 200 ms = several looks alike). With ~0.5 s screenshots that means: stable = two consecutive screenshots alike (whatever their spacing), max wait unchanged. Make the after-tap / re-press logic correct for both eyes (the cable path must not get slower). Tests with a fake slow Eyes.

### 2.5 New act action `launch`
`{"action":"launch","app":"<display name or bundle id>"}`. Resolve via the cached app list (refresh on miss, cache ~10 min per device): exact bundle id, else case-insensitive display name, else a unique case-insensitive prefix. Not found -> `bad request: no app named "<x>" on the phone`. Then `devicectl device process launch --device <udid> <bundle>`, wait for the screen to settle, answer `{"ok":true,"changed":bool,"bundle_id":"..."}`. Goes through the same gate (pause, switch, grant, lease) and one journal line like every act. Only with network eyes (needs the UDID).

### 2.6 `type` through the phone's pasteboard
With network eyes: text that is non-ASCII OR longer than 40 chars goes `devicectl device pasteboard copy` (stdin, UTF-8) then the board presses cmd+v. Short ASCII stays typed by the board (unchanged). Journal keeps the length only, never the text. Never log the text. Cable eyes: unchanged (`unsupported:` for non-ASCII).

### 2.7 Status abilities
`phone_control.status` adds `"abilities":{"launch":bool,"paste":bool}` (both true only with network eyes and a reachable device). Document in the wire comment at the top of phone/mod.rs.

### 2.8 Self-test + docs
selftest_phone.rs: with network eyes, one screenshot and the app list (read-only). Update docs (connect.md "Phone" or wherever phone_control is documented in this repo).

## 3. Synth (Go, openpaw-internal: connect/phone_control.go, phonetask/)

- `PhoneStatus.Abilities{Launch, Paste bool}` (json "abilities"), `Screen.Source string`.
- phonetask runner reads status once per task (it already does a status call; reuse it).
- `open_app` with Launch: one `Act{Kind:"launch", App: name}` (add `App string \`json:"app,omitempty"\`` to Action), then the usual settle. A `bad request: no app named` answer is NOT fatal: the planner gets a short "no app named X on the phone" result and replans (same path as a target not found). Without Launch: today's Spotlight sequence unchanged.
- `type` (and open_app names): non-ASCII allowed when Paste is true; `errNonASCII` only when Paste is false.
- Prompt (phonetask/prompt.go): open_app line stays accurate for both paths (do not promise launch when it is absent; keep the prompt one static string per ability set so it caches: at most two variants).
- Tests: fake phone with and without abilities (launch path, launch miss -> replan, paste allowed/refused).
- Docs: docs/connect.md + docs/tool-config.md phone sections.
- Do NOT touch scripts/lib/release-stack.sh (the reviewer adds the attestation row).

## 4. Acceptance (the reviewer runs it on the mini after building the app)
calc 1234x56, Wi-Fi name, Telegram App Store rating, timer 3 min set + cancel, plus a Russian text goal ("напиши в Заметках 'привет'": create a note, then the reviewer deletes it) with the cable unplugged. Target: open_app under 1.5 s, per-step overhead (frame + act) under 2 s, Telegram rating task under 60 s.
