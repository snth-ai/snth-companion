# connect-core

The core of SNTH Connect: one outbound WebSocket from the owner's computer
to their cloud synth, giving the synth five capabilities under the owner's
policy (internet, cookies, computer control, files, and the owner's own
apps). UI-free. The Tauri shell holds a single [`Core`] and drives it with
commands; `connectd` (sibling crate) is the headless CLI over the same API.

Protocol v1 is implemented exactly from
`openpaw-connect-p0/docs/connect.md`. The synth side (Go) is the peer.

## Building a Core

```rust
use connect_core::{Core, CoreConfig, CuaAttach};
use std::sync::Arc;

let core = Core::new(CoreConfig {
    settings_path: Some(config_dir.join("settings.json")), // None = in-memory
    journal_path:  Some(data_dir.join("journal.jsonl")),   // None = ring + broadcast only
    approver:      my_approver,        // Arc<dyn Approver>  (the UI implements this)
    cookies:       my_cookie_source,   // Arc<dyn CookieSource>
    apps:          connect_core::apps::system_backend(Some(my_notifier)), // Arc<dyn apps::Backend>
    secret:        my_secret_store,    // Arc<dyn SecretStore>  (keyring in prod)
    hub:           Arc::new(connect_core::UreqHubClient),
    app_version:   env!("CARGO_PKG_VERSION").into(),
    cua_bin:       "cua-driver".into(),             // used only when cua_private_root is None
    cua_private_root: Some(data_dir.join("cua")),   // the app's own pinned driver copy
    cua_attach:    CuaAttach::Private, // our own embedded daemon (the product path)
});
```

Everything on `Core` is `Send + Sync`; clone the handle freely (it is an
`Arc` inside). No globals.

## Traits the host implements

- **`Approver`**: `fn ask(&self, ApprovalRequest) -> ApprovalFuture`. The core
  calls it; the UI shows a window and resolves the future. The core bounds
  every ask at 55 s itself (the synth's hard limit is 60 s), so an approver
  may take as long as it likes; a late answer still records a grant for the
  next attempt. `ApprovalRequest.reason` is the synth's text, already
  sanitized (control chars stripped, length-capped); show it as
  "the synth says: …". `ApprovalRequest.summary` is app-authored and safe.
- **`CookieSource`**: `fn get(&self, domain) -> Result<CookieRead, CookieError>`.
  Use `MultiReader` with the installed-browser DB paths and a `KeySource`
  (the macOS Keychain reader in prod; a fixture key in tests). Reads a COPY
  of the DB. Cookie values never hit a log or the journal.
- **`apps::Notifier`**: `fn show(&self, title, body) -> NotifyFuture`. Shows
  a notification as the host app (its own name and icon). Pass it to
  `apps::system_backend(Some(..))`; with `None` the system's own tools are
  used (`osascript`, a PowerShell toast, `notify-send`).
- **`apps::Backend`**: the owner's apps of one system. The product passes
  `apps::system_backend(..)`; tests and `connectd --apps mac|windows` pass
  `apps::stub::StubBackend`, an in-memory stand-in.
- **`SecretStore`**: token custody. `KeyringSecretStore` in prod,
  `FileSecretStore` headless, `MemSecretStore` for tests.
- **`HubClient`**: `UreqHubClient` claims a code against the hub; a fake is
  used in tests.

## Commands (all on `Core`)

| Command | What |
|---|---|
| `pair_begin(hub_url, code).await -> Pending` | claim a 6-digit code; holds the result in memory ONLY (nothing stored, no link) |
| `pair_confirm() -> Pending` | the owner confirmed the synth identity: store the token, allow the link |
| `pair_cancel()` | drop the pending pairing |
| `pending() -> Option<Pending>` | the claimed-not-confirmed pairing (synth_id, synth_url) |
| `unpair().await` | send `unpair`, await the ack (bounded), then wipe token + grants + state |
| `apply_preset(Preset) -> rev` | careful / balanced / full. Shared folders, the look and the owner's choice of apps are kept |
| `mutate_settings(FnMut) -> rev` | arbitrary edit; bumps `settings_rev`, persists, pushes a `settings` message. An edit of nothing but `computer.look` is saved without moving the rev and without a message |
| `settings() -> Settings` | current settings |
| `add_root(path, write) -> rev` / `remove_root(path) -> rev` | shared folders |
| `Core::default_roots() -> Vec<Root>` | Desktop + Downloads, for onboarding |
| `grant_list()` / `revoke(cap, scope)` | live grants (`until`: 0 = always, -1 = this session, else a unix second) |
| `is_paired()` | a confirmed pairing exists (link up or not) |
| `active_streams()` | how many streams the synth has open right now (0 = nothing in flight; the host may restart for an update) |
| `set_metered(bool)` | the host reports a metered connection (the core cannot detect it) |
| `flush()` | write batched state (egress counters) before exit |
| `pause().await` / `resume()` | pause closes streams, refuses new ones, stops our Cua daemon, reports `paused:true`; resume is explicit |
| `cua_ensure().await -> InstallState` | make sure the app's pinned driver copy is installed: check it, download + verify + unpack + switch when missing or of another version. Idempotent, one run at a time, safe to retry. After an update of the app with a new pin, the copy of the version before keeps running until the new one is in place (see "A new driver version") |
| `cua_helper_state()` / `subscribe_cua_helper()` | where the install stands (`idle, checking, downloading{received,total}, installing, verifying, ready{version}, failed{code,detail}`), and a `watch` of it |
| `cua_stop_install()` | the app is quitting: stop a running install |
| `look_support()` | which "how it looks" settings this computer's driver follows (`cua_look::Support`: the cursor switch, a fixed colour, the motion settings, the preview window), so a host hides the rest |
| `computer_apps().await` | the owner's picker for "which apps": every app the driver lists on this computer (`cua_apps::AppChoice`: the identity the driver matches on, the name, running or not, a password manager or security surface or not), read now by the app's own runtime of the driver. A few seconds on a computer with many apps |
| `computer_task()` | the label the synth gave the computer-control task it is running, in the bounded form the driver shows under the cursor; None when nothing runs. Also in the snapshot |
| `cua_health().await` | "check the helper": the driver's own `health_report` from OUR daemon plus what the app knows itself (`cua_health::Report`: items with a fixed id, a verdict and a code the host has words for, and a scrubbed text for "copy details") |
| `cua_driver_command()` / `cua_theme_dir()` | a command of our driver with the environment every driver process of ours gets, and the folder our driver reads cursor themes from |
| `computer_permissions()` / `request_computer_permissions()` | macOS Accessibility + Screen Recording of the HOST app (what the embedded driver works under): read-only status, and the system prompts |
| `cua_install_argv()` / `cua_permissions_grant_argv()` / `cua_permissions_status()` | for a host that attaches to the user's own driver (`CuaAttach::User`): argv the host runs, and the read-only CLI status |
| `apps_views()` | the apps this computer offers (no Mac app on Windows), each with the owner's switch and its actions |
| `apps_permissions(app)` / `apps_request_permission(app, kind)` | macOS: where an app's Automation / Full Disk Access permission stands (read-only, never prompts), and the call that makes macOS ask. Empty elsewhere |
| `run_link(ws_url, shutdown).await` | the reconnect loop; spawn it. `ws_url` None = the paired synth url |
| `run_browser_link(endpoint, shutdown).await` | serve the SNTH Vault extension's helper on a per-user endpoint (see "The browser link"); returns an error at once when the endpoint cannot be bound |
| `run_browser_door(endpoint, door, shutdown).await` | the same for one door: `Door::Chromium` is what `run_browser_link` serves; `Door::Safari` is the endpoint in the app group folder, for the handler of the Safari extension that ships inside the app |
| `browsers()` | the browsers whose extension has talked to the app: connected or not, linked or not, which sites each may hand a sign-in for |
| `browser_link(install).await` / `browser_unlink(install).await` / `browser_forget(install)` | link a connected browser to the synth's vault (the owner's click in the host is the confirmation), unlink one (its extension forgets the pairing and its sites, the synth removes the pairing at the vault), drop one that is gone |
| `vault_list().await` / `vault_forget(site).await` | which sites the synth's vault holds a login for (names and dates only), and deleting one |
| `set_cookies_extension_first(bool)` | which cookie source goes first: a linked browser's extension, or the browsers' files (default: the extension on Windows, the files elsewhere) |
| `snapshot().await -> Snapshot` | the state snapshot below |

