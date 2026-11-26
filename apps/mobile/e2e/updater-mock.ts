import type { Page } from '@playwright/test'
import type { UpdateStatus } from '../src/updater'

export const MB = 1024 * 1024

export function mockStatus(patch: Partial<UpdateStatus> = {}): UpdateStatus {
  return {
    stage: 'available',
    current_version: '0.1.0-alpha.1',
    available_version: '0.2.0',
    release_notes: '• Faster search over Tor\n• Direct-order reliability fixes\n• In-app update verification',
    downloaded_bytes: 0,
    total_bytes: 43.8 * MB,
    bytes_per_sec: null,
    checked_at: 1764100000,
    error: null,
    automatic: true,
    location_problem: null,
    can_check: true,
    ...patch
  }
}

export async function setupMockUpdater(page: Page, initial: Partial<UpdateStatus> = {}) {
  const status = mockStatus(initial)
  await page.addInitScript(initStatus => {
    ;(window as unknown as { __OB_SHELL_BUILD__: unknown }).__OB_SHELL_BUILD__ = {
      version: '0.1.0-alpha.1',
      commit: 'unknown',
      dirty: false,
      buildDate: '2026-11-26',
      buildKind: 'release'
    }

    const MB_SIZE = 1024 * 1024
    let currentStatus = { ...initStatus }
    const calls: Array<{ command: string; args?: Record<string, unknown> }> = []
    const listeners = new Map<number, (event: unknown) => void>()
    let nextCallbackId = 1
    let nextInstallStatus: UpdateStatus | null = null
    let nextCancelStatus: UpdateStatus | null = null
    let nextCheckStatus: UpdateStatus | null = null

    function broadcast(next: UpdateStatus) {
      currentStatus = { ...next }
      for (const listener of listeners.values()) {
        try {
          listener({ event: 'ob://update-status', payload: currentStatus })
        } catch {}
      }
    }

    const invoke = async (command: string, args?: Record<string, unknown>) => {
      calls.push({ command, args })
      switch (command) {
        case 'update_status':
          return currentStatus
        case 'check_for_updates': {
          broadcast({ ...currentStatus, stage: 'checking' })
          if (nextCheckStatus) {
            const reply = nextCheckStatus
            setTimeout(() => broadcast(reply), 150)
          } else {
            setTimeout(() => {
              broadcast({
                ...currentStatus,
                stage: 'up_to_date',
                checked_at: Math.floor(Date.now() / 1000)
              })
            }, 150)
          }
          return { ...currentStatus, stage: 'checking' }
        }
        case 'install_available_update': {
          if (nextInstallStatus) {
            broadcast(nextInstallStatus)
            return nextInstallStatus
          }
          const downloadingStatus: UpdateStatus = {
            ...currentStatus,
            stage: 'downloading',
            downloaded_bytes: Math.round(18.4 * MB_SIZE),
            total_bytes: Math.round(43.8 * MB_SIZE),
            bytes_per_sec: Math.round(1.2 * MB_SIZE)
          }
          broadcast(downloadingStatus)
          return downloadingStatus
        }
        case 'cancel_update_download': {
          if (nextCancelStatus) {
            broadcast(nextCancelStatus)
            return nextCancelStatus
          }
          const availableStatus: UpdateStatus = {
            ...currentStatus,
            stage: 'available',
            downloaded_bytes: 0,
            bytes_per_sec: null
          }
          broadcast(availableStatus)
          return availableStatus
        }
        case 'set_automatic_updates': {
          const enabled = Boolean(args?.enabled)
          const updated: UpdateStatus = { ...currentStatus, automatic: enabled }
          broadcast(updated)
          return updated
        }
        case 'plugin:event|listen': {
          const handlerId = args?.handler as number
          return handlerId
        }
        case 'plugin:event|unlisten': {
          return null
        }
        default:
          return null
      }
    }

    ;(window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = {
      invoke,
      transformCallback: (callback: (event: unknown) => void) => {
        const id = nextCallbackId++
        listeners.set(id, callback)
        return id
      },
      unregisterCallback: (id: number) => {
        listeners.delete(id)
      }
    }

    ;(window as unknown as { __MOCK_SHELL__: unknown }).__MOCK_SHELL__ = {
      getStatus: () => currentStatus,
      setStatus: (patch: Partial<UpdateStatus>) => {
        broadcast({ ...currentStatus, ...patch })
        return currentStatus
      },
      setNextInstallStatus: (next: UpdateStatus | null) => {
        nextInstallStatus = next
      },
      setNextCancelStatus: (next: UpdateStatus | null) => {
        nextCancelStatus = next
      },
      setNextCheckStatus: (next: UpdateStatus | null) => {
        nextCheckStatus = next
      },
      getCalls: () => calls,
      clearCalls: () => {
        calls.length = 0
      }
    }
  }, status)
}
