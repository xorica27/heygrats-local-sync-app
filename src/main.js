import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { open } from '@tauri-apps/plugin-dialog'

const APP_VERSION = typeof __APP_VERSION__ === 'string' ? __APP_VERSION__ : 'dev'
const DEFAULT_ORIGIN = 'https://heygrats.com'
const appVersionEl = document.querySelector('#appVersion')
const originInput = document.querySelector('#origin')
const originBadge = document.querySelector('#originBadge')
const tokenInput = document.querySelector('#token')
const folderInput = document.querySelector('#folder')
const statusEl = document.querySelector('#status')
const logEl = document.querySelector('#log')
const storageKey = 'heygrats-local-sync-ui'
const isTauriRuntime = Boolean(window.__TAURI_INTERNALS__?.invoke)

restoreDraft()
updateOriginBadge()
renderVersion()

originInput?.addEventListener('input', () => {
  updateOriginBadge()
  persistDraft()
})

tokenInput?.addEventListener('input', persistDraft)
folderInput?.addEventListener('input', persistDraft)

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
    origin: originInput.value.trim() || DEFAULT_ORIGIN,
    token: tokenInput.value.trim(),
    folder: folderInput.value.trim(),
    deviceName: ''
  }
}

function updateOriginBadge() {
  if (originBadge) {
    originBadge.textContent = originInput.value.trim() || DEFAULT_ORIGIN
  }
}

function renderVersion() {
  if (appVersionEl) {
    appVersionEl.textContent = `Version ${APP_VERSION}`
  }
}

function persistDraft() {
  try {
    window.localStorage.setItem(
      storageKey,
      JSON.stringify({
        origin: originInput.value.trim(),
        folder: folderInput.value.trim()
      })
    )
  } catch {}
}

function restoreDraft() {
  try {
    const raw = window.localStorage.getItem(storageKey)
    if (!raw) return
    const value = JSON.parse(raw)
    if (typeof value.origin === 'string') originInput.value = value.origin
    if (typeof value.folder === 'string') folderInput.value = value.folder
  } catch {}
}

async function refreshStatus() {
  if (!isTauriRuntime) {
    setStatus('idle', 'Browser preview mode')
    return
  }
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
  if (!isTauriRuntime) {
    appendLog('Folder picker works in the desktop app.')
    return
  }
  const selected = await open({
    directory: true,
    multiple: false
  })
  if (typeof selected === 'string') {
    folderInput.value = selected
    persistDraft()
  }
})

document.querySelector('#start').addEventListener('click', async () => {
  if (!isTauriRuntime) {
    setStatus('idle', 'Start Sync works in the desktop app.')
    return
  }
  try {
    const status = await invoke('start_sync', { input: getPayload() })
    appendLog('Started local sync.')
    if (status.eventCode) {
      appendLog(`Connected to event ${status.eventCode}.`)
    }
    persistDraft()
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
  if (!isTauriRuntime) {
    setStatus('idle', 'Stop Sync works in the desktop app.')
    return
  }
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
  if (!isTauriRuntime) {
    appendLog('Clear Local Cache works in the desktop app.')
    return
  }
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

if (isTauriRuntime) {
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
} else {
  appendLog('Preview mode detected. Tauri desktop APIs are unavailable in browser.')
}

refreshStatus()