## Child processes

Every child process (the driver, `tar`, `hostname`) is created through
`proc::command` / `proc::std_command`. On Windows they set `CREATE_NO_WINDOW`,
without which a windowed app flashes a console window for each child. A test
fails on a raw `Command::new` anywhere in this crate, in `connectd` or in the
app's shell.

Our driver daemon listens on a private Unix socket on macOS and Linux and on
a private named pipe on Windows (`cua.rs`, `PrivateDaemon`). It counts as
started only once that endpoint listens; a failed start is reported with the
daemon's last words and retried with a growing pause (5 s to 5 min).
`connectd selftest cua` runs the whole computer-control path on the machine
it is started on.

## Events

- `subscribe() -> broadcast::Receiver<CoreEvent>`: `Link{state,detail}`,
  `Settings{rev}`, `Paired{synth_id}`, `Unpaired`, `Paused{paused}`,
  `Browsers` (an extension connected, left, was linked or unlinked, or the
  sites it may read changed), `ComputerTask` (the name of the running
  computer-control task changed, or there is none any more).
- `subscribe_journal() -> broadcast::Receiver<Entry>` and
  `recent_journal() -> Vec<Entry>`: the action journal (time, capability,
  action, target, outcome, bytes, count). No content, no cookie values, no
  bodies; for the owner's apps a line is the app, the action, the outcome
  and how many items came back, never what a message, a contact or a note
  says.

## Snapshot

`Snapshot { link_state, paired, synth_id, synth_name, synth_url,
synth_version, idle_seconds, screen_locked, paused, metered, settings,
settings_rev, cua_version, cua_status, computer_task, egress_month,
egress_bytes }`.
`link_state` is `unpaired | connecting | linked | reconnecting | paused`.
While the link is down, `synth_id` / `synth_name` come from the stored
pairing. All fields are `Serialize`, so a Tauri command can return the
snapshot as JSON directly.

## What survives a restart

`settings.json` holds, next to the settings and `settings_rev`:

- the confirmed pairing without its token (`identity`: synth id, url and the
  last friendly name). The token stays in the `SecretStore`; a stored
  pairing whose token is gone counts as not paired;
- durable grants ("always" and until-time). Session grants are memory only;
- this month's egress count, so the monthly cap is not reset by a restart
  (written at most every 30 s or 4 MiB, and on `flush()`).

The journal ring is refilled from the tail of `journal.jsonl` on start.
`unpair` / `wipe_local` erase the pairing and every grant.

## Settings + presets

Fresh install = everything off until onboarding applies a preset. The three
presets (TZ section 5):

| | net | cookies | computer | files | apps |
|---|---|---|---|---|---|
| careful  | ask | ask always | ask per task, foreground never | ask on write | on, ask before every change (`ask`) |
| balanced | all | ask new domain | auto, foreground idle | no questions | on, ask before a change, may remember (`ask_new`) |
| full     | all | auto | auto, foreground always | no questions | on, no questions (`auto`) |

