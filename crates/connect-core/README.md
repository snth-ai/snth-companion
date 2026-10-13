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
| `apply_preset(Preset) -> rev` | careful / balanced / full |
| `mutate_settings(FnMut) -> rev` | arbitrary edit; bumps `settings_rev`, persists, pushes a `settings` message |
| `settings() -> Settings` | current settings |
| `add_root(path, write) -> rev` / `remove_root(path) -> rev` | shared folders |
| `Core::default_roots() -> Vec<Root>` | Desktop + Downloads, for onboarding |
| `grant_list()` / `revoke(cap, scope)` | live grants (`until`: 0 = always, -1 = this session, else a unix second) |
| `is_paired()` | a confirmed pairing exists (link up or not) |
| `active_streams()` | how many streams the synth has open right now (0 = nothing in flight; the host may restart for an update) |
| `set_metered(bool)` | the host reports a metered connection (the core cannot detect it) |
| `flush()` | write batched state (egress counters) before exit |
| `pause().await` / `resume()` | pause closes streams, refuses new ones, stops our Cua daemon, reports `paused:true`; resume is explicit |
| `cua_ensure().await -> InstallState` | make sure the app's pinned driver copy is installed: check it, download + verify + unpack + switch when missing or of another version. Idempotent, one run at a time, safe to retry |
| `cua_helper_state()` / `subscribe_cua_helper()` | where the install stands (`idle, checking, downloading{received,total}, installing, verifying, ready{version}, failed{code,detail}`), and a `watch` of it |
| `cua_stop_install()` | the app is quitting: stop a running install |
| `computer_permissions()` / `request_computer_permissions()` | macOS Accessibility + Screen Recording of the HOST app (what the embedded driver works under): read-only status, and the system prompts |
| `cua_install_argv()` / `cua_permissions_grant_argv()` / `cua_permissions_status()` | for a host that attaches to the user's own driver (`CuaAttach::User`): argv the host runs, and the read-only CLI status |
| `apps_views()` | the apps this computer offers (no Mac app on Windows), each with the owner's switch and its actions |
| `apps_permissions(app)` / `apps_request_permission(app, kind)` | macOS: where an app's Automation / Full Disk Access permission stands (read-only, never prompts), and the call that makes macOS ask. Empty elsewhere |
| `run_link(ws_url, shutdown).await` | the reconnect loop; spawn it. `ws_url` None = the paired synth url |
| `run_browser_link(endpoint, shutdown).await` | serve the SNTH Vault extension's helper on a per-user endpoint (see "The browser link"); returns an error at once when the endpoint cannot be bound |
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
  sites it may read changed).
- `subscribe_journal() -> broadcast::Receiver<Entry>` and
  `recent_journal() -> Vec<Entry>`: the action journal (time, capability,
  action, target, outcome, bytes, count). No content, no cookie values, no
  bodies; for the owner's apps a line is the app, the action, the outcome
  and how many items came back, never what a message, a contact or a note
  says.

## Snapshot

`Snapshot { link_state, paired, synth_id, synth_name, synth_url,
synth_version, idle_seconds, screen_locked, paused, metered, settings,
settings_rev, cua_version, cua_status, egress_month, egress_bytes }`.
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
minutes, app blocklist, tool denylist), files (enabled, roots with a write
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
  idle), and a per-`tools/call` gate in the mcp pipe (tool denylist, app
  blocklist, foreground delivery per `never/idle/ask/always`). Our own
  private Cua daemon on a private socket; the user's daemon is never touched.
  With `cua_private_root` the driver itself is the app's own copy too
  (`cua_install.rs`: pinned version, SHA-256 pinned in the source, private
  `CUA_DRIVER_RS_HOME`); the hello `computer.status` is `not_installed` until
  it is in place and `no_permissions` while macOS has not allowed the app.

- **apps**: everything below, in ONE function (`apps::handle`, the only way
  an `apps.<app>.<action>` rpc call reaches a backend). See "The owner's
  apps".

The synth has no message that changes settings or grants.

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
  only from the session that was asked. `clean_cookies` drops every cookie
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
| helper -> app | `{"type":"host.open","v":1,"origin":"chrome-extension://<id>/"}` | first line of a connection: who the browser started the helper for |
| helper -> ext | `{"type":"host","app":"not_running"\|"connected"}`, `{"type":"host","error":"malformed"\|"origin_not_allowed"}` | the helper's own words |
| ext -> app | `hello{v:1, browser, ext_version, install, paired, device_id?, sites[]}` | who this browser is; never a token |
| app -> ext | `hello_ack{v:1, app_version, linked, synth, known}` | whether the app has a synth, its name, whether this browser is linked here |
| app -> ext | `status{linked, synth, known}` | the same, when it changes |
| ext -> app | `pair.request{id}` | "link this browser" was clicked in the popup |
| app -> ext | `pair.code{id, code, synth}` | the synth's pairing code (`id` 0 = the owner clicked in the app) |
| app -> ext | `pair.linked{id, synth}` | the browser's existing pairing is with this synth: linked, no new code |
| app -> ext | `pair.refused{id, error}` | `not_linked`, `paused`, `denied`, `busy`, `synth_offline`, `synth_outdated`, `other_synth`, `failed` |
| ext -> app | `pair.done{id, ok, device_id?, error?}` | the hub took the code (or did not) |
| ext -> app | `state{paired?, sites?}` | the pairing or the readable sites changed |
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
`<browser id>-extension` for a read through the extension.

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
