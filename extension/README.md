# SNTH Companion Browser Relay — Chrome extension

Bridges your existing Chrome to the SNTH Companion sidecar over a
local WebSocket. Alternative to launching Chrome with
`--remote-debugging-port=9222` — use the extension if you want to
keep your normal Chrome profile (logins, bookmarks, extensions)
rather than spinning up a dedicated debug profile.

## Install (developer-mode, until we publish)

1. Make sure the SNTH Companion is running. It listens for extension
   connections on `ws://127.0.0.1:18792/extension` by default.
2. Open `chrome://extensions`.
3. Flip "Developer mode" on (top-right).
4. Click "Load unpacked" and pick this `extension/` directory.
5. Pin the extension to your toolbar.
6. Click the icon while viewing the tab you want your synth to
   control. Chrome will show a yellow warning bar —
   "SNTH is debugging this browser" — that's mandatory Chrome
   transparency; it stays up until you detach.
7. Click the icon again to detach.

## Permissions

- `debugger` — required to send CDP commands to the attached tab.
- `tabs` / `activeTab` — to know which tab is currently in focus.
- `storage` — to remember the relay port you configured.
- `http://127.0.0.1/*`, `http://localhost/*` — to open the WebSocket
  to the companion.

**Nothing is sent outside your machine.** The WebSocket is strictly
127.0.0.1; the extension has no remote hosts.

## How it works

```
Chrome tab (attached via chrome.debugger.attach)
   │
   ↓ CDP
background.js (service worker)
   │
   ↓ ws://127.0.0.1:18792/extension
Companion (relay server) ──► remote_browser tool ──► synth
```

The extension is a dumb pipe: the companion sends CDP commands, the
extension forwards them to Chrome's debugger API, and vice versa
for CDP events. All actual browser logic lives in the companion.

## License

MIT.