In every preset Messages is off until the owner switches it on, and a
message sent as the owner is asked about every time. A settings file from
before apps existed loads with apps off.

`Settings` covers net (enabled, mode all/ask/list, domains, local-network
toggle, monthly cap, skip-on-metered), cookies (enabled, mode), computer
(enabled, mode ask/auto/away, foreground never/idle/ask/always, away-idle
minutes, `apps`: which apps the synth may use, see "Which apps", tool
denylist, and `look`: how it looks on the screen, see "How it looks"), files (enabled, roots with a write
flag, ask-on-write), apps (enabled, mode ask/ask_new/auto, per-app switches
that differ from the default). `settings_rev` grows by one on every change;
writes are atomic.

## What the core enforces (so the UI need not)

- **net**: mode (all / ask-per-domain / list), per-domain grants, the
  private/loopback/link-local address policy (local-network toggle),
  monthly byte cap, skip-on-metered, a journal line per connection.
- **files**: list / stat / read / write (create / replace / append),
  read-only vs read-write roots, canonicalize-after-symlink confinement on
  read AND create, no delete. See the TOCTOU note in `files.rs`.
- **cookies**: per-domain grant (mode ask / ask-new / auto); one site at a
  time (a public suffix such as `com` is refused); two sources, a copy of the
  browser DB and the extension of a browser the owner linked; values never
  logged or journaled.
