const input = document.getElementById('port')
const btn = document.getElementById('save')
const status = document.getElementById('status')

chrome.storage.local.get('relayPort').then(({ relayPort }) => {
  input.value = relayPort || 18792
})

btn.addEventListener('click', async () => {
  const v = parseInt(input.value, 10)
  if (!v || v < 1 || v > 65535) {
    status.textContent = 'Invalid port.'
    return
  }
  await chrome.storage.local.set({ relayPort: v })
  status.textContent = 'Saved. Re-click the extension icon to reconnect.'
})
