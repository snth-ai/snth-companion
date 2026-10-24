# Locked use: implementation status

Automatic unlock is deferred. The supported scope is keeping the Mac available,
waking its display for permitted tasks, and controlling an unlocked session.
The application **does not unlock macOS**. `computer.locked_use` is not exposed
in the UI and remains false in advertised capabilities. A matching bundle
or authorization-db entry cannot authorize a task.

## Implemented

- `computer.keep_awake`: explicit, default-off macOS setting. While paired,
  computer control is enabled and the application is not paused, IOKit prevents
  idle system sleep. The display can sleep between tasks. Changing the setting,
  unpairing, pausing or quitting releases the hold. Idle-sleep assertions expire after
  30 seconds unless renewed; the process owns and releases each assertion.
- Only permitted, forwarded computer tool calls request remote display activity.
  Catalog discovery does not wake the display. Display holds end after five
  minutes without a forwarded tool call (the driver's task-idle boundary),
  policy revocation or stream closure. They do not unlock a locked session.
- Computer streams recheck policy while open, including lock transitions and
  owner return in away mode. Revocation closes the proxy and stops our private
  CUA daemon. Previously remembered approvals no longer skip the lock gate.
- `AuthorizationPlugin.c`: native Authorization Services mechanism, built for
  arm64 and x86_64. It accepts only `allow`, requires an engine-context UID
  matching a non-root console user, and queries an explicitly signed privileged
  XPC broker. Missing context, missing service, wrong signature, malformed replies
  and timeouts deny this authorization branch. The wait is capped at 250 ms.
- A native ABI harness exercises plugin creation, invalid mechanisms, root/missing
  contexts, unavailable broker, deactivation and destruction without modifying
  the system authorization database.
- `Broker.c` and `Lease.c`: a privileged XPC service with separate signed-client
  endpoints for the application, guardian and Apple's authorization host. It
  binds pending grants to a connection, console UID, audit session, synth and
  task. UUID tokens stay in memory, hard deadlines are capped at 30 seconds,
  and both application and guardian must heartbeat within two seconds.
  Expiry/disconnect/cancellation enters revocation; a protected session cannot
  be replaced until the guardian acknowledges re-lock. Physical input latches
  the session until manual unlock. Continuous time includes sleep intervals.
  **Ordinary application tasks still cannot consume unlock grants.** A separate
  signed validation client can request one five-second grant after fresh native
  owner authentication, to test the real OS round trip under the guardian.
- `test_lease.c` exercises valid one-shot consumption, replay rejection, wrong
  UID/audit session/connection/token, concurrent tasks, hard expiry, missing
  heartbeats, disconnect, local input and the manual-unlock latch. These are
  state-machine tests, not a demonstration of unlocking the real Mac.
- `Guardian.m`: native AppKit guardian prototype, built as a separate app for
  both architectures. It authenticates the root broker before covering the
  desktop and only starts with a positively observed locked session. Black,
  non-activating panels cover the enumerated displays across Spaces. A session
  event tap suppresses input and requests revocation; a 100 ms monitor checks
  the tap, permissions, covers, lock state and broker heartbeat. Display/Space
  changes revoke and rebuild covers. A 30-second hard limit includes sleep. A separate watchdog child inherits the
  GUI/audit session and requests OS re-lock on guardian EOF, stale pulses or a
  one-second heartbeat timeout, independently of the guardian UI run loop.
  Cancellation, OS lock confirmation, and broker acknowledgment are all needed
  before uncovering. A pending heartbeat does not block cancellation.
  This is **not yet a proven protected-desktop boundary**; see limits below.
- `GuardianState.c` and `test_guardian.c`: exercise unknown lock state,
  permission/tap loss, cancellation before lock confirmation, missing/late
  replies, clock regression, sleep, the hard deadline and irreversible
  revocation. These tests also pass AddressSanitizer/UndefinedBehaviorSanitizer.

Build and test (no installation):

```sh
bash scripts/build-locked-use-plugin.sh
```

Outputs under `target/locked-use/`: `SNTHConnectAuthorizationPlugin.bundle`,
`ai.snth.connect.locked-use-broker`, its launchd plist,
`SNTHLockedUseGuardian.app`, `SNTHLockedUseValidation.app`, and native test binaries.
The bundle, service and guardian are built for arm64 and x86_64. The service exits
when launched without root; building/tests do not load it into launchd.
The default team requirement is `UNCONFIGURED`; it cannot authorize production
requests. A distribution build must supply `SNTH_TEAM_ID` and
`SNTH_CONNECT_SIGNING_IDENTITY`. The script does not notarize or install.

## Required before enabling unlock

1. Connect the application's approved MCP task to the broker's app endpoint.
   The native broker, lease engine and independent watchdog exist; the app
   bridge and its end-to-end validation do not. The broker must recover conservatively after a
   crash/restart, preserve the manual-unlock requirement across recovery, and
   confirm the lock before it can accept a replacement task. Until these are
   implemented and verified, keep `unlock_ready` false and actual consumption
   denied. No flag, user-default, or request parameter may bypass this gate.
2. Finish and validate the native protected desktop prototype on every connected display, including
   display hot-plug. Intercept physical input and relock before it can reach the
   desktop; stop automatic unlock until the user manually unlocks again. Handle
   controller/app crashes with an independent watchdog. An ordinary Tauri window
   and idle-time polling alone do not provide this protection. The current
   AppKit guardian revokes on hot-plug; its notifications do not guarantee
   atomic coverage of a newly attached display. OS event-tap timeout can disable
   interception. The independent watchdog requests re-lock on guardian failure;
   its process behavior is tested with fake OS calls, not yet on a live lock.
3. Implement and validate the actual loginwindow unlock request. Successfully
   evaluating a custom Authorization Services right does **not** by itself prove
   that the GUI session was unlocked. Confirm the lock state before forwarding
   the tool call. No boot-time login, FileVault unlock, or password storage.
4. A signed installer/uninstaller with a root-owned backup and rollback. Add the
   custom right as an alternative to `system.login.screensaver`, preserving its
   existing rule list and all third-party entries. Never replace the rule with
   `allow`, modify `system.login.console`, or erase another app's mechanism.
   Refuse unexpected rule shapes rather than guessing their semantics.
5. End-to-end macOS validation: ordinary password/Touch ID unlock, missing or
   crashed broker, expired/replayed grant, other user, physical input, multiple
   displays, sleep/wake, unpair/quit, and removal while locked.

Power management is separate from unlock: a process cannot receive commands
while the CPU is asleep. Closing the lid, manual sleep, shutdown and FileVault
preboot are outside the implemented wake behavior.

The XPC consumer expects request version `1`, operation
`consume-pending-unlock`, and the engine's `uid`. A successful broker reply must
be a dictionary with version `1`, the same `uid`, and `granted: true`. The broker
must consume a pending grant atomically and only after the protection lifecycle
is active. Ordinary app grants still receive `granted: false`. Only the distinct, locally
authenticated validation flow may consume a protected grant. No environment
variable or application setting enables production unlock.

### Broker protocol (not exposed to synths)

All requests require `version: 1` and an XPC reply handler. Connections and
notifications alone cannot issue or renew grants. Kernel-attested connection
UID/audit-session values are used; request-supplied identities are ignored.

| Endpoint | Allowed signing identity | Operations |
| --- | --- | --- |
| `ai.snth.connect.locked-use-broker.app` | `ai.snth.connect`, configured Developer ID team | `status`, `prepare-task`, `heartbeat`, `cancel-task` |
| `ai.snth.connect.locked-use-broker.guardian` | `ai.snth.connect.locked-use-guardian`, same team | `status`, `desktop-protected`, `heartbeat`, `cancel-task`, `local-input`, `screen-locked`, `manual-unlock` |
| `ai.snth.connect.locked-use-broker.validation` | `ai.snth.connect.locked-use-validation`, same team | `status`, `prepare-validation`, `validation-state`, `heartbeat`, `cancel-task` |
| `ai.snth.connect.locked-use-broker` | Apple's authorization host / SecurityAgent | `status`, `consume-pending-unlock` (protected validation grant only) |

`prepare-task` takes `task`, `synth`, `ttl_ms` and returns a generated `token`.
Subsequent task-scoped operations require that token and the originating
connection. `desktop-protected` establishes a separate guardian connection
for the same UID/audit session. During revocation the broker sends
`event: relock-required` to that guardian; it must keep the cover/input guard
active until the operating system confirms the lock. A missing guardian leaves
the broker in revocation, never in an implicitly authorized state.

### Guardian diagnostics and remaining integration

Read-only check, without showing windows, intercepting input, locking the Mac,
requesting TCC permissions, or contacting the broker:

```sh
target/locked-use/SNTHLockedUseGuardian.app/Contents/MacOS/SNTHLockedUseGuardian --check
```

`Session.m` now interprets the normal Quartz unlocked shape: the lock flag
is absent, but the dictionary belongs to the current UID, is on-console and
has completed login. Missing/malformed identity fields, another user, and
ambiguous secure-input state still return unknown. This corrected the earlier
false `unknown` result on this Mac. Native fixture tests cover these cases.

The private `SACLockScreenImmediate` symbol exists here; its presence alone does
not verify the OS behavior. `snth_session_request_unlock` locates Apple's signed
loginwindow and submits `AXConfirm` on its secure password control. Validation
now sets a random non-secret marker first, because confirming the empty field
did not invoke authorization in the live test. It never reads the field value
or obtains the owner's password. If the plug-in declines, the normal system
branch may reject the marker as an incorrect credential; no retry follows. It only reports submission, not successful unlock.
The validator must separately observe broker consumption, OS unlock and re-lock.
`relock_verified`, `unlock_ready`, and `capture_exclusion_integrated` remain false
until the real test and driver integration are complete.

The future app launcher passes a 36-byte UUID through an inherited stdin pipe
to `--protect`; tokens are never passed in argv or environment variables.
The local validator launches it; the product app bridge is not wired yet. A signed broker and a matching prepared task are
required. No successful live protection round trip has been recorded yet. A transport failure after
enrollment retains the cover until the broker acknowledges revocation and the
OS is positively locked; crash recovery is unfinished, so **do not install or
launch this prototype as a production guardian**.

The current event tap treats synthetic input as a reason to stop too. PID and
user-data fields in CGEvents do not authenticate the driver; there is no bypass
based on those fields. A trusted driver input route must be designed and tested
before admitting CUA actions. Input after final lock acknowledgment and the
manual-unlock latch also need an end-to-end race test.

`NSWindowSharingNone` is only advisory; it is not a ScreenCaptureKit exclusion.
The local recorded 0.34.1 CUA tool schema has no exclusion argument on
`get_desktop_state`. `get_window_state` supports window-specific capture, but
`display_only` explicitly returns a preview that cannot ground input actions.
The locally installed command is 0.33.1, so it cannot validate the pinned 0.34.1
runtime. SDK `SCContentFilter` supports excluding windows, which requires
integration in the driver's capture pipeline. No working hidden-desktop
capture or CUA action through the cover is claimed.

### Local owner-authenticated validation (not a product release)

`Validation.m` is signed under a separate identifier and uses its own broker
endpoint. Each attempt first checks the installed broker, then calls
LocalAuthentication with reuse disabled, then requests a system lock. After
observing lock, it obtains a five-second grant and launches the guardian. Only
when the protected desktop is acknowledged does it submit the system unlock
request. It cancels immediately after independently observing broker consumption
and OS unlock; success requires observing OS re-lock and grant completion too.
It exits on failure and leaves re-lock to the guardian/watchdog. It never reads,
stores or types a password. On failure, manual unlock may be necessary.

Build/sign and review the installation plan:

```sh
SNTH_TEAM_ID=CZAP38JFAE \
SNTH_CONNECT_SIGNING_IDENTITY='Developer ID Application: Aleksandr Sabri (CZAP38JFAE)' \
bash scripts/build-locked-use-plugin.sh
python3 scripts/locked-use-validation-install.py --plan
```

The development installer accepts `--install` or `--uninstall` with administrator
privileges. It verifies the exact signed identifiers/team in root-owned staging,
backs up the screensaver rule, preserves Codex and all existing alternatives,
refuses unknown rule shapes or pre-existing SNTH installations, and rolls back
on error. Removal deletes only SNTH's alternative, preserving subsequent
third-party changes. This script is a local validation tool, not the final
signed/notarized product installer. Five fixture tests cover install/remove,
third-party preservation, signature failure, registration failure and changed
rights; they never modify the real authorization database.

After installation, the explicit test command is:

```sh
'/Library/Application Support/SNTH Connect/Locked Use/SNTHLockedUseValidation.app/Contents/MacOS/SNTHLockedUseValidation' --roundtrip
```

The owner sees a native authentication prompt describing the lock/unlock/re-lock
cycle. Running the app without `--roundtrip` does nothing. Both validator and
guardian need macOS Accessibility access in the actual signed runtime; check
this before testing. The test finishes with the Mac locked. Production settings
and advertised capabilities remain disabled regardless of validation results.

### Live validation notes (2026-10-24)

- The signed validation components were installed with native administrator
  authentication. The root launchd broker is running; the original Codex and
  `use-login-window-ui` alternatives remain present.
- The first live attempt authenticated the owner, requested an OS lock, observed
  the lock and enrolled the guardian. The system authorization request did not
  find a ready password control; the grant was cancelled and the test observed
  the Mac locked. The owner also manually unlocked during this test, so this is
  not evidence for or against a successful unattended round trip.
- During manual unlock, the plug-in reached the broker from Apple's
  `com.apple.SecurityAgentHelper.arm64`. Its signature was rejected by our
  incomplete host list. The broker now also admits exactly the signed Apple
  SecurityAgentHelper arm64/x86_64 identities, verified against this Mac's
  installed system binaries. The Apple signing anchor is still mandatory.
- The validator now waits briefly for loginwindow UI settlement and searches
  AXWindows as well as AXChildren. A missing control is retried within the
  original five-second grant; it never retries a submitted authorization.
- The second attempt timed out waiting for owner authentication and did not
  request a lock. The validator now brings that prompt forward and distinguishes
  authentication timeout from an actual lock/unlock timeout.
- The installer comparison now includes the identifier and requirement that
  `security authorizationdb write` adds, so removal recognizes the installed
  right without accepting unrelated changes.

A successful automatic unlock/re-lock round trip is still pending.

The updated components were subsequently installed and their hashes matched the
signed build. A further hands-off attempt observed lock and protected desktop,
then successfully submitted AXConfirm, but did not observe unlock before grant
expiry. The broker log contained only validator and guardian connections during
that attempt: no authorization-plugin connection was seen. This supports testing
a nonempty, non-secret submission marker; it does not yet prove that the marker
will unlock the session. After the owner manually unlocked, the signed target
validator was launched at 13:52 local time. It exited at 13:54 with
`owner_authentication_timed_out; no_lock_requested`. LocalAuthentication logs
recorded `DidAppear` and `GotFocus`, without a successful authentication before
the timeout. A read-only check then confirmed the session remained unlocked.
The marker was never submitted, so the marker hypothesis is still untested.
No password-marker retries have occurred.

At 13:56 local time a fresh owner-authenticated attempt reached system lock,
protected desktop and a single marker/AXConfirm submission. It ended with
`roundtrip_failed_closed; system_is_locked`. Broker logs again showed validator
and guardian connections but no authorization-plugin connection during the
attempt. Both processes exited. This is not a successful automatic unlock.
The owner then chose to defer unlock and retain wake/availability support.
The validation installer was subsequently run with `--uninstall` successfully.
Read-only verification confirmed the broker is no longer registered with launchd
and the screensaver rule contains only the previous Codex and
`use-login-window-ui` alternatives. No experimental unlock component is required
for wake/availability support.