- **computer**: open gate (catalog free, `ask` needs a grant, `away` needs
  idle), and a per-`tools/call` gate in the mcp pipe (tool denylist, the
  owner's choice of apps: see "Which apps", foreground delivery per
  `never/idle/ask/always`, and the owner's look: see "How it looks"). Our own
  private Cua daemon on a private socket; the user's daemon is never touched.
  With `cua_private_root` the driver itself is the app's own copy too
  (`cua_install.rs`: pinned version, SHA-256 pinned in the source, private
  `CUA_DRIVER_RS_HOME`); the hello `computer.status` is `not_installed` until
  it is in place and `no_permissions` while macOS has not allowed the app.

- **apps**: everything below, in ONE function (`apps::handle`, the only way
  an `apps.<app>.<action>` rpc call reaches a backend). See "The owner's
  apps".

The synth has no message that changes settings or grants.

## Which apps (`cua_apps.rs`)

`settings.computer.apps` (`AppScope`): `policy` = `all` (every app),
`except` (every app except the ones on the `except` list) or `only` (only
the ones on the `only` list). Both lists are kept whatever the policy is.
An entry (`AppRef`) is the identity the driver matches on (the bundle id on
macOS, the path of the program on Windows and Linux) and a name for the
owner. The synth is told none of it: nothing is in `capabilities`. A change
does move `settings_rev`. A preset keeps the choice, like the shared
folders.

A settings file of an older version had `computer.app_blocklist`, words the
owner typed, each refusing a call that NAMED such an app. `Settings::migrate`
(run when the file is loaded) moves them to `apps.except` as `loose` entries
and sets the policy to `except`: a loose entry stands for every app whose
name, bundle id or path contains the word, and now also holds when the call
reaches the app by its window or process.

What Cua Driver 0.34.1 can do was read in its source at the pinned tag
(`session_manifest.rs`, `authorization.rs`, `tool.rs`) and checked on our
daemon (`connectd selftest cua`):

| The owner's choice | Who holds it |
|---|---|
| every app | nobody: the driver's standard mode, as before |
| only these | THE DRIVER. Our daemon is started with `--permission-mode bounded --capability-manifest <daemon dir>/apps.yaml --approve-capability-manifest`. The manifest (`Scope::manifest`) is deny by default: `allow.tools` = `MANIFEST_TOOLS`, `resources.apps` = one entry per chosen app (`bundle_id` or `executable`, `launch: true`, `windows: all`, `terminate: driver_launched`), `resources.files.write` = the folders shared for writing, `resources.desktop.display: false`, `expires_after: 24h`, `idle_timeout: 24h`. The driver then answers `bounded_resource_outside_manifest` for a window or process of any other app, and for everything that names no process: the unfiltered `list_windows`, input by screen position. A tool that is not listed is refused by name. The pipe decides the same once more, before the driver |
| every app except these | THE PIPE. The driver has no such mode (a manifest only allows). Every `tools/call` is resolved to an app and refused when the app is excluded |

The pipe's decision (`cua_apps::decide`, called by `mcp::filter_scoped`) is
one function for both restricted choices. Every tool of the driver has a
kind (`TOOLS`; a tool that is not there is refused while the apps are
limited, and fails the self-test):

- a tool that touches no app (sessions, sizes, the driver's own state): passes;
- a tool for a window (`get_window_state`, `click`, `type_text`, `zoom`,
  `invoke_menu`, `kill_app`, `page`, ...): every pid it names (`pid`,
  `target.pid`) and the owner of every window it names (`window_id`,
  `target.window_id`) must be an app the synth may use. A call that names
  neither, an `element_token` alone included, is refused, and so is a
  window or process that is no app the driver lists;
- the whole screen (`get_desktop_state`, `get_accessibility_tree`,
  `escalate_session`, `scope: "desktop"`, `target.kind: "desktop"`): refused.
  A picture of the whole screen shows every app, and a click by screen
  position lands wherever it lands; the driver has no hit test to ask;
- `launch_app`: the app it names (bundle id, path, or a name looked up in the
  driver's list) must be allowed. The driver does not check this even in
  bounded mode: the manifest's `launch` flag is read and never used;
- `list_apps` and `list_windows`: answered by the app itself, from its own
  reading of the driver's lists, without the apps the synth may not use (the
  bounded daemon refuses both outright);
- the page tools (`get_browser_state`, `browser_*`): in `only` refused (a
  manifest that allows them may not allow the window tools); in `except` a
  call that binds a browser window is checked by that window, and a call by
  `target_id` passes, because the driver mints a target id only for such a
  bind on the same connection;
- the clipboard: refused in `only` (the bounded driver refuses the system
  clipboard under any manifest), not limited in `except`.

Refusals start with `denied: ` and tell the synth what to say to the owner;
they never name an app. Each carries a class and a code in `error.data` (see
"How a refusal of the pipe reads"). The journal gets a fixed reason, never an app or a
window.

`Directory` is the app's own reading of the driver's lists: a second runtime
of the same driver binary (`cua-driver mcp --direct --embedded
--no-overlay`: no daemon, no socket) that only ever runs `list_apps` and
`list_windows`. `pid -> app` is trusted for a minute, `window -> pid` for
five seconds; an unknown pid or window makes it read again, at most every
two seconds. It is started on first need and stopped when the last
computer-control session ends.

A scope belongs to a daemon for its whole life. `Shared::cua_session` starts
a new daemon when the owner's choice differs from the one the running daemon
was started with, or when a bounded daemon is half a day old (`RENEW_AFTER`;
its permissions run out after a day and cannot be extended), but only while
no session holds it. The synth keeps its stream open between tasks, so the
pipe ends a stream itself when a call arrives after five quiet minutes
(the driver has ended every named session by then) on a daemon that is out
of date: the call is answered `unavailable: SNTH Connect is restarting its
helper ...; call again`, and the next call opens a new stream on a new
daemon. The same now carries a new look into effect.

What bounded mode costs, measured on the pinned driver: every call that
names a process makes the driver run its own `list_apps` first (to learn the
bundle id), which took 1 to 3 seconds on a Mac with 94 apps; no picture of
the whole screen; no clipboard tools; no page tools; `move_cursor` only with
a window target; `kill_app` never matches on macOS (the driver compares it
by program path, the manifest names a bundle id).

`cua_apps::choices` is the picker's list and `default_exclusions` what a
list of exceptions starts with: the apps on this computer whose name is in
`SENSITIVE_APPS` (password managers, Keychain Access, Passwords, System
Settings, Windows Security ...). The driver protects none of them by
itself: its "protected host" is its own app only.

## Check the helper (`cua_health.rs`)

`Core::cua_health` asks OUR daemon for the driver's `health_report` over a
connection of the app's own, and adds what the app knows: whether the
installed version is the pinned one, whether the synth's pointer can be
drawn (`get_agent_cursor_state`), and the owner's choice of apps (for
`only`: whether the daemon really refuses the list of every window). With
computer control off or the app paused the daemon is not started for it.
The driver's hints are written for `CuaDriver.app`, so the report carries
ids and codes for the host's own words, and the driver's English only in
`details`, after `scrub` (the app's data folder becomes `<app-data>`, a line
is cut where any other path begins).

`cua-driver doctor` and `cua-driver diagnose` are not used. On macOS
`doctor` unloads and deletes a LaunchAgent and a script of a driver the
owner may have installed for themselves; `diagnose` asks the daemon on the
default socket, which is never ours, and reads the system's permission
database.

## A new driver version (`cua_install.rs`)

The pin is `cua::PINNED_VERSION` (0.34.1) with the SHA-256 of the five
release archives in `cua_install.rs`. An app version with a new pin finds
`releases/<new>` missing and installs it next to the copy the version before
left:

- Until the new copy is in place, `Shared::cua_bin_now` is the old copy
  (`CuaInstaller::usable_bin`), so computer control keeps working on the
  driver the owner already had, also when the download fails (the supervisor
  in the shell tries again later). `cua_detect` reports the old version
  meanwhile, and "Check the helper" shows it as not the tested one.
- When the new copy is verified it is moved into place. A daemon that runs
  from the old copy is stopped at once only when no session holds it;
  otherwise the session goes on (`CuaSession::outdated` turns true, so the
  pipe ends the stream between tasks) and the next session starts a daemon
  from the new copy.
- The old release dir is removed when nothing runs from it any more
  (`hold` / `release_old`; on Windows a program that runs cannot be deleted
  anyway). The proxy of a session (`cua-driver mcp`) is always started from
  the binary its daemon runs.

`connectd selftest cua` plays this with the real driver.

## How it looks (`cua_look.rs`)

`settings.computer.look` (`LookSettings`): show the synth's cursor, its
colour, its speed, when it hides, whether it is animated, the largest
screenshot the synth gets, the driver's experimental preview window. Every
preset has the same look, the driver's own. It is the app's own business:
no part of it is in `capabilities`, a change of it does not move
`settings_rev`, and the synth cannot change it.

What Cua Driver 0.34.1 can do was read in its source at the pinned tag and
checked against the real binary (`connectd selftest cua`):

| The owner's choice | How the driver takes it |
|---|---|
| cursor on / off | `--no-overlay` on our daemon |
| colour | `--cursor-theme <id>`. The driver has NO colour setting: its built-in theme is recoloured by the driver, one of nine colours per session, picked from the session's name, and an installed theme keeps the colours it was made with. So each fixed colour is a theme (`ai.snth.connect.sky`, `.violet`, `.rose`, `.green`, `.amber`, `.red`, `.graphite`), and `auto` is the built-in one |
| speed | `--glide-ms` and `--dwell-ms`: fast 250 / 40, normal the driver's own (since 0.34 the timing of its default motion style, 0.2 to 1.4 s by distance; dwell 80), relaxed 1500 / 400. The synth's action waits for the cursor, so this is also how fast it works |
| motion style | not offered. 0.34 has six (`signature_arc`, the new default, `spring_settle`, `magnetic`, `comet_swoop`, `adaptive`, `classic`), three timings and five effects, chosen per session (`set_agent_cursor_motion`, `start_session.cursor_motion`) or by a saved default `cursor.motion.*`. There is no start flag, and the saved default is a key of the shared `~/.cua-driver/config.json`, which the app never writes. The synth may not choose either: `set_agent_cursor_motion` is refused and a `cursor_motion` is taken out of `start_session` |
| hide when idle | `--idle-hide-ms` (5 s, 15 s = the driver's default, 60 s = its maximum, 0 = never) |
| animation | `--cursor-reduced-motion on|off`. The driver's `auto` does not look at the system (only `on` changes anything), so "follow the system" is resolved by the app when the daemon starts (`presence::reduced_motion`) |
| largest screenshot | not a daemon setting we may use (below): `max_image_dimension` is put on each `get_window_state` / `get_desktop_state` call by the pipe, or brought down to the limit. 0 = no rule, the calls pass untouched |
| preview window | `--experimental-pip` (macOS only) |

The daemon's start flags are the driver's template for the cursor of EVERY
session, named or not, so the look holds without the synth doing or knowing
anything. They cannot be changed in a running daemon: a new look needs a new
one. `Shared::cua_session` (what an mcp stream takes, a `CuaSession`) starts
a new daemon when the look differs from the one the running daemon was
started with, but only while no other session holds it. A session that is
open keeps its daemon and its look; the change starts with the next task.

The synth and the look (`cua_look::gate`, called by `mcp::filter_line`):
`set_agent_cursor_enabled`, `set_agent_cursor_motion`,
`set_agent_cursor_theme` are refused with a sentence the
synth can read (`denied: the cursor's look on this computer is its owner's
choice ...`), and a `cursor_theme` and a `cursor_motion` are taken out of
`start_session`. While
the owner has the cursor switched off, `move_cursor` (which moves nothing
else) is answered by the app too, because the driver's own answer then
reads like a fault. Refused
and not rewritten into a no-op: an answer "done" for something that was not
done would be a lie the synth builds on.

Two folders of the driver are NOT moved by `CUA_DRIVER_RS_HOME`, and a driver
the owner installed for themselves uses them too:

- cursor themes (`~/Library/Application Support/Cua Driver/cursor-themes`,
  `%LOCALAPPDATA%\Cua Driver\cursor-themes`, `~/.local/share/cua-driver/...`).
  `DriverEnv` sets `CUA_DRIVER_CURSOR_THEME_DIR` to `<our driver home>/cursor-themes`
  for every driver process we start, and `install_themes` writes the seven
  files there (the same file under the same name that `cua-driver
  cursor-theme install` writes), again at every daemon start.
- the driver's settings file, `~/.cua-driver/config.json`
  (`max_image_dimension`, `experimental_pip`). Nothing redirects it. The app
  never writes it, which is why the screenshot limit rides on each call and
  the preview window is a start flag, and why the synth's `set_config` is
  never allowed (next section): its `experimental_pip` keys are written to
  that shared file from any session. One consequence stays: if the owner switched the preview
  window on for their own driver in that file, our daemon shows it too. The
  same holds for a cursor motion default (`cursor.motion.*`, since 0.34)
  the owner saved for their own driver: our daemon moves the cursor that
  way too.

The name under the cursor is not a setting. The driver draws the `session`
label the synth passes on its calls (for a few seconds and when the real
pointer is over the cursor), in a pill tinted with the session's colour.
Neither the text nor the tint can be set, by a flag, a tool or a theme, so
the app offers neither. The driver takes any text as a label (only an empty
one, `default` and one that starts with `__cua_runtime_` are not labels) and
shows it after `sanitize_session_label` (`cursor-overlay/src/session_badge.rs`):
control characters dropped, runs of white space made one space, at most 28
characters (27 and an ellipsis beyond), then cut again to the width of the
pill. Its font has Latin, Cyrillic and Greek letters. The synth is told to
start the label with its own name (`DesktopOwnerNote` on the synth side).
The app keeps the label of the running task in memory in that same bounded
form (`core::task_label`, `Core::computer_task`) for the status line, and
nowhere else: the journal names the tool, never the label.

The themes are compiled into the core (`assets/cursor-themes/*.cua-theme`,
14 KB each). `scripts/build-cursor-themes.py` builds them: it takes the
driver's own theme source (`assets/cursor-themes/cua.default.lottie`, MIT,
see `NOTICE.md` next to it), swaps its one palette colour, and compiles it
with the driver's own compiler (`cua-cursor-theme`, which our installer
keeps next to the driver). Shipped ready-made and not compiled on the
owner's computer: the result is the same on every system (the artifact has
no platform in it), a compile there could only fail, and the artifact
format belongs to the driver version, which the app pins anyway. When the
pin is raised, run the script again; the self-test fails if the new driver
does not accept the files. Only the built-in theme has the driver's gentle
"floating" movement; a fixed colour stands still between actions.

