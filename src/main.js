import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { open } from '@tauri-apps/plugin-dialog'

const originInput = document.querySelector('#origin')
const tokenInput = document.querySelector('#token')
const folderInput = document.querySelector('#folder')
const deviceNameInput = document.querySelector('#deviceName')
const statusEl = document.querySelector('#status')
const logEl = document.querySelector('#log')

function appendLog(message) {
  const timestamp = new Date().toLocaleTimeString()
  logEl.textContent += `[${timestamp}] ${message}\n`
  logEl.scrollTop = logEl.scrollHeight
}

function setStatus(kind, message) {
  statusEl.className = `status ${kind}`
  statusEl.textContent = message
}

function getPayload() {
  return {
    origin: originInput.value.trim(),
    token: tokenInput.value.trim(),
    folder: folderInput.value.trim(),
    deviceName: deviceNameInput.value.trim()
  }
}

async function refreshStatus() {
  try {
    const status = await invoke('get_sync_status')
    if (status.running) {
      setStatus(
        'running',
        `Running for ${status.eventCode || 'event'} from ${status.folder || 'folder'}`
      )
    } else {
      setStatus('idle', status.lastMessage || 'Idle')
    }
  } catch (error) {
    setStatus('error', String(error))
  }
}

document.querySelector('#pickFolder').addEventListener('click', async () => {
  const selected = await open({
    directory: true,
    multiple: false
  })
  if (typeof selected === 'string') {
    folderInput.value = selected
  }
})

document.querySelector('#start').addEventListener('click', async () => {
  try {
    const status = await invoke('start_sync', { input: getPayload() })
    appendLog('Started local sync.')
    if (status.eventCode) {
      appendLog(`Connected to event ${status.eventCode}.`)
    }
    setStatus(
      'running',
      `Running for ${status.eventCode || 'event'} from ${status.folder || 'folder'}`
    )
  } catch (error) {
    setStatus('error', String(error))
    appendLog(`Start failed: ${error}`)
  }
})

document.querySelector('#stop').addEventListener('click', async () => {
  try {
    const status = await invoke('stop_sync')
    setStatus('idle', status.lastMessage || 'Stopped')
    appendLog('Stopped local sync.')
  } catch (error) {
    setStatus('error', String(error))
    appendLog(`Stop failed: ${error}`)
  }
})

document.querySelector('#clearState').addEventListener('click', async () => {
  try {
    await invoke('clear_sync_cache', { token: tokenInput.value.trim() })
    appendLog('Cleared local cache for the current token.')
  } catch (error) {
    setStatus('error', String(error))
    appendLog(`Cache clear failed: ${error}`)
  }
})

document.querySelector('#clearLog').addEventListener('click', () => {
  logEl.textContent = ''
})

listen('sync-log', (event) => {
  appendLog(event.payload?.message || String(event.payload))
})

listen('sync-status', (event) => {
  const payload = event.payload || {}
  if (payload.running) {
    setStatus(
      'running',
      `Running for ${payload.eventCode || 'event'} from ${payload.folder || 'folder'}`
    )
  } else if (payload.lastError) {
    setStatus('error', payload.lastError)
  } else {
    setStatus('idle', payload.lastMessage || 'Idle')
  }
})

listen('sync-cleanup', async (event) => {
  const payload = event.payload || {}
  const shouldDelete = window.confirm(
    payload.message ||
      'Sync ended. Remove local sync cache for this event from this computer?'
  )
  if (shouldDelete && payload.token) {
    try {
      await invoke('clear_sync_cache', { token: payload.token })
      appendLog('Local event cache removed after session ended.')
    } catch (error) {
      appendLog(`Cleanup failed: ${error}`)
    }
  }
})

refreshStatus()

