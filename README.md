# SNTH Companion

Local sidecar for [SNTH](https://snth.ai) synthetic companions. Runs on
the user's Mac, pairs 1:1 with a synth, and exposes sandboxed local
capabilities (bash, filesystem, Apple apps, Chrome) over a secure
WebSocket to the synth's backend.

## Status

Phase 1 (MVP). 22 tools across 5 waves. Menubar app. Chrome extension
relay. `snth-companion` is the first public repo of the SNTH stack.

## Architecture

```
Synth (Hetzner) ─tool call─> Companion (user's Mac)
                             │
                 ┌───────────┼──────────────────────────┐
                 │           │                          │
              Bash        FS R/W                    Apple apps
          (sandbox)      (scoped)         (Calendar/Notes/Reminders/
                                           Contacts/Messages/Shortcuts/
                                           Clipboard/Notify via AppleScript)
                 │
                 └─> Chrome  (extension OR --remote-debugging-port)
                     └─ page-agent DOM extractor + CDP actions
```

- **Transport:** persistent `wss://<synth>/api/companion/ws` opened by
  the companion (outbound, NAT-friendly). Survives laptop sleep via
  reconnect loop with exponential backoff.
- **Sandbox:** by default FS tools operate under `~/SNTH/<synth-slug>/`.
  User can grant additional folder roots from the menubar UI.
  Out-of-sandbox operations require a native macOS approval prompt
  (via osascript).
- **Pairing:** one companion = one synth. Set `SNTH_COMPANION_TOKEN`
  env var on the synth, paste same token + synth URL into companion's
  pair form at `http://127.0.0.1:<port>/`. A TG-mediated pair-code
  flow via the hub is planned.

## Tool catalogue

| Tool | Waveset | Danger | Description |
|------|---------|--------|-------------|
| `remote_bash`               | 1 | prompt        | `bash -c` in sandbox. Accepts `cmd`/`command` + `cwd`/`dir` aliases. |
| `remote_fs_read`            | 1 | prompt        | 2 MiB cap. Binary → base64. Tilde expansion. |
| `remote_fs_write`           | 1 | prompt        | Atomic (temp + rename). 4 MiB cap. |
| `remote_fs_list`            | 1 | safe          | Sorted entries. |
| `remote_shortcut`           | 1 | prompt        | `shortcuts run "<name>"`. Session-scoped approval cache. |
| `remote_calendar_list`      | 2 | safe          | Today or RFC3339 range. |
| `remote_calendar_create`    | 2 | prompt        | Title + start/end + optional calendar/location/notes/all_day. |
| `remote_calendar_search`    | 2 | safe          | ±30-day fuzzy match. |
| `remote_notes_list`         | 2 | safe          | Optional folder scope. |
| `remote_notes_create`       | 2 | prompt        | Plain text body; HTML-wrapped internally. |
| `remote_notes_read`         | 2 | prompt        | By id or title. HTML → plain. |
| `remote_clipboard_read`     | 2 | prompt        | `pbpaste`. 512 KiB cap. |
| `remote_clipboard_write`    | 2 | prompt        | `pbcopy`. |
| `remote_notify`             | 2 | safe          | `osascript display notification`. Optional sound. |
| `remote_reminders_list`     | 3 | safe          | All lists or scoped. Incomplete by default. |
| `remote_reminders_create`   | 3 | prompt        | Title + optional due + list. |
| `remote_reminders_complete` | 3 | prompt        | Mark done by id. |
| `remote_contacts_search`    | 3 | prompt        | Server-side `whose` filter. |
| `remote_messages_send`      | 3 | always-prompt | iMessage / SMS via Messages.app. |
| `remote_messages_recent`    | 3 | always-prompt | Direct `chat.db` SQLite query. Needs FDA. |
| `remote_browser`            | 4 | prompt        | Composite Chrome driver (navigate/snapshot/click/type/press/wait/screenshot/tabs/version/eval). |
| `remote_flight_search`      | 5 | safe          | Search IATA-to-IATA flights via the `letsfg` CLI on the paired Mac. 1-2 min per call; scrapes multiple OTAs in parallel. |

## Platforms

- macOS (ARM64 + Intel) — supported in MVP.
- Windows — planned for a future phase.

## Install

Until we ship a notarized build, build from source:

```bash
git clone https://github.com/snth-ai/snth-companion
cd snth-companion
go build -o snth-companion ./cmd/companion
./snth-companion
```

First run opens the browser UI at `http://127.0.0.1:<random>/` — paste
the synth URL + `SNTH_COMPANION_TOKEN` + synth ID to pair.

The companion shows a menubar icon (● SNTH) with status, open-UI shortcut,
and quit. Skipped in `--headless` mode.

## Chrome

Two paths to Chrome, both supported. The companion auto-detects which
is available; extension is preferred because it keeps the user's normal
profile.

### Option A: Chrome extension (preferred)

Install the unpacked extension from [`./extension/`](./extension/) —
see the [extension README](./extension/README.md) for the one-time
install dance (chrome://extensions → Developer mode → Load unpacked).
Then click the extension icon on any tab to attach. Chrome shows a
yellow "SNTH is debugging this browser" bar until detached.

### Option B: `--remote-debugging-port=9222`

Simpler but loses your normal profile unless you set `--user-data-dir`:

```bash
/Applications/Google\ Chrome.app/Contents/MacOS/Google\ Chrome \
  --remote-debugging-port=9222 \
  --user-data-dir=/tmp/snth-browser-profile &
```

Then any `remote_browser` action goes straight to Chrome's debug port.

## License

MIT — see [LICENSE](LICENSE).

## Acknowledgements

- The DOM extractor at `internal/browser/assets/dom_tree.js` is forked
  verbatim from [alibaba/page-agent](https://github.com/alibaba/page-agent),
  which itself is forked from [browser-use](https://github.com/browser-use/browser-use).
  MIT.
- Chrome extension relay pattern inspired by OpenClaw's browser bridge.