Per system: macOS, Windows and Linux X11 draw the cursor with the driver's
shared renderer and follow all of it. On a Wayland session the driver may
hand the cursor to a helper inside the desktop shell that takes only a
position and the session's colour, so there `Support` offers only the
on/off switch. The preview window exists on macOS only.

## What the synth is never allowed to trigger (`cua_guard.rs`)

The driver's catalog is built for an agent on its own machine. A few of its
tools do things on the owner's computer that the owner has no switch for in
this app. The mcp pipe refuses them whatever the settings say, with a
sentence that tells the synth what to say to the owner. The list is `NEVER`
in `cua_guard.rs`, one line of reason each:

| Tool | Why never |
|---|---|
| `start_recording` | keeps screenshots, window contents and optionally a video of the owner's screen in a folder on disk; the owner has no switch for it yet |
| `install_ffmpeg` | installs a program with the system's package manager |
| `install_extension` | downloads and installs a component of the driver |
| `check_for_update` | asks GitHub from the owner's computer, writes its answer into `~/.cua-driver` (the home of a driver the owner may run themselves) and returns an install command that would replace that driver |
| `set_config` | changes the driver's own settings; its `experimental_pip` keys go to `~/.cua-driver/config.json`, shared with the owner's own driver |
| `replay_trajectory` | runs stored actions inside the driver, where the app cannot check them one by one |

