# connect-core

The core of SNTH Connect: one outbound WebSocket from the owner's computer
to their cloud synth, giving the synth four capabilities under the owner's
policy. UI-free. The Tauri shell holds a single [`Core`] and drives it with
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
| `run_link(ws_url, shutdown).await` | the reconnect loop; spawn it. `ws_url` None = the paired synth url |
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
  `Settings{rev}`, `Paired{synth_id}`, `Unpaired`, `Paused{paused}`.
- `subscribe_journal() -> broadcast::Receiver<Entry>` and
  `recent_journal() -> Vec<Entry>`: the action journal (time, capability,
  action, target, outcome, bytes). No content, no cookie values, no bodies.

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

| | net | cookies | computer | files |
|---|---|---|---|---|
| careful  | ask | ask always | ask per task, foreground never | ask on write |
| balanced | all | ask new domain | auto, foreground idle | no questions |
| full     | all | auto | auto, foreground always | no questions |

`Settings` covers net (enabled, mode all/ask/list, domains, local-network
toggle, monthly cap, skip-on-metered), cookies (enabled, mode), computer
(enabled, mode ask/auto/away, foreground never/idle/ask/always, away-idle
minutes, app blocklist, tool denylist), files (enabled, roots with a write
flag, ask-on-write). `settings_rev` grows by one on every change; writes are
atomic.

## What the core enforces (so the UI need not)

- **net**: mode (all / ask-per-domain / list), per-domain grants, the
  private/loopback/link-local address policy (local-network toggle),
  monthly byte cap, skip-on-metered, a journal line per connection.
- **files**: list / stat / read / write (create / replace / append),
  read-only vs read-write roots, canonicalize-after-symlink confinement on
  read AND create, no delete. See the TOCTOU note in `files.rs`.
- **cookies**: per-domain grant (mode ask / ask-new / auto), read from a copy
  of the browser DB, values never logged or journaled.
- **computer**: open gate (catalog free, `ask` needs a grant, `away` needs
  idle), and a per-`tools/call` gate in the mcp pipe (tool denylist, app
  blocklist, foreground delivery per `never/idle/ask/always`). Our own
  private Cua daemon on a private socket; the user's daemon is never touched.
  With `cua_private_root` the driver itself is the app's own copy too
  (`cua_install.rs`: pinned version, SHA-256 pinned in the source, private
  `CUA_DRIVER_RS_HOME`); the hello `computer.status` is `not_installed` until
  it is in place and `no_permissions` while macOS has not allowed the app.

The synth has no message that changes settings or grants.
