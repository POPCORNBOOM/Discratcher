import { ref } from 'vue'
import type { OpticalDriveInfo } from '../types'

// One JSON file per language in ./locales. Adding a language is adding a file: it shows up
// in the Help > Language menu by itself. "_name" is how the language names itself.
const files = import.meta.glob<Record<string, string>>('./locales/*.json', { eager: true, import: 'default' })

const dictionaries: Record<string, Record<string, string>> = {}
for (const [path, strings] of Object.entries(files)) {
  const code = path.match(/\/([^/]+)\.json$/)?.[1]
  if (code) dictionaries[code] = strings
}

/** Strings missing from a translation fall back to this one, so a partial file still works. */
const SOURCE = 'zh-CN'
const STORAGE_KEY = 'discratcher.locale'

export const locales = Object.entries(dictionaries)
  .map(([code, strings]) => ({ code, name: strings._name ?? code }))
  .sort((a, b) => (a.code === SOURCE ? -1 : b.code === SOURCE ? 1 : a.code.localeCompare(b.code)))

function stored() {
  try {
    return localStorage.getItem(STORAGE_KEY)
  } catch {
    return null
  }
}

function initial() {
  const saved = stored()
  if (saved && dictionaries[saved]) return saved
  for (const wanted of navigator.languages ?? [navigator.language]) {
    const exact = Object.keys(dictionaries).find((c) => c.toLowerCase() === wanted.toLowerCase())
    const base = Object.keys(dictionaries).find((c) => c.split('-')[0].toLowerCase() === wanted.split('-')[0].toLowerCase())
    if (exact ?? base) return (exact ?? base)!
  }
  return dictionaries.en ? 'en' : SOURCE
}

export const locale = ref(initial())
document.documentElement.lang = locale.value

export function setLocale(code: string) {
  if (!dictionaries[code]) return
  locale.value = code
  document.documentElement.lang = code
  try {
    localStorage.setItem(STORAGE_KEY, code)
  } catch {
    // The choice just won't survive a restart.
  }
}

/** Looks a string up by key; `{name}` in the text is filled from `params`. */
export function t(key: string, params?: Record<string, string | number>): string {
  const text = dictionaries[locale.value]?.[key] ?? dictionaries[SOURCE]?.[key] ?? key
  return params ? text.replace(/\{(\w+)\}/g, (whole, name) => (name in params ? String(params[name]) : whole)) : text
}

/** What to say about a disc that cannot be recorded, when the drive says why. */
const UNSUPPORTED_KEYS: Record<string, string> = {
  'dvd-ram': 'drive.unsupportedDvdRam',
  'dual-layer': 'drive.unsupportedDualLayer',
  'blu-ray': 'drive.unsupportedBluRay',
}

/** The words for a disc Discratcher cannot record. */
export function unsupportedText(d: OpticalDriveInfo): string {
  const key = d.unsupportedReason ? UNSUPPORTED_KEYS[d.unsupportedReason] : undefined
  return key ? t(key) : t('drive.unsupported', { type: d.mediaType })
}