The cursor's look is refused separately (`set_agent_cursor_*`, see "How it
looks").

One rule for a file the driver writes (`WRITE_ARGS`: `screenshot_out_file`
of `get_window_state` and `get_desktop_state`, `debug_image_out` of `click`,
`destination_root` of `browser_download`): the place must be inside a folder
the owner shared for writing, checked after symlinks are resolved with the
files tool's own check, and the call goes on with the resolved path. With
files off, with nothing shared for writing there, with a path that is not
absolute, or when the owner is asked before every write (the driver cannot
ask), the call is refused.

Read and left alone, with the reason next to each in `REVIEWED_TOOLS` /
`REVIEWED_ARGS`: `stop_recording`, `get_recording_state`, `get_config`
(read-only or harmless), `check_permissions` (an embedded daemon never
raises a system prompt, whatever `prompt` says), `escalate_session`
(deprecated, one session's capture scope in memory). The legacy `page`
actions that change a page or a browser setting are off in the driver
itself unless the daemon is started with
`CUA_DRIVER_ENABLE_LEGACY_PAGE_MUTATIONS`, which the app never sets, and the
driver's screen history needs `--experimental-history`, which the app never
passes (it has no tool in the catalog either).

Not in these kinds and not changed: `browser_set_input_files` and the
`file_path` / `image_path` of `clipboard_write` READ a file from anywhere
(the same reach computer control has through any file dialog).

`connectd selftest cua` holds the lists against the catalog of the driver
that is really installed (`unclassified`): a tool whose name has install,
record, update, upgrade, config, permission, history, extension, download,
escalate, replay, telemetry, autostart, skill, grant or revoke in it, or an
argument whose name has file, path, dir, out, output, destination, save or
folder in it, that no list mentions fails the self-test. A new pin cannot
bring one in unseen.

## How a refusal of the pipe reads (`error.data`)

Every `tools/call` the mcp pipe refuses instead of handing it to the driver
is answered with a JSON-RPC error whose `message` is the sentence for the
synth (it starts with `denied: `, or `unavailable: ` when the list of apps
could not be read; unchanged, older synths read only that) and whose `data`
says what kind of refusal it is:

```json
{"jsonrpc":"2.0","id":7,"error":{"code":-32001,"message":"denied: ...","data":{"class":"owner_policy","code":"app_excluded"}}}
```

- `class` is `owner_policy` or `bad_argument`.
  - `owner_policy`: the owner's settings forbid this action or this target.
    The synth stops and tells the owner (the sentence says what to say). It
    never retries the call and never routes around it (another tool, another
    window, the whole screen).
  - `bad_argument`: the call's arguments are wrong in a way the synth can
    correct without going around any choice of the owner's: leave an
    argument out, pass `pid` and `window_id`, take a `bundle_id` from
    `list_apps`, go on without the call. The sentence says how. It may call
    again, corrected.
- Anything ambiguous is `owner_policy`, and so is the default: `jsonrpc_error`
  in `mcp.rs` cannot be called without a `cua_guard::RefusalKind`, and
  `RefusalKind::default()` is `owner_policy` with the code `refused`.
- `code` is stable, short, snake_case and unique per place a refusal is
  raised. The journal lines are unchanged.

| Code | Class | Raised in | When |
|---|---|---|---|
| `catalog_stream` | owner_policy | `mcp::filter_scoped` | a catalog stream calls a tool |
| `never_allowed` | owner_policy | `mcp::filter_scoped` (`cua_guard::NEVER`) | a tool the synth may never call |
| `tool_denylist` | owner_policy | `mcp::filter_scoped` | the tool is on the owner's denylist |
| `foreground_off` | owner_policy | `mcp::filter_scoped` | foreground delivery, `foreground = never` |
| `foreground_away_only` | owner_policy | `mcp::filter_scoped` | foreground delivery, `foreground = idle`, the owner is active |
| `foreground_not_allowed` | owner_policy | `mcp::filter_scoped` | foreground delivery, the owner was asked and said no |
| `app_not_chosen` | owner_policy | `cua_apps` | apps `only`: not one of the chosen apps |
| `app_excluded` | owner_policy | `cua_apps` | apps `except`: an app the owner excluded |
| `apps_list_unreadable` | owner_policy | `cua_apps` | the list of apps could not be read (`unavailable: `) |
| `tool_unplaced` | owner_policy | `cua_apps` | apps limited: a tool the app cannot place |
| `clipboard_not_chosen` | owner_policy | `cua_apps` | apps `only`: the clipboard |
| `page_tools_while_limited` | owner_policy | `cua_apps` | apps `only`: the page tools |
| `whole_screen_while_limited` | bad_argument | `cua_apps` | apps limited: the whole screen or a screen position; work window by window |
| `window_not_named` | bad_argument | `cua_apps` | apps limited: no `pid` / `window_id` (or `launch_app` naming no app) |
| `window_app_unknown` | bad_argument | `cua_apps` | apps limited: a window or process of no known app; take them from a fresh `list_windows` |
| `browser_unnamed` | bad_argument | `cua_apps` | `browser_prepare` would start Chrome or Edge while one is excluded; name the window |
| `app_unplaced` | bad_argument | `cua_apps` | `launch_app` names an app not in the helper's list; take `bundle_id` or the path from `list_apps` |
| `write_arg_not_path` | bad_argument | `cua_guard::confine_paths` | a write argument that is not a string |
| `write_arg_files_off` | bad_argument | `cua_guard::confine_paths` | a write argument while files are off |
| `write_arg_ask_on_write` | bad_argument | `cua_guard::confine_paths` | a write argument while the owner is asked before every write |
| `write_arg_outside_shared` | bad_argument | `cua_guard::confine_paths` | a write argument outside the folders shared for writing |
| `cursor_look_is_owners` | bad_argument | `cua_look::gate` | `set_agent_cursor_*` |
| `agent_cursor_off` | bad_argument | `cua_look::gate` | `move_cursor` while the owner switched the cursor off |

The write arguments are `bad_argument` because leaving them out complies
with the owner's settings: the picture comes back in the answer.

Not refusals, and without `data`: the answer `unavailable: SNTH Connect is
restarting its helper ...; call again` when a stream is closed between tasks
so new settings apply (nothing was decided about the call). Not JSON-RPC
either, and not classified: the stream-open refusals of the mux protocol
(`{"error":"denied: ..."}`, `proto.rs`), the files, cookies and device-apps
rpc errors.

## Logins: requests to the synth and the browser link

THE RULE: a password is for the synth's cloud browser only (the hub types
it; this app never holds one). A cookie is for requests made from this
computer only. Nothing in the core sends a cookie anywhere but back to the
synth's `cookies.get` on the device link, and nothing in it ever holds a
password.

