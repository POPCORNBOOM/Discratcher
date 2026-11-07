import type { DiscGeometryPreset } from './types'

/** Calibration data lives in this repository's issues. A workflow publishes open ones as one file. */
const REPOSITORY = 'POPCORNBOOM/Discratcher'
export const REPOSITORY_URL = `https://github.com/${REPOSITORY}`
const RATING_TEMPLATE = 'rating.yml'
/** A maintainer adds this after checking a submission. */
const VERIFIED_LABEL = 'verified'
const TEMPLATE = 'calibration.yml'
const CATALOG_URL = `https://raw.githubusercontent.com/${REPOSITORY}/catalog/catalog.json`
const TIMEOUT_MS = 8000
/** Shorter queries match nearly everything. */
export const MIN_QUERY = 3

export interface Rating {
  average: number
  count: number
}

export interface CatalogEntry {
  preset: DiscGeometryPreset
  /** The GitHub account that opened the issue: nobody types it, so nobody can fake it. */
  contributor: string
  verified: boolean
  rating: Rating | null
}

interface Issue {
  number: number
  body: string | null
  pull_request?: unknown
  labels: { name: string }[]
  user?: { login?: string } | null
}

/** Disc diameter, read off the measured geometry rather than asked of the contributor. */
export function sizeCm(preset: DiscGeometryPreset) {
  return preset.outerRadiusMm > 45 ? 12 : 8
}

export const sizeLabel = (preset: DiscGeometryPreset) => `${sizeCm(preset)} cm`

/** The issue number a downloaded preset came from, or null for anything made locally. */
export function issueOf(id: string) {
  return id.match(/^issue-(.+)$/)?.[1] ?? null
}

/** Discs print codes with uneven spacing and punctuation, so only letters and digits count. */
export function normalizeCode(text: string) {
  return text.toUpperCase().replace(/[^0-9A-Z]/g, '')
}

export function matches(preset: DiscGeometryPreset, query: string) {
  const q = normalizeCode(query)
  return q.length >= MIN_QUERY && [preset.stamperCode, preset.mediaId, preset.name, preset.brand ?? ''].some((f) => normalizeCode(f).includes(q))
}

/** Issues are open to anyone: keep only entries with the full numeric shape. */
function isPreset(p: Partial<DiscGeometryPreset>): boolean {
  const numbers = [p.totalSectors, p.channelBitLengthNm, p.innerRadiusMm, p.outerRadiusMm, p.pitchLinear, p.pitchQuadratic, p.pitchCubic, p.basePitchUm, p.rmsResidualDeg]
  return (
    numbers.every(Number.isFinite) &&
    [p.name, p.stamperCode].every((v) => typeof v === 'string' && v) &&
    Array.isArray(p.measuredSpanMm) &&
    p.measuredSpanMm.length === 2 &&
    p.measuredSpanMm.every(Number.isFinite)
  )
}

export function parseIssue(issue: Issue): CatalogEntry | null {
  const fenced = issue.body?.match(/```json\s*([\s\S]*?)```/)?.[1]
  if (issue.pull_request || !fenced) return null
  let data: Partial<DiscGeometryPreset>
  try {
    data = JSON.parse(fenced)
  } catch {
    return null
  }
  if (!data || !isPreset(data)) return null
  const preset = {
    ...data,
    id: `issue-${issue.number}`,
    mediaId: String(data.mediaId ?? ''),
    brand: String(data.brand ?? '').slice(0, 40),
    burnedAt: '',
    driveModel: String(data.driveModel ?? ''),
    createdAt: String(data.createdAt ?? ''),
  } as DiscGeometryPreset
  return {
    preset,
    contributor: issue.user?.login ?? '',
    verified: issue.labels.some((l) => l.name === VERIFIED_LABEL),
    rating: null,
  }
}

/** One vote per account per calibration: the newest issue wins, which is the first listed. */
export function parseRatings(issues: Issue[]): Map<string, Rating> {
  const seen = new Set<string>()
  const sums = new Map<string, { total: number; count: number }>()
  for (const issue of issues) {
    const fenced = issue.body?.match(/```json\s*([\s\S]*?)```/)?.[1]
    if (issue.pull_request || !fenced || !issue.user?.login) continue
    let data: { target?: unknown; score?: unknown }
    try {
      data = JSON.parse(fenced)
    } catch {
      continue
    }
    const target = String(data.target ?? '')
    const score = Number(data.score)
    if (!target || !Number.isInteger(score) || score < 1 || score > 5) continue
    const key = `${issue.user.login}/${target}`
    if (seen.has(key)) continue
    seen.add(key)
    const sum = sums.get(target) ?? { total: 0, count: 0 }
    sums.set(target, { total: sum.total + score, count: sum.count + 1 })
  }
  return new Map([...sums].map(([target, { total, count }]) => [target, { average: total / count, count }]))
}

/** The whole library, submissions still under review included: those stay out of search. */
export async function fetchCatalog(): Promise<CatalogEntry[]> {
  const r = await fetch(CATALOG_URL, { signal: AbortSignal.timeout(TIMEOUT_MS) })
  if (!r.ok) throw new Error(`HTTP ${r.status}`)
  const { calibrations, ratings: ratingIssues }: { calibrations: Issue[]; ratings: Issue[] } = await r.json()
  const ratings = parseRatings(ratingIssues)
  return calibrations.flatMap((i) => {
    const entry = parseIssue(i)
    return entry ? [{ ...entry, rating: ratings.get(String(i.number)) ?? null }] : []
  })
}

/** Best rated first; unrated entries follow, newest first within a tie. */
export function byRating(a: CatalogEntry, b: CatalogEntry) {
  return (b.rating?.average ?? -1) - (a.rating?.average ?? -1) || (b.rating?.count ?? 0) - (a.rating?.count ?? 0) || b.preset.createdAt.localeCompare(a.preset.createdAt)
}

/** A pre-filled issue form. Nothing is sent until the user submits it on GitHub. */
/** The GitHub page of a calibration from the library. */
export const issueUrl = (preset: DiscGeometryPreset) => `${REPOSITORY_URL}/issues/${issueOf(preset.id)}`

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
