import type { DiscGeometryPreset, PresetLook } from './types'

/** Calibration data lives in this repository's issues. A workflow publishes the library as files. */
const REPOSITORY = 'POPCORNBOOM/Discratcher'
export const REPOSITORY_URL = `https://github.com/${REPOSITORY}`
const RATING_TEMPLATE = 'rating.yml'
const TEMPLATE = 'calibration.yml'
const PUBLISHED_URL = `https://raw.githubusercontent.com/${REPOSITORY}/catalog`
/** What a search reads: printed fields and ratings, without the geometry. */
const INDEX_URL = `${PUBLISHED_URL}/index.json`
/** One folder per calibration; the geometry inside is fetched only to use that one. */
const LIBRARY_URL = `${PUBLISHED_URL}/library`
const TIMEOUT_MS = 8000
/** Long enough to answer every keystroke locally, short enough that a new calibration shows up. */
const INDEX_TTL_MS = 10 * 60 * 1000

export interface Rating {
  average: number
  count: number
}

/** One row of the library; the same shape previews a disc, so a library row can too. */
export interface CatalogEntry extends PresetLook {
  id: string
  brand: string
  /** Disc diameter, worked out by the workflow so searching never loads the geometry. */
  size: 8 | 12
  /** The GitHub account that opened the issue: nobody types it, so nobody can fake it. */
  contributor: string
  verified: boolean
  rating: Rating | null
  createdAt: string
}

async function get<T>(url: string): Promise<T> {
  const r = await fetch(url, { signal: AbortSignal.timeout(TIMEOUT_MS) })
  if (!r.ok) throw new Error(`HTTP ${r.status}`)
  return r.json() as Promise<T>
}

let index: { at: number; entries: CatalogEntry[] } | null = null
let pulling: Promise<CatalogEntry[]> | null = null

/** Fetched once per session, refreshed when stale; a failed refresh keeps the copy already held. */
export function fetchIndex(): Promise<CatalogEntry[]> {
  if (index && Date.now() - index.at < INDEX_TTL_MS) return Promise.resolve(index.entries)
  pulling ??= get<CatalogEntry[]>(INDEX_URL)
    .then((entries) => (index = { at: Date.now(), entries }).entries)
    .finally(() => (pulling = null))
  return pulling
}

const presets = new Map<string, Promise<DiscGeometryPreset>>()

/** One calibration's geometry. Held for the session, and only fetched when it is really used. */
export function fetchPreset(id: string): Promise<DiscGeometryPreset> {
  const held = presets.get(id)
  if (held) return held
  const pull = get<DiscGeometryPreset>(`${LIBRARY_URL}/${encodeURIComponent(id)}/data.json`).catch((e) => {
    presets.delete(id)
    throw e
  })
  presets.set(id, pull)
  return pull
}

/** Disc diameter, read off the measured geometry rather than asked of the contributor. */
export const sizeCm = (preset: { outerRadiusMm: number }): 8 | 12 => (preset.outerRadiusMm > 45 ? 12 : 8)

export const sizeLabel = (cm: number) => `${cm} cm`

/** The issue number a downloaded preset came from, or null for anything made locally. */
export function issueOf(id: string) {
  return id.match(/^issue-(.+)$/)?.[1] ?? null
}

/** Discs print codes with uneven spacing and punctuation, so only letters and digits count. */
export function normalizeCode(text: string) {
  return text.toUpperCase().replace(/[^0-9A-Z]/g, '')
}

/** What a calibration is called: the code printed on the disc, or the media it was measured from. */
export const labelOf = (entry: PresetLook) => entry.stamperCode || entry.mediaId || '—'

/** An empty query lists everything; any printed field may carry the rest. */
export function matches(entry: PresetLook & { brand?: string }, query: string) {
  const q = normalizeCode(query)
  return !q || [entry.stamperCode, entry.mediaId, entry.brand ?? ''].some((f) => normalizeCode(f).includes(q))
}

/** Best rated first; unrated entries follow, newest first within a tie. */
export function byRating(a: CatalogEntry, b: CatalogEntry) {
  return (b.rating?.average ?? -1) - (a.rating?.average ?? -1) || (b.rating?.count ?? 0) - (a.rating?.count ?? 0) || b.createdAt.localeCompare(a.createdAt)
}

/** The GitHub page of a calibration from the library. */
export const issueUrl = (preset: { id: string }) => `${REPOSITORY_URL}/issues/${issueOf(preset.id)}`

/** A pre-filled issue form. Nothing is sent until the user submits it on GitHub. */
export function contributionUrl(preset: DiscGeometryPreset) {
  const { id: _id, burnedAt: _burnedAt, ...fields } = preset
  const data = JSON.stringify(fields, null, 2)
  const query = new URLSearchParams({ template: TEMPLATE, title: `[标定] ${preset.stamperCode}`, data })
  return `${REPOSITORY_URL}/issues/new?${query}`
}

/** A pre-filled rating form for a downloaded calibration; GitHub supplies who is rating. */
export function ratingUrl(target: string, score: number) {
  const data = JSON.stringify({ target, score })
  const query = new URLSearchParams({ template: RATING_TEMPLATE, title: `[评分] #${target} ${score}/5`, data })
  return `${REPOSITORY_URL}/issues/new?${query}`
}