**Requests to the synth** (`requests.rs`). Protocol v1 let only the synth
ask. The control stream now also carries the app's questions:
`{"type":"request","id","method","params"}` answered by
`{"type":"response","id","result"|"error"}`. Sent only to a synth whose
`hello_ack` had `"requests"` in `features`; otherwise the call fails at once
with `unavailable: the synth does not answer this yet (it needs an update)`.
Bounded at 30 s, at most 16 waiting, all failed when the link drops. Used
for `vault.pair_code`, `vault.list`, `vault.forget`, `vault.revoke_device`
and nothing else. The answer is the synth's word and is cut to shape before
use (`parse_vault_list`, `clean_code`).

**The browser link** (`browser.rs`). The extension reaches the app through
the helper of `connect-host`; each browser profile is one `Session`.

- A connection is served only when the helper's first line names one of our
  extension origins, and nothing is heard from it before a well-formed
  `hello` (browser, extension version, a random profile id, whether it holds
  a vault pairing, which sites it may read).
- Two doors (`connect_host::Door`), one room. The Chromium helper connects
  to the app's own endpoint and names a `chrome-extension://` origin; the
  native handler of the Safari extension that ships inside the app connects
  to a second endpoint, in the folder of the app group the two share, and
  names `safari-web-extension://ai.snth.connect.safari/`. Each door takes
  its own callers only (the right origin on the wrong door is closed
  unheard), and which browser a session is follows from the door: what came
  in on the Safari door is `safari` whatever its hello says, and nothing on
  the other door is. Everything after that is the same code.
- Pairing: `pair.request` from the extension makes the core ask the owner
  (`Approver`, capability `vault`, scope = the browser's name, never
  remembered). On yes it asks the synth for a code and sends
  `pair.code`; the extension redeems it at the hub and answers `pair.done`,
  and only then is the browser `linked`. An extension that already holds a
  pairing is linked only when the synth's vault lists its device
  (`other_synth` otherwise). `Core::browser_link` is the same without the
  question: the owner clicked in the app. One pairing at a time per browser;
  a `pair.done` nobody asked for links nothing.
- Cookies: `cookies.get{site, synth, asked}` goes only to a connected,
  LINKED browser, only for a registrable domain, and the answer is taken
  only from the session that was asked. With more than one linked browser,
  one that may read the site already is asked first, then the one that
  connected last; a browser that holds no cookie of the site is not the end
  (the next one is asked, inside the same time limit), a refusal or a
  silence is. `clean_cookies` drops every cookie
  that is not of that site, is malformed or is past 400. Unless the owner's
  answer for the site is a standing one (`ask_new` with a grant, or `auto`),
  the core tells the browser to give the site back right after the read
  (`site.revoke`); revoking a site in the host does the same; a browser that
  says hello with sites the owner no longer allows is told to drop them.
- The app tells an extension: who it is linked to, a pairing code after the
  owner's click, the name of a site. Never a cookie, a token or a setting.
