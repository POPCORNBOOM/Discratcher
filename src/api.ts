import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { documentDir } from '@tauri-apps/api/path'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { open, save } from '@tauri-apps/plugin-dialog'
import { openUrl, revealItemInDir } from '@tauri-apps/plugin-opener'
import { t } from './i18n'
import type { DiscGeometryPreset } from './types'
import { fetchIndex, fetchPreset, type CatalogEntry } from './catalog'

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

export async function loadIndex(): Promise<CatalogEntry[]> {
  return mock ? mock.index() : fetchIndex()
}

export async function loadPreset(id: string): Promise<DiscGeometryPreset> {
  return mock ? mock.preset(id) : fetchPreset(id)
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

let mockPicks = 0

export async function pickImage(): Promise<string | null> {
  if (mock) return ['/Users/demo/artwork.png', '/Users/demo/wide.png'][mockPicks++ % 2]
  const r = await open({
    multiple: false,
    filters: [{ name: t('dialog.imageFilter'), extensions: ['png', 'jpg', 'jpeg', 'webp', 'bmp', 'tiff'] }],
  })
  return typeof r === 'string' ? r : null
}

let mockModels = 0

export async function pickModel(): Promise<string | null> {
  if (mock) return ['/Users/demo/star.obj', '/Users/demo/gem.glb'][mockModels++ % 2]
  const r = await open({
    multiple: false,
    filters: [{ name: t('dialog.modelFilter'), extensions: ['obj', 'gltf', 'glb'] }],
  })
  return typeof r === 'string' ? r : null
}

/** A design folder to open. */
export async function pickDesign(): Promise<string | null> {
  if (mock) return mock.lastDesignDir()
  const r = await open({ directory: true, multiple: false })
  return typeof r === 'string' ? r : null
}

/** A design on the app's list of ones opened or saved before. */
export interface RecentDesign {
  dir: string
  openedMs: number
  /** The folder still holds a design. */
  found: boolean
}

/** Show a file or folder in Finder or the file manager. */
export async function revealPath(path: string): Promise<void> {
  if (mock) return
  await revealItemInDir(path)
}

/** An older scene JSON file to import as an unsaved design. */
export async function pickLegacyScene(): Promise<string | null> {
  if (mock) return '/Users/demo/legacy.holo.json'
  const r = await open({ multiple: false, filters: [{ name: t('dialog.designFilter'), extensions: ['json'] }] })
  return typeof r === 'string' ? r : null
}

let mockLocations = 0

/** A parent folder to place a new design folder inside. */
export async function pickFolder(defaultPath?: string): Promise<string | null> {
  if (mock) return ['/Users/demo/Documents', '/Users/demo/Desktop'][mockLocations++ % 2]
  const r = await open({ directory: true, multiple: false, ...(defaultPath ? { defaultPath } : {}) })
  return typeof r === 'string' ? r : null
}

/** Default parent folder for saving designs. */
export async function defaultDesignLocation(): Promise<string> {
  if (mock) return '/Users/demo/Documents'
  try {
    return await documentDir()
  } catch {
    return ''
  }
}

let mockSaved = '/Users/demo/flowers.discratch'
const COMPOSITION = { name: '', extensions: ['discratch'] }

export async function pickComposition(): Promise<string | null> {
  if (mock) return mockSaved
  const r = await open({ multiple: false, filters: [{ ...COMPOSITION, name: t('dialog.compositionFilter') }] })
  return typeof r === 'string' ? r : null
}

export async function pickCompositionSave(defaultPath: string): Promise<string | null> {
  if (mock) return (mockSaved = `/Users/demo/${defaultPath}`)
  return save({ defaultPath, filters: [{ ...COMPOSITION, name: t('dialog.compositionFilter') }] })
}

export async function pickDataPath(defaultPath: string): Promise<string | null> {
  if (mock) return `/Users/demo/${defaultPath}`
  return save({ defaultPath, filters: [{ name: t('dialog.dataFilter'), extensions: ['json'] }] })
}

export async function pickSavePath(defaultPath: string): Promise<string | null> {
  if (mock) return `/Users/demo/${defaultPath}`
  return save({ defaultPath, filters: [{ name: t('dialog.isoFilter'), extensions: ['iso'] }] })
}
