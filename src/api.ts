import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { open, save } from '@tauri-apps/plugin-dialog'
import { openUrl } from '@tauri-apps/plugin-opener'
import { t } from './i18n'
import { fetchCatalog, type CatalogEntry } from './catalog'

const inTauri = '__TAURI_INTERNALS__' in window

/** Browser-only stand-in, so the interface can be developed without the native shell. */
const mock = !inTauri && import.meta.env.DEV ? await import('./mock') : null

/** Browser-only development build; there is no native shell to talk to. */
/** What the native side reports when a job is cancelled; compared against, never shown. */
export const CANCELLED = '已取消'

export const isMock = !!mock
export const mockDev = mock

export function call<T>(cmd: string, args: Record<string, unknown> = {}): Promise<T> {
  return mock ? (mock.call(cmd, args) as Promise<T>) : invoke<T>(cmd, args)
}

export async function on<T>(event: string, cb: (p: T) => void): Promise<() => void> {
  if (mock) return mock.on(event, cb as (p: unknown) => void)
  return listen<T>(event, (e) => cb(e.payload))
}

export async function loadCatalog(): Promise<CatalogEntry[]> {
  return mock ? mock.catalog() : fetchCatalog()
}

/** The page draws the window frame, so these are the only window calls the UI makes. */
export async function windowControl(action: 'minimize' | 'toggleMaximize' | 'close'): Promise<void> {
  if (!inTauri) return
  const w = getCurrentWindow()
  if (action === 'minimize') await w.minimize()
  else if (action === 'toggleMaximize') await w.toggleMaximize()
  else await w.close()
}

/** False where there is no native window, so the restore icon never lies in a browser. */
export async function isWindowMaximized(): Promise<boolean> {
  return inTauri ? getCurrentWindow().isMaximized() : false
}

export async function openInBrowser(url: string) {
  if (mock) mock.openUrl(url)
  else await openUrl(url)
}

export async function pickImage(): Promise<string | null> {
  if (mock) return '/Users/demo/artwork.png'
  const r = await open({
    multiple: false,
    filters: [{ name: t('dialog.imageFilter'), extensions: ['png', 'jpg', 'jpeg', 'webp', 'bmp', 'tiff'] }],
  })
  return typeof r === 'string' ? r : null
}

export async function pickSavePath(defaultPath: string): Promise<string | null> {
  if (mock) return `/Users/demo/${defaultPath}`
  return save({ defaultPath, filters: [{ name: t('dialog.isoFilter'), extensions: ['iso'] }] })
}