- What survives a restart: `settings.json` `browsers` (profile id, browser,
  linked, the vault's device id, last seen). Unlinking the computer clears
  every `linked` flag and tells connected extensions to drop their sites.

The messages (JSON objects; the helper relays them unchanged and adds
nothing but its own `host` / `host.open`):

| Direction | Message | Meaning |
|---|---|---|
| helper -> app | `{"type":"host.open","v":1,"origin":"chrome-extension://<id>/"}` | first line of a connection: who the browser started the helper for. On the Safari door the extension's handler writes it, with `"origin":"safari-web-extension://ai.snth.connect.safari/"` |
| helper -> ext | `{"type":"host","app":"not_running"\|"connected"}`, `{"type":"host","error":"malformed"\|"origin_not_allowed"}` | the helper's own words |
| ext -> app | `hello{v:1, browser, ext_version, install, paired, device_id?, sites[], all_sites?}` | who this browser is; never a token. `all_sites`: the owner let the extension read every site in the browser's own settings |
| app -> ext | `hello_ack{v:1, app_version, linked, synth, known}` | whether the app has a synth, its name, whether this browser is linked here |
| app -> ext | `status{linked, synth, known}` | the same, when it changes |
| ext -> app | `pair.request{id}` | "link this browser" was clicked in the popup |
| app -> ext | `pair.code{id, code, synth}` | the synth's pairing code (`id` 0 = the owner clicked in the app) |
| app -> ext | `pair.linked{id, synth}` | the browser's existing pairing is with this synth: linked, no new code |
| app -> ext | `pair.refused{id, error}` | `not_linked`, `paused`, `denied`, `busy`, `synth_offline`, `synth_outdated`, `other_synth`, `failed` |
| ext -> app | `pair.done{id, ok, device_id?, error?}` | the hub took the code (or did not) |
| ext -> app | `state{paired?, sites?, all_sites?}` | the pairing or the readable sites changed |
| app -> ext | `cookies.get{id, site, synth, asked}` | hand over the cookies of this one site (`asked`: the owner just answered in the app) |
| ext -> app | `cookies.result{id, ok, cookies[]?, error?}` | cookie = `{name, value, domain, path, expires, secure, http_only, same_site}`; `error`: `denied`, `bad_site`, `failed` |
| app -> ext | `site.revoke{site}`, `sites.clear` | give back one site, or all of them |
| app -> ext | `unpair` | forget the vault pairing (the owner unlinked this browser in the app) |

Unknown types are ignored on both sides.

**`cookies.get`** (`rpc.rs`), after the owner's settings allowed the site:
where the files of the common browsers cannot be read (Chrome and Edge on
Windows) the linked browser's extension is asked first and the files second;
elsewhere the files first and the extension when they gave nothing. When the
extension goes first and that browser has not confirmed the site yet, the
approval request carries the detail `browser_step` = the browser's name, so
the host's window can say "step 1 of 2". The whole call stays inside 55 s:
a browser that is still waiting for the owner is answered with
`denied: no answer from the user (...)`, and the confirmation given later
serves the next attempt. `source` is the browser's name for a file read and
`<browser id>-extension` for a read through the extension. Safari's files
are never read (the cookie readers know Chromium and Firefox), so for a
sign-in that lives in Safari the linked Safari extension is the only source,
whichever goes first (`source` is then `safari-extension`).

## The owner's apps (`src/apps/`)

| File | What |
|---|---|
| `mod.rs` | `CATALOG` (every action and its risk class), the gate `handle`, the policy `decide`, the result bounds `bound`, the `Backend` trait |
| `request.rs` | `Request::parse`: every param checked and bounded before anyone is asked anything; the allowlist of file types `open.file` hands to the system |
| `mac.rs` | macOS: AppleScripts for Contacts, Calendar, Reminders, Notes and sending in Messages; `chat.db` for reading Messages; `shortcuts`, `pbcopy` / `pbpaste`, `open`; the permission checks |
| `win.rs` | Windows: the Win32 clipboard, `ShellExecuteW`, the Start menu (`Get-StartApps`) |
| `linux.rs` | Linux: `notify-send`, `wl-copy` / `xclip` / `xsel`, `xdg-open`, each only when installed |
| `notify.rs` | the `Notifier` seam and the system fallback |
| `time.rs` | times as the synth writes them (local or with an offset) |
| `stub.rs` | the in-memory stand-in |

Risk classes: READ (list, search, read), ACT (create, complete, send, run,
write, open) and ACT-light (`notify.show`, which never asks and is limited
to 6 a minute). What the gate does, in order: paused, the master switch,
what this system offers, the per-app switch, `Request::parse`, the
decision (`decide(mode, risk, request)`), the ask, the backend call with a
time limit, `bound`, one journal line.

- READ never asks: the per-app switch is the consent for reading.
- ACT follows `apps.mode`: `ask` asks every time and remembers nothing;
  `ask_new` asks and may remember per kind of change (scope
  `calendar.create`, `shortcuts.run:<name>`, `open.url:<site>`, ...);
  `auto` does not ask.
- `messages.send` is outside the mode. It asks in every mode, with the
  recipient and the whole text in the request (`ApprovalRequest.details`),
  and the only thing that can skip the ask is the owner's "always" for that
  exact recipient (scope `messages.send:<recipient>`, `Remember::AlwaysOnly`),
  not even that in `ask` mode. `permission.request` refuses the `apps`
  capability, so the synth cannot make such a grant.
- A run of a shortcut WITH input is never covered by a remembered answer.
- Results are bounded whatever a backend returns: list lengths, text
  lengths, no attachment data, and for `contacts.search` only real matches
  of a real query (at least 2 letters, or 4 digits of a number).
- `open.url` takes http and https only; `open.file` only a document
  (allowlist of types, no executable bit) inside a shared folder, checked
  after symlinks are resolved; `open.app` only a plain name.

The limits (defaults and caps) are the constants at the top of
`request.rs` and are listed in the protocol document (`connect.md`).

macOS scripts are constants. What the synth sent travels as arguments of
the script (`on run argv`), never as script text. `scripts/check-applescripts.sh`
compiles all of them against the real apps' dictionaries without running
them (inside a sandbox that forbids sending anything to an app).
