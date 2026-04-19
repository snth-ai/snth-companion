// SNTH Companion Browser Relay — service worker.
//
// Flow:
//   1. User clicks extension icon on a tab.
//   2. We call chrome.debugger.attach({tabId}, "1.3") — Chrome shows
//      the yellow "SNTH is debugging this tab" warning bar, which is
//      our mandatory transparency.
//   3. Open WebSocket to ws://127.0.0.1:<port>/extension.
//   4. For every frame received from the companion (method+params),
//      we forward to the tab via chrome.debugger.sendCommand and
//      reply back on the same WS.
//   5. For every CDP event Chrome emits (chrome.debugger.onEvent),
//      we push it out over the WS so the companion sees live state.
//   6. Second click detaches + closes WS.
//
// We re-attach automatically on service-worker wakeup if the tab
// still exists and the WS port is reachable.

const DEFAULT_RELAY_PORT = 18792

const state = {
  attachedTabId: null, // number or null
  ws: null,
  nextReqId: 1,
  pending: new Map(), // extension-originated req id → waiting promise
}

async function getRelayPort() {
  const { relayPort } = await chrome.storage.local.get('relayPort')
  return relayPort || DEFAULT_RELAY_PORT
}

function log(...args) {
  console.log('[snth-relay]', ...args)
}

// --- WS lifecycle ----------------------------------------------------------

async function openRelay() {
  const port = await getRelayPort()
  const url = `ws://127.0.0.1:${port}/extension`
  log('opening', url)
  const ws = new WebSocket(url)

  ws.addEventListener('open', () => {
    log('ws open')
    // Announce ourselves so the companion knows which tab is live.
    ws.send(JSON.stringify({
      type: 'hello',
      tabId: state.attachedTabId,
      extensionVersion: chrome.runtime.getManifest().version,
    }))
  })

  ws.addEventListener('message', async (ev) => {
    let msg
    try { msg = JSON.parse(ev.data) } catch (_) { return }

    // Companion-to-browser CDP command: {id, method, params, sessionId?}
    // We forward to Chrome and post the result back.
    if (typeof msg.id !== 'undefined' && msg.method) {
      const target = state.attachedTabId ? { tabId: state.attachedTabId } : null
      if (!target) {
        ws.send(JSON.stringify({ id: msg.id, error: { code: -1, message: 'not attached' } }))
        return
      }
      try {
        const result = await chrome.debugger.sendCommand(target, msg.method, msg.params || {})
        ws.send(JSON.stringify({ id: msg.id, result }))
      } catch (e) {
        ws.send(JSON.stringify({ id: msg.id, error: { code: -32603, message: String(e && e.message || e) } }))
      }
    }
  })

  ws.addEventListener('close', () => {
    log('ws close, will retry in 3s if still attached')
    if (state.ws === ws) state.ws = null
    if (state.attachedTabId != null) {
      setTimeout(() => { if (state.attachedTabId != null) openRelay() }, 3000)
    }
  })

  ws.addEventListener('error', (e) => {
    log('ws error', e.message || '(no message — is the companion running?)')
  })

  state.ws = ws
}

// --- chrome.debugger events → ws -------------------------------------------

chrome.debugger.onEvent.addListener((source, method, params) => {
  if (!state.ws || state.ws.readyState !== WebSocket.OPEN) return
  state.ws.send(JSON.stringify({ event: method, params, tabId: source.tabId }))
})

chrome.debugger.onDetach.addListener((source, reason) => {
  log('debugger detached', source, reason)
  if (source.tabId === state.attachedTabId) {
    state.attachedTabId = null
    if (state.ws) {
      try { state.ws.close() } catch (_) {}
      state.ws = null
    }
    setActionTitle(null)
  }
})

// --- icon click: attach or detach ------------------------------------------

chrome.action.onClicked.addListener(async (tab) => {
  if (state.attachedTabId === tab.id) {
    // Detach.
    try { await chrome.debugger.detach({ tabId: tab.id }) } catch (_) {}
    state.attachedTabId = null
    if (state.ws) { try { state.ws.close() } catch (_) {} state.ws = null }
    setActionTitle(null)
    return
  }
  // Attach.
  try {
    if (state.attachedTabId) {
      try { await chrome.debugger.detach({ tabId: state.attachedTabId }) } catch (_) {}
    }
    await chrome.debugger.attach({ tabId: tab.id }, '1.3')
    state.attachedTabId = tab.id
    setActionTitle(tab.id)
    await openRelay()
  } catch (e) {
    log('attach failed', e)
  }
})

function setActionTitle(tabId) {
  chrome.action.setTitle({
    title: tabId
      ? `Attached to tab ${tabId} — click to detach`
      : 'SNTH Companion Relay (click to attach/detach)',
  })
}
